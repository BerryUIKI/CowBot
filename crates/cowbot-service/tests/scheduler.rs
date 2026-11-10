use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cowbot_agent::StandardRoles;
use cowbot_bus::EventBus;
use cowbot_core::agent::AgentId;
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::assignment::{
    Assignment, AssignmentMode, AssignmentOrigin, AssignmentRequester, AssignmentStatus, ReplyMode,
};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::task::{Task, TaskStatus};
use cowbot_llm::{ChatCompletionRequest, ChatCompletionResponse, LlmProvider, MockLlmProvider};
use cowbot_service::{bootstrap_local, AssignmentScheduler, AssignmentService, CrewRunOptions};
use cowbot_store::Store;
use cowbot_tools::ToolRegistry;

async fn setup() -> (Store, WorkspaceId) {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let context = bootstrap_local(&store).await.unwrap();
    (store, context.workspace.id)
}

fn requester() -> AssignmentRequester {
    AssignmentRequester::User("user-1".to_string())
}

fn options(max_concurrency: usize) -> CrewRunOptions {
    CrewRunOptions {
        approval_level: ApprovalLevel::Ask,
        channel_id: None,
        max_concurrency,
        use_model_permissions: false,
    }
}

/// LLM that records how many completions overlap.
struct SlowCountingLlm {
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    delay: Duration,
}

#[async_trait]
impl LlmProvider for SlowCountingLlm {
    fn name(&self) -> &str {
        "slow-counting"
    }

    async fn chat_complete(
        &self,
        _request: &ChatCompletionRequest,
    ) -> CowResult<ChatCompletionResponse> {
        let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(current, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(ChatCompletionResponse {
            content: Some("completed".to_string()),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_string()),
            usage: None,
            raw_usage: None,
        })
    }
}

struct FailingLlm;

#[async_trait]
impl LlmProvider for FailingLlm {
    fn name(&self) -> &str {
        "failing"
    }

    async fn chat_complete(
        &self,
        _request: &ChatCompletionRequest,
    ) -> CowResult<ChatCompletionResponse> {
        Err(CowError::Llm("provider exploded".to_string()))
    }
}

#[tokio::test]
async fn independent_roots_run_concurrently() {
    let (store, workspace_id) = setup().await;
    let coder = StandardRoles::coder();
    let coder_id = coder.id.clone();

    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let llm = Arc::new(SlowCountingLlm {
        active,
        max_active: max_active.clone(),
        delay: Duration::from_millis(100),
    });

    let mut scheduler =
        AssignmentScheduler::new(store.clone(), EventBus::default_bus(), workspace_id, 2);
    scheduler.register_agent(coder, llm, ToolRegistry::new(), "mock-model");

    let first = Task::new("First root", "First output").assign_to(coder_id.clone());
    let second = Task::new("Second root", "Second output").assign_to(coder_id);
    let run_options = options(2);
    scheduler
        .enqueue(&first, requester(), AssignmentOrigin::User, &run_options)
        .await
        .unwrap();
    scheduler
        .enqueue(&second, requester(), AssignmentOrigin::User, &run_options)
        .await
        .unwrap();

    let completed = scheduler.run_until_idle().await.unwrap();
    assert_eq!(completed.len(), 2);
    assert!(completed.contains_key(&first.id));
    assert!(completed.contains_key(&second.id));
    assert!(
        max_active.load(Ordering::SeqCst) >= 2,
        "expected two overlapping completions, saw {}",
        max_active.load(Ordering::SeqCst)
    );
}

#[tokio::test]
async fn a_waiting_parent_resumes_with_child_output() {
    let (store, workspace_id) = setup().await;
    let bus = EventBus::default_bus();
    let service = AssignmentService::new(store.clone(), bus.clone());

    let planner = StandardRoles::planner();
    let planner_id = planner.id.clone();
    let coder = StandardRoles::coder();
    let coder_id = coder.id.clone();

    let root_task =
        Task::new("Compose the final answer", "A final answer").assign_to(planner_id.clone());
    store
        .create_task(&workspace_id, None, &root_task)
        .await
        .unwrap();
    let root = service
        .enqueue_root(
            &root_task,
            AssignmentOrigin::User,
            requester(),
            planner_id,
            ApprovalLevel::Ask,
        )
        .await
        .unwrap();
    service.start(&root.id).await.unwrap();
    service.mark_waiting(&root.id).await.unwrap();

    let child_task = Task::new("Gather the facts", "The facts").assign_to(coder_id.clone());
    store
        .create_task(&workspace_id, None, &child_task)
        .await
        .unwrap();
    let child = service
        .delegate(
            &root.id,
            &child_task,
            requester(),
            coder_id,
            ApprovalLevel::Ask,
        )
        .await
        .unwrap();

    let mut scheduler = AssignmentScheduler::new(store.clone(), bus, workspace_id, 4);
    scheduler.register_agent(
        planner,
        Arc::new(MockLlmProvider::echo()),
        ToolRegistry::new(),
        "mock-model",
    );
    scheduler.register_agent(
        coder,
        Arc::new(MockLlmProvider::echo()),
        ToolRegistry::new(),
        "mock-model",
    );

    let completed = scheduler.run_until_idle().await.unwrap();
    assert!(completed.contains_key(&child_task.id));
    assert!(completed.contains_key(&root_task.id));

    let child_output = completed.get(&child_task.id).unwrap();
    let root_output = completed.get(&root_task.id).unwrap();
    assert!(
        root_output.contains(child_output),
        "the parent should receive the child output as context:\n{root_output}"
    );

    let parent = service.get(&root.id).await.unwrap().unwrap();
    assert_eq!(parent.status, AssignmentStatus::Completed);
    assert_eq!(parent.executions, 2);
    assert_eq!(parent.wakes, 1);
    assert_eq!(
        parent.outcome,
        Some(cowbot_core::assignment::AssignmentOutcome::Completed {
            output: root_output.clone()
        })
    );

    // The first execution was started by hand in the test setup, so only the
    // re-entry through the scheduler produced a run row.
    let runs = store.list_runs(&root_task.id).await.unwrap();
    assert_eq!(runs.len(), 1);

    let stored_child = service.get(&child.id).await.unwrap().unwrap();
    assert_eq!(stored_child.status, AssignmentStatus::Completed);
}

#[tokio::test]
async fn a_poll_reply_leaves_the_parent_waiting() {
    let (store, workspace_id) = setup().await;
    let bus = EventBus::default_bus();
    let service = AssignmentService::new(store.clone(), bus.clone());

    let planner = StandardRoles::planner();
    let planner_id = planner.id.clone();
    let coder = StandardRoles::coder();
    let coder_id = coder.id.clone();

    let root_task = Task::new("Wait for the facts", "An answer").assign_to(planner_id.clone());
    store
        .create_task(&workspace_id, None, &root_task)
        .await
        .unwrap();
    let root = service
        .enqueue_root(
            &root_task,
            AssignmentOrigin::User,
            requester(),
            planner_id,
            ApprovalLevel::Ask,
        )
        .await
        .unwrap();
    service.start(&root.id).await.unwrap();
    service.mark_waiting(&root.id).await.unwrap();

    let child_task = Task::new("Gather the facts", "The facts").assign_to(coder_id.clone());
    store
        .create_task(&workspace_id, None, &child_task)
        .await
        .unwrap();
    let child = Assignment::new_child(
        &root,
        child_task.id.clone(),
        requester(),
        coder_id,
        AssignmentMode::Work,
        ReplyMode::Poll,
        ApprovalLevel::Ask,
    );
    store.create_assignment(&child).await.unwrap();

    let mut scheduler = AssignmentScheduler::new(store.clone(), bus, workspace_id, 4);
    scheduler.register_agent(
        coder,
        Arc::new(MockLlmProvider::echo()),
        ToolRegistry::new(),
        "mock-model",
    );

    let completed = scheduler.run_until_idle().await.unwrap();
    assert_eq!(completed.len(), 1);
    assert!(completed.contains_key(&child_task.id));

    let parent = service.get(&root.id).await.unwrap().unwrap();
    assert_eq!(parent.status, AssignmentStatus::Waiting);
    assert_eq!(parent.wakes, 0);
    assert_eq!(store.list_runs(&root_task.id).await.unwrap().len(), 0);
}

#[tokio::test]
async fn a_failed_dependency_fails_the_dependent_task() {
    let (store, workspace_id) = setup().await;

    let failing_profile = StandardRoles::coder();
    let failing = AgentId::new("failing-agent");
    let succeeding = StandardRoles::reviewer();
    let succeeding_id = succeeding.id.clone();

    let mut scheduler =
        AssignmentScheduler::new(store.clone(), EventBus::default_bus(), workspace_id, 2);
    scheduler.register_agent(
        failing_profile,
        Arc::new(FailingLlm),
        ToolRegistry::new(),
        "mock-model",
    );
    scheduler.register_agent(
        succeeding,
        Arc::new(MockLlmProvider::echo()),
        ToolRegistry::new(),
        "mock-model",
    );

    let first = Task::new("Fails", "Nothing").assign_to(failing);
    let second = Task::new("Depends on the failure", "Nothing")
        .assign_to(succeeding_id)
        .with_context_task(first.id.clone());

    let run_options = options(2);
    scheduler
        .enqueue(&first, requester(), AssignmentOrigin::User, &run_options)
        .await
        .unwrap();
    scheduler
        .enqueue(&second, requester(), AssignmentOrigin::User, &run_options)
        .await
        .unwrap();

    let result = scheduler.run_until_idle().await;
    assert!(result.is_err());

    let first_assignments = store.list_assignments_for_task(&first.id).await.unwrap();
    assert_eq!(first_assignments.len(), 1);
    assert_eq!(first_assignments[0].status, AssignmentStatus::Failed);

    let second_assignments = store.list_assignments_for_task(&second.id).await.unwrap();
    assert_eq!(second_assignments.len(), 1);
    assert_eq!(second_assignments[0].status, AssignmentStatus::Failed);
    match &second_assignments[0].outcome {
        Some(cowbot_core::assignment::AssignmentOutcome::Failed { error }) => {
            assert!(
                error.contains("dependency task"),
                "unexpected failure reason: {error}"
            );
        }
        other => panic!("expected a failed outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn an_assignment_without_an_executor_is_failed() {
    let (store, workspace_id) = setup().await;
    let scheduler =
        AssignmentScheduler::new(store.clone(), EventBus::default_bus(), workspace_id, 2);

    let task = Task::new("Unrouted work", "Output").assign_to(AgentId::new("missing-agent"));
    scheduler
        .enqueue(&task, requester(), AssignmentOrigin::User, &options(2))
        .await
        .unwrap();

    let result = scheduler.run_until_idle().await;
    assert!(result.is_err());

    let assignments = store.list_assignments_for_task(&task.id).await.unwrap();
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].status, AssignmentStatus::Failed);
}

#[tokio::test]
async fn a_scoped_scheduler_claims_only_its_own_roots() {
    let (store, workspace_id) = setup().await;
    let own_agent = StandardRoles::planner();
    let foreign_agent = StandardRoles::coder();

    let mut owners = AssignmentScheduler::new(
        store.clone(),
        EventBus::default_bus(),
        workspace_id.clone(),
        2,
    )
    .with_scope();
    owners.register_agent(
        own_agent.clone(),
        Arc::new(MockLlmProvider::echo()),
        ToolRegistry::new(),
        "mock-model",
    );
    let own_task = Task::new("Owned work", "Output").assign_to(own_agent.id.clone());
    owners
        .enqueue(&own_task, requester(), AssignmentOrigin::User, &options(2))
        .await
        .unwrap();

    // Another caller queued work that this scoped scheduler did not create; it
    // must leave that root alone so the two runs cannot race for it.
    let foreign = AssignmentScheduler::new(
        store.clone(),
        EventBus::default_bus(),
        workspace_id.clone(),
        2,
    );
    let foreign_task = Task::new("Foreign work", "Output").assign_to(foreign_agent.id.clone());
    foreign
        .enqueue(
            &foreign_task,
            requester(),
            AssignmentOrigin::User,
            &options(2),
        )
        .await
        .unwrap();

    let completed = owners.run_until_idle().await.unwrap();
    assert_eq!(completed.len(), 1);
    assert!(completed.contains_key(&own_task.id));

    let untouched = store
        .list_assignments_for_task(&foreign_task.id)
        .await
        .unwrap();
    assert!(untouched
        .iter()
        .all(|assignment| assignment.status == AssignmentStatus::Queued));
}

#[tokio::test]
async fn plan_mode_stores_the_output_as_a_task_plan() {
    let (store, workspace_id) = setup().await;
    let planner = StandardRoles::planner();
    let planner_id = planner.id.clone();
    let mut scheduler =
        AssignmentScheduler::new(store.clone(), EventBus::default_bus(), workspace_id, 1);
    scheduler.register_agent(
        planner,
        Arc::new(MockLlmProvider::echo()),
        ToolRegistry::new(),
        "mock-model",
    );

    let task = Task::new("Design the change", "A plan").assign_to(planner_id);
    let mut run_options = options(1);
    run_options.approval_level = ApprovalLevel::PlanOnly;
    scheduler
        .enqueue(&task, requester(), AssignmentOrigin::User, &run_options)
        .await
        .unwrap();

    let completed = scheduler.run_until_idle().await.unwrap();
    let output = completed.get(&task.id).expect("plan run output");
    let stored = store.get_task(&task.id).await.unwrap().unwrap();
    assert_eq!(stored.plan.as_deref(), Some(output.as_str()));
    assert!(!stored.plan_approved);
    assert!(matches!(stored.status, TaskStatus::Completed { .. }));
}
