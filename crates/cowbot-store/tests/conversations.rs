use std::fs;
use std::path::Path;

use chrono::Utc;
use cowbot_core::agent::AgentId;
use cowbot_core::channel::{Channel, ChannelId};
use cowbot_core::conversation::{
    ChannelKind, ContextEntry, ContextEvent, ContextEventKind, ContextRole, ConversationSettings,
};
use cowbot_core::identity::{User, Workspace};
use cowbot_core::run::Run;
use cowbot_core::task::Task;
use cowbot_core::usage::{LlmCallKind, LlmCallRecord, TokenUsage};
use cowbot_store::Store;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

async fn test_store() -> Store {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    store
}

async fn seed_workspace(store: &Store) -> Workspace {
    let user = User::new("Alice", "alice@example.com", "argon2-hash");
    let workspace = Workspace::new("Conversation Lab");
    store.create_user(&user).await.unwrap();
    store.create_workspace(&workspace).await.unwrap();
    workspace
}

fn entry(
    channel_id: &ChannelId,
    agent_id: &AgentId,
    role: ContextRole,
    content: &str,
) -> ContextEntry {
    let mut entry = ContextEntry::new(
        channel_id.clone(),
        agent_id.clone(),
        role,
        content.to_string(),
    );
    entry.recount();
    entry
}

#[tokio::test]
async fn conversation_round_trips_with_participant_and_settings() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let agent_id = AgentId::new("agent-planner");

    let channel = Channel::conversation("Design review", "Conversation with Planner");
    let settings = ConversationSettings::new(
        channel.id.clone(),
        Some("provider-1".to_string()),
        "deepseek-chat",
    );
    store
        .create_conversation(
            &workspace.id,
            &channel,
            &settings,
            std::slice::from_ref(&agent_id),
        )
        .await
        .unwrap();

    let loaded = store.get_channel(&channel.id).await.unwrap().unwrap();
    assert_eq!(loaded.kind, ChannelKind::Conversation);

    let participants = store.list_channel_participants(&channel.id).await.unwrap();
    assert_eq!(participants.len(), 1);
    assert_eq!(participants[0].agent_id, agent_id);

    let stored = store
        .get_conversation_settings(&channel.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.model, "deepseek-chat");
    assert_eq!(stored.provider_id.as_deref(), Some("provider-1"));
    assert_eq!(
        stored.mode,
        cowbot_core::conversation::ConversationMode::Single
    );
    assert!(!stored.auto_compact);
    assert_eq!(stored.compact_threshold, 80);

    let mut updated = stored.clone();
    updated.auto_compact = true;
    updated.compact_threshold = 70;
    updated.updated_at = Utc::now();
    store.update_conversation_settings(&updated).await.unwrap();
    let reloaded = store
        .get_conversation_settings(&channel.id)
        .await
        .unwrap()
        .unwrap();
    assert!(reloaded.auto_compact);
    assert_eq!(reloaded.compact_threshold, 70);
}

#[tokio::test]
async fn context_entries_are_sequential_per_agent_and_compact_atomically() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let channel = Channel::conversation("Branch", "");
    let agent_id = AgentId::new("agent-coder");
    let settings = ConversationSettings::new(channel.id.clone(), None, "mock-model");
    store
        .create_conversation(&workspace.id, &channel, &settings, &[agent_id.clone()])
        .await
        .unwrap();

    let first = entry(&channel.id, &agent_id, ContextRole::User, "first question");
    let second = entry(
        &channel.id,
        &agent_id,
        ContextRole::Assistant,
        "first answer",
    );
    assert_eq!(store.append_context_entry(&first).await.unwrap(), 1);
    assert_eq!(store.append_context_entry(&second).await.unwrap(), 2);

    let other_agent = AgentId::new("agent-reviewer");
    let other = entry(
        &channel.id,
        &other_agent,
        ContextRole::User,
        "independent branch",
    );
    assert_eq!(store.append_context_entry(&other).await.unwrap(), 1);

    let entries = store
        .list_context_entries(&channel.id, &agent_id)
        .await
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].seq, 1);
    assert_eq!(entries[0].content, "first question");
    assert_eq!(entries[1].seq, 2);

    let mut summary = ContextEntry::summary(
        channel.id.clone(),
        agent_id.clone(),
        "summary of the earlier turn",
    );
    summary.recount();
    let event = ContextEvent::new(
        channel.id.clone(),
        agent_id.clone(),
        ContextEventKind::Compact,
        10,
        4,
    );
    store
        .compact_context(&summary, &[first.id.clone(), second.id.clone()], &event)
        .await
        .unwrap();

    let after = store
        .list_context_entries(&channel.id, &agent_id)
        .await
        .unwrap();
    assert_eq!(after.len(), 3);
    let excluded: Vec<_> = after.iter().filter(|entry| entry.excluded).collect();
    assert_eq!(excluded.len(), 2);
    assert_eq!(
        after[2].kind,
        cowbot_core::conversation::ContextEntryKind::Summary
    );

    let events = store
        .list_context_events(&channel.id, &agent_id, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, ContextEventKind::Compact);

    // The explicit branch stays untouched by compaction of another agent.
    let reviewer_entries = store
        .list_context_entries(&channel.id, &other_agent)
        .await
        .unwrap();
    assert_eq!(reviewer_entries.len(), 1);
    assert!(!reviewer_entries[0].excluded);
}

#[tokio::test]
async fn clearing_history_removes_context_entries() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let channel = Channel::conversation("Clear me", "");
    let agent_id = AgentId::new("agent-coder");
    let settings = ConversationSettings::new(channel.id.clone(), None, "mock-model");
    store
        .create_conversation(&workspace.id, &channel, &settings, &[agent_id.clone()])
        .await
        .unwrap();
    store
        .append_context_entry(&entry(&channel.id, &agent_id, ContextRole::User, "hello"))
        .await
        .unwrap();

    assert_eq!(
        store
            .exclude_all_context_entries(&channel.id)
            .await
            .unwrap(),
        1
    );
    assert_eq!(store.clear_context_entries(&channel.id).await.unwrap(), 1);
    assert!(store
        .list_context_entries(&channel.id, &agent_id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn channel_usage_sums_all_calls_and_the_latest_run() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let channel = Channel::conversation("Usage", "");
    let agent_id = AgentId::new("agent-coder");
    let settings = ConversationSettings::new(channel.id.clone(), None, "mock-model");
    store
        .create_conversation(&workspace.id, &channel, &settings, &[agent_id.clone()])
        .await
        .unwrap();

    let task = Task::new("turn", "answer")
        .assign_to(agent_id.clone())
        .in_channel(channel.id.clone());
    store
        .create_task(&workspace.id, Some(&channel.id), &task)
        .await
        .unwrap();
    let run = Run::new(task.id.clone(), agent_id.clone());
    store.create_run(&run).await.unwrap();

    let mut first = LlmCallRecord::new(LlmCallKind::Agent, "mock-model");
    first.channel_id = Some(channel.id.clone());
    first.run_id = Some(run.id.clone());
    first.agent_id = Some(agent_id.clone());
    first.usage = Some(TokenUsage {
        uncached_input_tokens: 30,
        cached_input_tokens: 70,
        output_tokens: 10,
        ..TokenUsage::default()
    });
    store.record_llm_call(&first).await.unwrap();

    let mut compact = LlmCallRecord::new(LlmCallKind::Compact, "mock-model");
    compact.channel_id = Some(channel.id.clone());
    compact.agent_id = Some(agent_id.clone());
    compact.usage = Some(TokenUsage {
        uncached_input_tokens: 100,
        ..TokenUsage::default()
    });
    store.record_llm_call(&compact).await.unwrap();

    let totals = store.sum_llm_usage_for_channel(&channel.id).await.unwrap();
    assert_eq!(totals.calls, 2);
    assert_eq!(totals.total_input_tokens(), 200);

    let latest = store
        .latest_run_usage_for_channel(&channel.id)
        .await
        .unwrap();
    assert_eq!(latest.calls, 1);
    assert_eq!(latest.cached_input_tokens, 70);
    assert_eq!(latest.total_input_tokens(), 100);
}

const LEGACY_MIGRATIONS: [&str; 5] = [
    "0001_init.sql",
    "0002_assignments.sql",
    "0003_agent_channels.sql",
    "0004_channel_management.sql",
    "0005_llm_calls.sql",
];

fn migrations_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations")
}

async fn create_legacy_database(path: &Path, root: &Path) {
    let target = root.join("legacy-migrations");
    fs::create_dir_all(&target).unwrap();
    for name in LEGACY_MIGRATIONS {
        fs::copy(migrations_dir().join(name), target.join(name)).unwrap();
    }
    let migrator = Migrator::new(target).await.unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    migrator.run(&pool).await.unwrap();
    pool.close().await;
}

#[tokio::test]
async fn migration_backfills_channel_kinds() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("legacy.db");
    create_legacy_database(&path, temp.path()).await;

    let store = Store::connect(&path).await.unwrap();
    sqlx::query(
        "INSERT INTO workspaces (id, name, created_at) VALUES ('w1', 'Legacy', '2026-01-01T00:00:00Z')",
    )
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO channels (id, workspace_id, name, topic, created_at, agent_id)
         VALUES ('group-1', 'w1', 'general', '', '2026-01-01T00:00:00Z', NULL)",
    )
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO channels (id, workspace_id, name, topic, created_at, agent_id)
         VALUES ('direct-1', 'w1', 'Planner', '', '2026-01-01T00:00:00Z', 'agent-1')",
    )
    .execute(store.pool())
    .await
    .unwrap();

    store.migrate().await.unwrap();

    let group = store
        .get_channel(&ChannelId::new("group-1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(group.kind, ChannelKind::Channel);
    let direct = store
        .get_channel(&ChannelId::new("direct-1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(direct.kind, ChannelKind::Direct);
}
