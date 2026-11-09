use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cowbot_agent::{AgentInstance, Crew, StandardRoles};
use cowbot_bus::EventBus;
use cowbot_core::approval::{ApprovalLevel, ApprovalStatus};
use cowbot_core::assignment::{
    AssignmentEventKind, AssignmentOrigin, AssignmentRequester, AssignmentStatus,
};
use cowbot_core::error::CowError;
use cowbot_core::run::RunStatus;
use cowbot_core::task::Task;
use cowbot_core::tool::{Tool, ToolCall, ToolResult};
use cowbot_core::usage::{LlmCallKind, TokenUsage};
use cowbot_llm::{ChatCompletionResponse, LlmToolCall, MockLlmProvider};
use cowbot_service::{bootstrap_local, ApprovalService, CrewRunner};
use cowbot_store::Store;
use cowbot_tools::ToolRegistry;
use serde_json::json;

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo_tool"
    }

    fn description(&self) -> &str {
        "Echoes a value"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, CowError> {
        Ok(ToolResult::success(call.id.clone(), "echoed"))
    }
}

#[tokio::test]
async fn crew_runner_executes_tasks_through_assignments() {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let context = bootstrap_local(&store).await.unwrap();
    let bus = EventBus::default_bus();
    let runner = CrewRunner::new(store.clone(), bus);

    let llm = Arc::new(MockLlmProvider::echo());
    let tools = ToolRegistry::new();

    let planner_profile = StandardRoles::planner();
    let planner_id = planner_profile.id.clone();
    let coder_profile = StandardRoles::coder();
    let coder_id = coder_profile.id.clone();

    let mut crew = Crew::new();
    crew.add_agent(AgentInstance::new(
        planner_profile,
        llm.clone(),
        tools.clone(),
        "mock-model",
    ));
    crew.add_agent(AgentInstance::new(coder_profile, llm, tools, "mock-model"));

    let plan_task = Task::new("Plan the change", "An implementation plan").assign_to(planner_id);
    let plan_id = plan_task.id.clone();
    let code_task = Task::new("Implement the change", "Working code")
        .assign_to(coder_id)
        .with_context_task(plan_id.clone());
    let code_id = code_task.id.clone();

    crew.add_task(plan_task);
    crew.add_task(code_task);

    let results = runner
        .run(
            &mut crew,
            &context.workspace.id,
            AssignmentRequester::User(context.user.id.0.clone()),
            AssignmentOrigin::User,
            ApprovalLevel::Ask,
        )
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert!(results.contains_key(&plan_id));
    assert!(results.contains_key(&code_id));

    for task_id in [&plan_id, &code_id] {
        let assignments = store.list_assignments_for_task(task_id).await.unwrap();
        assert_eq!(assignments.len(), 1);
        let assignment = &assignments[0];
        assert_eq!(assignment.status, AssignmentStatus::Completed);
        assert_eq!(assignment.executions, 1);

        let events = store.list_assignment_events(&assignment.id).await.unwrap();
        let kinds: Vec<_> = events.iter().map(|event| event.kind).collect();
        assert_eq!(
            kinds,
            vec![
                AssignmentEventKind::Enqueued,
                AssignmentEventKind::Started,
                AssignmentEventKind::Completed,
            ]
        );

        let runs = store.list_runs(task_id).await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, RunStatus::Completed);
        assert!(runs[0].finished_at.is_some());

        let expected_agent = crew
            .tasks
            .iter()
            .find(|task| task.id == *task_id)
            .and_then(|task| task.assigned_agent.clone())
            .unwrap();
        assert_eq!(runs[0].agent_id, expected_agent);
    }
}

#[tokio::test]
async fn failed_task_leaves_a_failed_run() {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let context = bootstrap_local(&store).await.unwrap();
    let bus = EventBus::default_bus();
    let runner = CrewRunner::new(store.clone(), bus);

    let mut profile = StandardRoles::coder();
    profile.max_iterations = 1;
    let agent_id = profile.id.clone();
    let llm = Arc::new(MockLlmProvider::new(vec![ChatCompletionResponse {
        content: Some("calling a tool".to_string()),
        tool_calls: vec![LlmToolCall {
            id: "call-1".to_string(),
            name: "execute_code".to_string(),
            arguments: json!({ "command": "echo hi" }),
        }],
        finish_reason: Some("tool_calls".to_string()),
        usage: Some(TokenUsage {
            uncached_input_tokens: 321,
            output_tokens: 12,
            ..TokenUsage::default()
        }),
        raw_usage: None,
    }]));

    let mut crew = Crew::new();
    crew.add_agent(AgentInstance::new(
        profile,
        llm,
        ToolRegistry::new(),
        "mock-model",
    ));
    let task = Task::new("Exhaust the iteration limit", "A failure").assign_to(agent_id);
    let task_id = task.id.clone();
    crew.add_task(task);

    let result = runner
        .run(
            &mut crew,
            &context.workspace.id,
            AssignmentRequester::User(context.user.id.0.clone()),
            AssignmentOrigin::User,
            ApprovalLevel::Trusted,
        )
        .await;
    assert!(result.is_err());

    let runs = store.list_runs(&task_id).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Failed);
    assert!(runs[0].finished_at.is_some());

    let calls = store.list_llm_calls_for_run(&runs[0].id).await.unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].call_kind, LlmCallKind::Agent);
    assert_eq!(calls[0].usage.as_ref().unwrap().uncached_input_tokens, 321);

    let assignments = store.list_assignments_for_task(&task_id).await.unwrap();
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].status, AssignmentStatus::Failed);
}

#[tokio::test]
async fn crew_runner_records_usage_for_a_completed_task() {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let context = bootstrap_local(&store).await.unwrap();
    let bus = EventBus::default_bus();
    let runner = CrewRunner::new(store.clone(), bus);

    let profile = StandardRoles::coder();
    let agent_id = profile.id.clone();
    let llm = Arc::new(MockLlmProvider::echo());

    let mut crew = Crew::new();
    crew.add_agent(AgentInstance::new(
        profile,
        llm,
        ToolRegistry::new(),
        "mock-model",
    ));
    let task = Task::new("Write a summary", "Summary").assign_to(agent_id.clone());
    let task_id = task.id.clone();
    crew.add_task(task);

    runner
        .run(
            &mut crew,
            &context.workspace.id,
            AssignmentRequester::User(context.user.id.0.clone()),
            AssignmentOrigin::User,
            ApprovalLevel::Trusted,
        )
        .await
        .unwrap();

    let runs = store.list_runs(&task_id).await.unwrap();
    assert_eq!(runs.len(), 1);
    let calls = store.list_llm_calls_for_run(&runs[0].id).await.unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].agent_id, Some(agent_id));
    assert_eq!(calls[0].model, "mock-model");
    assert!(calls[0].usage.is_some());

    let totals = store.sum_llm_usage_for_task(&task_id).await.unwrap();
    assert_eq!(totals.calls, 1);
    assert!(totals.total_input_tokens() > 0);
    assert!(totals.output_tokens > 0);
}

#[tokio::test]
async fn crew_runner_records_tool_calls() {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let context = bootstrap_local(&store).await.unwrap();
    let bus = EventBus::default_bus();
    let runner = CrewRunner::new(store.clone(), bus);

    let responses = vec![
        ChatCompletionResponse {
            content: Some("calling the tool".to_string()),
            tool_calls: vec![LlmToolCall {
                id: "call-1".to_string(),
                name: "echo_tool".to_string(),
                arguments: json!({ "value": "hi" }),
            }],
            finish_reason: Some("tool_calls".to_string()),
            usage: None,
            raw_usage: None,
        },
        ChatCompletionResponse {
            content: Some("finished".to_string()),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_string()),
            usage: None,
            raw_usage: None,
        },
    ];
    let llm = Arc::new(MockLlmProvider::new(responses));
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(EchoTool));

    let profile = StandardRoles::coder();
    let agent_id = profile.id.clone();
    let mut crew = Crew::new();
    crew.add_agent(AgentInstance::new(profile, llm, tools, "mock-model"));

    let task = Task::new("Call the echo tool", "Echoed output").assign_to(agent_id);
    let task_id = task.id.clone();
    crew.add_task(task);

    let results = runner
        .run(
            &mut crew,
            &context.workspace.id,
            AssignmentRequester::User(context.user.id.0.clone()),
            AssignmentOrigin::User,
            ApprovalLevel::Trusted,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1);

    let runs = store.list_runs(&task_id).await.unwrap();
    assert_eq!(runs.len(), 1);
    let calls = store.list_tool_calls(&runs[0].id).await.unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call-1");
    assert_eq!(calls[0].name, "echo_tool");
    assert_eq!(calls[0].arguments, json!({ "value": "hi" }));
    assert_eq!(calls[0].approved_by, None);

    let result = calls[0].result.as_ref().unwrap();
    assert_eq!(result["success"], json!(true));
    assert_eq!(result["output"], json!("echoed"));
}

#[tokio::test]
async fn crew_runner_records_the_human_approver() {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let context = bootstrap_local(&store).await.unwrap();
    let bus = EventBus::default_bus();
    let runner = CrewRunner::new(store.clone(), bus.clone());

    let responses = vec![
        ChatCompletionResponse {
            content: Some("calling the tool".to_string()),
            tool_calls: vec![LlmToolCall {
                id: "call-approved".to_string(),
                name: "echo_tool".to_string(),
                arguments: json!({ "value": "hi" }),
            }],
            finish_reason: Some("tool_calls".to_string()),
            usage: None,
            raw_usage: None,
        },
        ChatCompletionResponse {
            content: Some("finished".to_string()),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_string()),
            usage: None,
            raw_usage: None,
        },
    ];
    let llm = Arc::new(MockLlmProvider::new(responses));
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(EchoTool));

    let profile = StandardRoles::coder();
    let agent_id = profile.id.clone();
    let mut crew = Crew::new();
    crew.add_agent(AgentInstance::new(profile, llm, tools, "mock-model"));

    let task = Task::new("Approve the echo tool", "Echoed output").assign_to(agent_id);
    let task_id = task.id.clone();
    crew.add_task(task);

    let decider = ApprovalService::new(store.clone(), bus, context.workspace.id.clone());
    let decide_store = store.clone();
    let decide_task_id = task_id.clone();
    tokio::spawn(async move {
        for _ in 0..100 {
            let pending = decide_store
                .list_pending_approvals(&decide_task_id)
                .await
                .unwrap();
            if let Some(approval) = pending.first() {
                decider
                    .decide(&approval.id, ApprovalStatus::Approved, "user-1")
                    .await
                    .unwrap();
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("no pending approval appeared");
    });

    let results = tokio::time::timeout(
        Duration::from_secs(5),
        runner.run(
            &mut crew,
            &context.workspace.id,
            AssignmentRequester::User(context.user.id.0.clone()),
            AssignmentOrigin::User,
            ApprovalLevel::Ask,
        ),
    )
    .await
    .expect("crew runner timed out")
    .unwrap();
    assert_eq!(results.len(), 1);

    let runs = store.list_runs(&task_id).await.unwrap();
    assert_eq!(runs.len(), 1);
    let calls = store.list_tool_calls(&runs[0].id).await.unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call-approved");
    assert_eq!(calls[0].approved_by.as_deref(), Some("user-1"));
    assert_eq!(calls[0].result.as_ref().unwrap()["success"], json!(true));
}
