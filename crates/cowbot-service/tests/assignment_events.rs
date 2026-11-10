use cowbot_bus::EventBus;
use cowbot_core::agent::AgentId;
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::assignment::{
    AssignmentEventKind, AssignmentOrigin, AssignmentRequester, AssignmentStatus,
};
use cowbot_core::event::EventPayload;
use cowbot_core::identity::WorkspaceId;
use cowbot_core::task::Task;
use cowbot_service::{bootstrap_local, AssignmentService};
use cowbot_store::Store;

async fn setup() -> (Store, EventBus, AssignmentService, WorkspaceId, AgentId) {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let context = bootstrap_local(&store).await.unwrap();
    let bus = EventBus::default_bus();
    let service = AssignmentService::new(store.clone(), bus.clone());
    (
        store,
        bus,
        service,
        context.workspace.id,
        AgentId::new("agent-1"),
    )
}

async fn make_task(
    store: &Store,
    workspace_id: &WorkspaceId,
    agent: &AgentId,
    description: &str,
) -> Task {
    let task = Task::new(description, "A result").assign_to(agent.clone());
    store.create_task(workspace_id, None, &task).await.unwrap();
    task
}

fn assignment_events(bus: &EventBus, task: &Task) -> Vec<(AssignmentStatus, AssignmentEventKind)> {
    bus.history()
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::AssignmentUpdated {
                task_id,
                status,
                kind,
                ..
            } if task_id == &task.id => Some((*status, *kind)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn assignment_transitions_publish_events() {
    let (store, bus, service, workspace_id, agent) = setup().await;
    let task = make_task(&store, &workspace_id, &agent, "Implement login").await;

    let assignment = service
        .enqueue_root(
            &task,
            AssignmentOrigin::User,
            AssignmentRequester::User("user-1".to_string()),
            agent,
            ApprovalLevel::Ask,
        )
        .await
        .unwrap();
    service.start(&assignment.id).await.unwrap();
    service.mark_waiting(&assignment.id).await.unwrap();
    service.resume(&assignment.id).await.unwrap();
    service.start(&assignment.id).await.unwrap();
    service
        .complete(&assignment.id, "Login implemented")
        .await
        .unwrap();

    assert_eq!(
        assignment_events(&bus, &task),
        vec![
            (AssignmentStatus::Queued, AssignmentEventKind::Enqueued),
            (AssignmentStatus::Running, AssignmentEventKind::Started),
            (AssignmentStatus::Waiting, AssignmentEventKind::Waiting),
            (AssignmentStatus::Resume, AssignmentEventKind::Resumed),
            (AssignmentStatus::Running, AssignmentEventKind::Started),
            (AssignmentStatus::Completed, AssignmentEventKind::Completed),
        ]
    );
}

#[tokio::test]
async fn failure_cancellation_and_reconcile_publish_events() {
    let (store, bus, service, workspace_id, agent) = setup().await;

    let failed_task = make_task(&store, &workspace_id, &agent, "Fail").await;
    let failed = service
        .enqueue_root(
            &failed_task,
            AssignmentOrigin::User,
            AssignmentRequester::System,
            agent.clone(),
            ApprovalLevel::Ask,
        )
        .await
        .unwrap();
    service.start(&failed.id).await.unwrap();
    service.fail(&failed.id, "boom").await.unwrap();
    assert_eq!(
        assignment_events(&bus, &failed_task).last(),
        Some(&(AssignmentStatus::Failed, AssignmentEventKind::Failed))
    );

    let cancelled_task = make_task(&store, &workspace_id, &agent, "Cancel").await;
    let cancelled = service
        .enqueue_root(
            &cancelled_task,
            AssignmentOrigin::User,
            AssignmentRequester::System,
            agent.clone(),
            ApprovalLevel::Ask,
        )
        .await
        .unwrap();
    service.start(&cancelled.id).await.unwrap();
    service
        .cancel(&cancelled.id, "no longer needed")
        .await
        .unwrap();
    assert_eq!(
        assignment_events(&bus, &cancelled_task).last(),
        Some(&(AssignmentStatus::Cancelled, AssignmentEventKind::Cancelled))
    );

    let reconciled_task = make_task(&store, &workspace_id, &agent, "Reconcile").await;
    let reconciled = service
        .enqueue_root(
            &reconciled_task,
            AssignmentOrigin::User,
            AssignmentRequester::System,
            agent,
            ApprovalLevel::Ask,
        )
        .await
        .unwrap();
    service.start(&reconciled.id).await.unwrap();
    service.reconcile_on_startup().await.unwrap();
    assert_eq!(
        assignment_events(&bus, &reconciled_task).last(),
        Some(&(AssignmentStatus::Queued, AssignmentEventKind::Restarted))
    );
}
