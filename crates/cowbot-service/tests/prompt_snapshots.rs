//! Prompt snapshot capture across the call kinds the service records
//! (ADR-0037).

use std::sync::Arc;

use cowbot_agent::StandardRoles;
use cowbot_bus::EventBus;
use cowbot_core::agent::AgentProfile;
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::assignment::{AssignmentOrigin, AssignmentRequester};
use cowbot_core::channel::{Channel, ChannelMessage, Sender};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::prompt::PromptSlotName;
use cowbot_core::prompt_snapshot::PromptSnapshotSettings;
use cowbot_core::task::Task;
use cowbot_core::usage::LlmCallKind;
use cowbot_llm::MockLlmProvider;
use cowbot_service::{
    bootstrap_local, AssignmentScheduler, ChannelService, ChannelTurnOptions, ConversationService,
    CrewRunOptions,
};
use cowbot_store::Store;
use cowbot_tools::ToolRegistry;

async fn setup() -> (Store, WorkspaceId, AgentProfile) {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let context = bootstrap_local(&store).await.unwrap();
    let agent = StandardRoles::coder();
    store
        .upsert_agent(&context.workspace.id, &agent)
        .await
        .unwrap();
    (store, context.workspace.id, agent)
}

fn turn_options() -> ChannelTurnOptions {
    ChannelTurnOptions {
        approval_level: ApprovalLevel::Trusted,
        max_concurrency: 2,
        use_model_permissions: false,
        platform_context: None,
        participant_capacity: Vec::new(),
    }
}

fn user_message(channel: &Channel, content: &str) -> ChannelMessage {
    ChannelMessage::new(
        channel.id.clone(),
        Sender::User("user-1".to_string()),
        content,
    )
}

async fn conversation(
    store: &Store,
    workspace_id: &WorkspaceId,
    agent: &AgentProfile,
) -> (ConversationService, ChannelService, Channel) {
    let bus = EventBus::default_bus();
    let service = ConversationService::new(store.clone(), bus.clone(), workspace_id.clone());
    let channels = ChannelService::new(store.clone(), bus);
    let channel = service
        .create(
            Some("Snapshot review"),
            &agent.id,
            Some("provider-1".to_string()),
            "mock-model",
        )
        .await
        .unwrap();
    (service, channels, channel)
}

#[tokio::test]
async fn a_conversation_turn_records_a_sealed_snapshot_with_slots() {
    let (store, workspace_id, agent) = setup().await;
    let (service, _channels, channel) = conversation(&store, &workspace_id, &agent).await;

    service
        .reply(
            &channel.id,
            &user_message(&channel, "Explain the change"),
            Arc::new(MockLlmProvider::echo()),
            ToolRegistry::new(),
            &turn_options(),
        )
        .await
        .unwrap();

    let calls = store.list_llm_calls_for_channel(&channel.id).await.unwrap();
    assert_eq!(calls.len(), 1, "one agent call is recorded");
    assert_eq!(calls[0].call_kind, LlmCallKind::Agent);

    let detail = store
        .get_prompt_snapshot_detail(&calls[0].id)
        .await
        .unwrap()
        .expect("the call has a snapshot");
    assert_eq!(detail.summary.channel_id, Some(channel.id.clone()));
    assert_eq!(detail.summary.call_kind, LlmCallKind::Agent);
    assert_eq!(detail.summary.model, calls[0].model);
    assert!(!detail.hash_mismatch, "the stored document is intact");
    assert!(!detail.summary.truncated);

    let snapshot = &detail.snapshot;
    assert!(snapshot.hash_matches());
    assert!(!snapshot.content_hash.is_empty());
    assert!(
        snapshot
            .slots
            .iter()
            .any(|slot| slot.name == PromptSlotName::Agent),
        "the agent slot is recorded beside the finished messages"
    );
    assert!(
        snapshot
            .messages
            .iter()
            .any(|message| message.content.contains("Explain the change")),
        "the user message is part of the captured request"
    );

    // The summary is reachable from the run and the audit view.
    let by_channel = store
        .list_prompt_snapshots_for_channel(&workspace_id, &channel.id, 10)
        .await
        .unwrap();
    assert_eq!(by_channel.len(), 1);
    let recent = store
        .list_recent_prompt_snapshots(&workspace_id, 10)
        .await
        .unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].llm_call_id, calls[0].id);
}

#[tokio::test]
async fn a_workspace_with_snapshots_off_keeps_usage_and_records_none() {
    let (store, workspace_id, agent) = setup().await;
    let mut settings = PromptSnapshotSettings::new(workspace_id.clone());
    settings.enabled = false;
    store
        .save_prompt_snapshot_settings(&settings)
        .await
        .unwrap();

    let (service, _channels, channel) = conversation(&store, &workspace_id, &agent).await;
    service
        .reply(
            &channel.id,
            &user_message(&channel, "Explain the change"),
            Arc::new(MockLlmProvider::echo()),
            ToolRegistry::new(),
            &turn_options(),
        )
        .await
        .unwrap();

    assert_eq!(
        store
            .list_llm_calls_for_channel(&channel.id)
            .await
            .unwrap()
            .len(),
        1,
        "usage is recorded whether or not snapshots are"
    );
    assert!(store
        .list_recent_prompt_snapshots(&workspace_id, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_task_run_records_a_snapshot_linked_to_its_run() {
    let (store, workspace_id, agent) = setup().await;
    let bus = EventBus::default_bus();
    let mut scheduler = AssignmentScheduler::new(store.clone(), bus, workspace_id.clone(), 1);
    scheduler.register_agent(
        agent.clone(),
        Arc::new(MockLlmProvider::echo()),
        ToolRegistry::new(),
        "mock-model",
    );

    let task = Task::new("Summarise the migration", "A short summary").assign_to(agent.id.clone());
    scheduler
        .enqueue(
            &task,
            AssignmentRequester::User("user-1".to_string()),
            AssignmentOrigin::User,
            &CrewRunOptions {
                approval_level: ApprovalLevel::Trusted,
                channel_id: None,
                max_concurrency: 1,
                use_model_permissions: false,
                capture_plan_task: None,
            },
        )
        .await
        .unwrap();
    scheduler.run_until_idle().await.unwrap();

    let runs = store.list_runs(&task.id).await.unwrap();
    assert_eq!(runs.len(), 1);
    let summaries = store
        .list_prompt_snapshots_for_run(&runs[0].id)
        .await
        .unwrap();
    assert!(!summaries.is_empty(), "every agent iteration is captured");
    assert_eq!(summaries[0].task_id, Some(task.id.clone()));
    assert_eq!(summaries[0].agent_id, Some(agent.id.clone()));

    let detail = store
        .get_prompt_snapshot_detail(&summaries[0].llm_call_id)
        .await
        .unwrap()
        .unwrap();
    assert!(detail.snapshot.hash_matches());
    assert!(
        detail
            .snapshot
            .messages
            .iter()
            .any(|message| message.content.contains("Summarise the migration")),
        "the task prompt is part of the captured request"
    );
    assert!(
        detail
            .snapshot
            .messages
            .iter()
            .any(|message| message.role == cowbot_core::prompt::PromptRole::System),
        "the agent system prompt is captured"
    );
}

#[tokio::test]
async fn a_retention_sweep_keeps_recent_snapshots_and_usage() {
    let (store, workspace_id, agent) = setup().await;
    let (service, _channels, channel) = conversation(&store, &workspace_id, &agent).await;
    service
        .reply(
            &channel.id,
            &user_message(&channel, "Explain the change"),
            Arc::new(MockLlmProvider::echo()),
            ToolRegistry::new(),
            &turn_options(),
        )
        .await
        .unwrap();

    let snapshots = PromptSnapshotSettings::new(workspace_id.clone());
    let mut retention = snapshots;
    retention.set_retention_days(1);
    store
        .save_prompt_snapshot_settings(&retention)
        .await
        .unwrap();

    // Everything is younger than a day, so the sweep keeps it.
    let snapshots_service =
        cowbot_service::PromptSnapshotService::new(store.clone(), workspace_id.clone());
    assert_eq!(snapshots_service.sweep().await.unwrap(), 0);
    assert_eq!(
        store
            .list_llm_calls_for_channel(&channel.id)
            .await
            .unwrap()
            .len(),
        1
    );
}
