use chrono::{Duration, Utc};
use cowbot_core::agent::AgentId;
use cowbot_core::channel::Channel;
use cowbot_core::identity::{User, Workspace};
use cowbot_core::prompt::PromptRole;
use cowbot_core::prompt_snapshot::{
    LlmPromptSnapshot, PromptSnapshotSettings, SnapshotMessage,
    DEFAULT_PROMPT_SNAPSHOT_RETENTION_DAYS,
};
use cowbot_core::run::Run;
use cowbot_core::task::Task;
use cowbot_core::usage::{LlmCallKind, LlmCallRecord, ESTIMATE_COUNTER};
use cowbot_store::Store;

async fn test_store() -> Store {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    store
}

async fn seed_workspace(store: &Store) -> Workspace {
    let user = User::new("Alice", "alice@example.com", "argon2-hash");
    let workspace = Workspace::new("Snapshot Lab");
    store.create_user(&user).await.unwrap();
    store.create_workspace(&workspace).await.unwrap();
    workspace
}

fn snapshot(kind: LlmCallKind, content: &str) -> LlmPromptSnapshot {
    LlmPromptSnapshot::new(kind, "mock-model")
        .with_messages(vec![
            SnapshotMessage::new(PromptRole::System, "You are CowBot."),
            SnapshotMessage::new(PromptRole::User, content),
        ])
        .seal(&ESTIMATE_COUNTER)
}

fn call(
    run: &Run,
    task: &Task,
    agent_id: &AgentId,
    created_at: chrono::DateTime<Utc>,
) -> LlmCallRecord {
    let mut record = LlmCallRecord::new(LlmCallKind::Agent, "mock-model");
    record.run_id = Some(run.id.clone());
    record.task_id = Some(task.id.clone());
    record.agent_id = Some(agent_id.clone());
    record.created_at = created_at;
    record
}

#[tokio::test]
async fn a_snapshot_round_trips_and_joins_its_call() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let agent_id = AgentId::new("agent-coder");
    let task = Task::new("Write a summary", "Summary").assign_to(agent_id.clone());
    store.create_task(&workspace.id, None, &task).await.unwrap();
    let run = Run::new(task.id.clone(), agent_id.clone());
    store.create_run(&run).await.unwrap();

    let call = call(&run, &task, &agent_id, Utc::now());
    store.record_llm_call(&call).await.unwrap();
    let sealed = snapshot(LlmCallKind::Agent, "Summarise the task.");
    store
        .record_prompt_snapshot(&workspace.id, &call.id, &sealed)
        .await
        .unwrap();

    let loaded = store
        .get_prompt_snapshot(&call.id)
        .await
        .unwrap()
        .expect("the snapshot is stored");
    assert_eq!(loaded, sealed);
    assert!(loaded.hash_matches());

    let summaries = store.list_prompt_snapshots_for_run(&run.id).await.unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].llm_call_id, call.id);
    assert_eq!(summaries[0].run_id, Some(run.id.clone()));
    assert_eq!(summaries[0].task_id, Some(task.id.clone()));
    assert_eq!(summaries[0].agent_id, Some(agent_id.clone()));
    assert_eq!(summaries[0].call_kind, LlmCallKind::Agent);
    assert_eq!(summaries[0].model, "mock-model");
    assert_eq!(summaries[0].token_count, sealed.token_count);
    assert!(!summaries[0].truncated);
    assert!(!summaries[0].redacted);

    let by_task = store
        .list_prompt_snapshots_for_task(&task.id)
        .await
        .unwrap();
    assert_eq!(by_task.len(), 1);
    let recent = store
        .list_recent_prompt_snapshots(&workspace.id, 10)
        .await
        .unwrap();
    assert_eq!(recent.len(), 1);
    assert!(store
        .list_recent_prompt_snapshots(&workspace.id, 10)
        .await
        .unwrap()
        .first()
        .is_some_and(|entry| entry.byte_size > 0));
}

#[tokio::test]
async fn a_channel_lists_its_snapshots_newest_first() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let channel = Channel::new("Team", "Topic");
    store.create_channel(&workspace.id, &channel).await.unwrap();

    let mut first = LlmCallRecord::new(LlmCallKind::Router, "mock-model");
    first.channel_id = Some(channel.id.clone());
    first.created_at = Utc::now() - Duration::seconds(5);
    let mut second = LlmCallRecord::new(LlmCallKind::Compact, "mock-model");
    second.channel_id = Some(channel.id.clone());
    store.record_llm_call(&first).await.unwrap();
    store.record_llm_call(&second).await.unwrap();
    store
        .record_prompt_snapshot(
            &workspace.id,
            &first.id,
            &snapshot(LlmCallKind::Router, "Who should answer?"),
        )
        .await
        .unwrap();
    store
        .record_prompt_snapshot(
            &workspace.id,
            &second.id,
            &snapshot(LlmCallKind::Compact, "Summarise."),
        )
        .await
        .unwrap();

    let listed = store
        .list_prompt_snapshots_for_channel(&workspace.id, &channel.id, 10)
        .await
        .unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].llm_call_id, second.id);
    assert_eq!(listed[1].llm_call_id, first.id);
}

#[tokio::test]
async fn recording_a_call_twice_replaces_its_snapshot() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let agent_id = AgentId::new("agent-coder");
    let task = Task::new("Write a summary", "Summary").assign_to(agent_id.clone());
    store.create_task(&workspace.id, None, &task).await.unwrap();
    let run = Run::new(task.id.clone(), agent_id.clone());
    store.create_run(&run).await.unwrap();
    let call = call(&run, &task, &agent_id, Utc::now());
    store.record_llm_call(&call).await.unwrap();

    store
        .record_prompt_snapshot(
            &workspace.id,
            &call.id,
            &snapshot(LlmCallKind::Agent, "first"),
        )
        .await
        .unwrap();
    store
        .record_prompt_snapshot(
            &workspace.id,
            &call.id,
            &snapshot(LlmCallKind::Agent, "second"),
        )
        .await
        .unwrap();

    let loaded = store.get_prompt_snapshot(&call.id).await.unwrap().unwrap();
    assert!(loaded.messages[1].content.contains("second"));
    assert_eq!(
        store
            .list_prompt_snapshots_for_run(&run.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn retention_removes_old_snapshots_and_keeps_usage_rows() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let agent_id = AgentId::new("agent-coder");
    let task = Task::new("Write a summary", "Summary").assign_to(agent_id.clone());
    store.create_task(&workspace.id, None, &task).await.unwrap();
    let run = Run::new(task.id.clone(), agent_id.clone());
    store.create_run(&run).await.unwrap();

    let old = call(&run, &task, &agent_id, Utc::now() - Duration::days(40));
    let recent = call(&run, &task, &agent_id, Utc::now());
    store.record_llm_call(&old).await.unwrap();
    store.record_llm_call(&recent).await.unwrap();
    let mut old_snapshot = LlmPromptSnapshot::new(LlmCallKind::Agent, "mock-model")
        .with_messages(vec![SnapshotMessage::new(PromptRole::User, "old")]);
    old_snapshot.created_at = old.created_at;
    let old_snapshot = old_snapshot.seal(&ESTIMATE_COUNTER);
    let mut recent_snapshot = LlmPromptSnapshot::new(LlmCallKind::Agent, "mock-model")
        .with_messages(vec![SnapshotMessage::new(PromptRole::User, "recent")]);
    recent_snapshot.created_at = recent.created_at;
    let recent_snapshot = recent_snapshot.seal(&ESTIMATE_COUNTER);
    store
        .record_prompt_snapshot(&workspace.id, &old.id, &old_snapshot)
        .await
        .unwrap();
    store
        .record_prompt_snapshot(&workspace.id, &recent.id, &recent_snapshot)
        .await
        .unwrap();

    let deleted = store
        .delete_prompt_snapshots_before(&workspace.id, Utc::now() - Duration::days(30))
        .await
        .unwrap();
    assert_eq!(deleted, 1);
    assert!(store.get_prompt_snapshot(&old.id).await.unwrap().is_none());
    assert!(store
        .get_prompt_snapshot(&recent.id)
        .await
        .unwrap()
        .is_some());

    // Usage rows outlive their snapshots.
    let calls = store.list_llm_calls_for_run(&run.id).await.unwrap();
    assert_eq!(calls.len(), 2);
}

#[tokio::test]
async fn deleting_a_run_cascades_to_its_snapshots() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let agent_id = AgentId::new("agent-coder");
    let task = Task::new("Write a summary", "Summary").assign_to(agent_id.clone());
    store.create_task(&workspace.id, None, &task).await.unwrap();
    let run = Run::new(task.id.clone(), agent_id.clone());
    store.create_run(&run).await.unwrap();
    let call = call(&run, &task, &agent_id, Utc::now());
    store.record_llm_call(&call).await.unwrap();
    store
        .record_prompt_snapshot(
            &workspace.id,
            &call.id,
            &snapshot(LlmCallKind::Agent, "hello"),
        )
        .await
        .unwrap();

    sqlx::query("DELETE FROM runs WHERE id = ?")
        .bind(&run.id.0)
        .execute(store.pool())
        .await
        .unwrap();

    assert!(store.get_prompt_snapshot(&call.id).await.unwrap().is_none());
    assert!(store
        .list_recent_prompt_snapshots(&workspace.id, 10)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .list_llm_calls_for_run(&run.id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn unknown_calls_have_no_snapshot() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    assert!(store
        .get_prompt_snapshot("missing-call")
        .await
        .unwrap()
        .is_none());
    assert!(store
        .list_recent_prompt_snapshots(&workspace.id, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_detail_read_carries_the_call_links_and_an_intact_hash() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let agent_id = AgentId::new("agent-coder");
    let task = Task::new("Write a summary", "Summary").assign_to(agent_id.clone());
    store.create_task(&workspace.id, None, &task).await.unwrap();
    let run = Run::new(task.id.clone(), agent_id.clone());
    store.create_run(&run).await.unwrap();
    let call = call(&run, &task, &agent_id, Utc::now());
    store.record_llm_call(&call).await.unwrap();
    let sealed = snapshot(LlmCallKind::Agent, "hello");
    store
        .record_prompt_snapshot(&workspace.id, &call.id, &sealed)
        .await
        .unwrap();

    let detail = store
        .get_prompt_snapshot_detail(&call.id)
        .await
        .unwrap()
        .expect("the detail is stored");
    assert_eq!(detail.summary.run_id, Some(run.id.clone()));
    assert_eq!(detail.summary.task_id, Some(task.id.clone()));
    assert_eq!(detail.summary.agent_id, Some(agent_id.clone()));
    assert_eq!(detail.summary.call_kind, LlmCallKind::Agent);
    assert!(!detail.hash_mismatch);
    assert_eq!(detail.snapshot, sealed);
}

#[tokio::test]
async fn deleting_a_channel_removes_only_its_snapshots() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let first_channel = Channel::new("First", "Topic");
    let second_channel = Channel::new("Second", "Topic");
    store
        .create_channel(&workspace.id, &first_channel)
        .await
        .unwrap();
    store
        .create_channel(&workspace.id, &second_channel)
        .await
        .unwrap();

    let mut first_call = LlmCallRecord::new(LlmCallKind::Router, "mock-model");
    first_call.channel_id = Some(first_channel.id.clone());
    let mut second_call = LlmCallRecord::new(LlmCallKind::Router, "mock-model");
    second_call.channel_id = Some(second_channel.id.clone());
    store.record_llm_call(&first_call).await.unwrap();
    store.record_llm_call(&second_call).await.unwrap();
    store
        .record_prompt_snapshot(
            &workspace.id,
            &first_call.id,
            &snapshot(LlmCallKind::Router, "first"),
        )
        .await
        .unwrap();
    store
        .record_prompt_snapshot(
            &workspace.id,
            &second_call.id,
            &snapshot(LlmCallKind::Router, "second"),
        )
        .await
        .unwrap();

    let removed = store
        .delete_prompt_snapshots_for_channel(&first_channel.id)
        .await
        .unwrap();
    assert_eq!(removed, 1);
    assert!(store
        .get_prompt_snapshot(&first_call.id)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get_prompt_snapshot(&second_call.id)
        .await
        .unwrap()
        .is_some());
    // The channel's usage rows stay.
    assert_eq!(
        store
            .list_llm_calls_for_channel(&first_channel.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn settings_default_on_first_use_and_persist() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;

    let defaults = store
        .get_prompt_snapshot_settings(&workspace.id)
        .await
        .unwrap();
    assert!(defaults.enabled);
    assert_eq!(
        defaults.retention_days,
        DEFAULT_PROMPT_SNAPSHOT_RETENTION_DAYS
    );

    let mut settings = PromptSnapshotSettings::new(workspace.id.clone());
    settings.enabled = false;
    settings.set_retention_days(90);
    store
        .save_prompt_snapshot_settings(&settings)
        .await
        .unwrap();

    let loaded = store
        .get_prompt_snapshot_settings(&workspace.id)
        .await
        .unwrap();
    assert!(!loaded.enabled);
    assert_eq!(loaded.retention_days, 90);
    assert_eq!(loaded.workspace_id, workspace.id);

    // Out-of-range values are clamped on write.
    let mut clamped = PromptSnapshotSettings::new(workspace.id.clone());
    clamped.retention_days = 0;
    store.save_prompt_snapshot_settings(&clamped).await.unwrap();
    let loaded = store
        .get_prompt_snapshot_settings(&workspace.id)
        .await
        .unwrap();
    assert_eq!(loaded.retention_days, 1);
}
