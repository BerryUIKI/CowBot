use std::fs;
use std::path::Path;

use chrono::Utc;
use cowbot_core::agent::AgentId;
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage, Sender};
use cowbot_core::conversation::{
    ChannelKind, CompactionWrite, ContextEntry, ContextEntryKind, ContextEvent, ContextEventKind,
    ContextRole, ConversationMode, ConversationSettings, ConversationTurn, RouteSource,
};
use cowbot_core::identity::{User, Workspace};
use cowbot_core::run::{Run, RunId};
use cowbot_core::task::{Task, TaskId};
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
        .create_conversation(
            &workspace.id,
            &channel,
            &settings,
            std::slice::from_ref(&agent_id),
        )
        .await
        .unwrap();

    let first = entry(&channel.id, &agent_id, ContextRole::User, "first question");
    let second = entry(
        &channel.id,
        &agent_id,
        ContextRole::Assistant,
        "first answer",
    );
    let third = entry(
        &channel.id,
        &agent_id,
        ContextRole::User,
        "the kept follow-up",
    );
    assert_eq!(store.append_context_entry(&first).await.unwrap(), 1);
    assert_eq!(store.append_context_entry(&second).await.unwrap(), 2);
    let third_seq = store.append_context_entry(&third).await.unwrap();
    assert_eq!(third_seq, 3);

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
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].seq, 1);
    assert_eq!(entries[0].content, "first question");
    assert_eq!(entries[1].seq, 2);

    let mut summary = ContextEntry::summary(
        channel.id.clone(),
        agent_id.clone(),
        "summary of the earlier turn",
    );
    summary.recount();
    let mut event = ContextEvent::new(
        channel.id.clone(),
        agent_id.clone(),
        ContextEventKind::Compact,
        10,
        4,
    );
    let written = store
        .compact_context(
            &summary,
            &[first.id.clone(), second.id.clone()],
            third_seq,
            &mut event,
        )
        .await
        .unwrap();
    assert!(matches!(
        written,
        cowbot_core::conversation::CompactionWrite::Applied { .. }
    ));

    let after = store
        .list_context_entries(&channel.id, &agent_id)
        .await
        .unwrap();
    assert_eq!(after.len(), 4);
    let excluded: Vec<_> = after.iter().filter(|entry| entry.excluded).collect();
    assert_eq!(excluded.len(), 2);
    assert_eq!(
        after[2].kind,
        cowbot_core::conversation::ContextEntryKind::Summary
    );
    assert_eq!(after[2].seq, third_seq);
    assert_eq!(after[3].seq, third_seq + 1);
    assert_eq!(after[3].content, "the kept follow-up");
    assert!(!after[3].excluded);
    assert!(event.tokens_before > 0);
    assert!(event.tokens_after > 0);

    let fourth = entry(
        &channel.id,
        &agent_id,
        ContextRole::User,
        "a message appended after compaction",
    );
    let fourth_seq = store.append_context_entry(&fourth).await.unwrap();
    assert_eq!(fourth_seq, third_seq + 2);
    let final_entries = store
        .list_context_entries(&channel.id, &agent_id)
        .await
        .unwrap();
    assert_eq!(
        final_entries.last().map(|entry| entry.content.as_str()),
        Some("a message appended after compaction")
    );
    assert_eq!(
        final_entries.last().map(|entry| entry.seq),
        Some(fourth_seq)
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
async fn compaction_rejects_a_stale_prefix_without_writing() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let channel = Channel::conversation("Stale compaction", "");
    let agent_id = AgentId::new("agent-coder");
    let settings = ConversationSettings::new(channel.id.clone(), None, "mock-model");
    store
        .create_conversation(
            &workspace.id,
            &channel,
            &settings,
            std::slice::from_ref(&agent_id),
        )
        .await
        .unwrap();

    let first = entry(&channel.id, &agent_id, ContextRole::User, "one");
    let second = entry(&channel.id, &agent_id, ContextRole::Assistant, "two");
    let third = entry(&channel.id, &agent_id, ContextRole::User, "three");
    store.append_context_entry(&first).await.unwrap();
    store.append_context_entry(&second).await.unwrap();
    store.append_context_entry(&third).await.unwrap();

    // Another writer excludes one of the entries the summary was built from.
    store
        .exclude_context_entries(std::slice::from_ref(&first.id))
        .await
        .unwrap();

    let mut summary = ContextEntry::summary(
        channel.id.clone(),
        agent_id.clone(),
        "summary that must not be written",
    );
    summary.recount();
    let mut event = ContextEvent::new(
        channel.id.clone(),
        agent_id.clone(),
        ContextEventKind::Compact,
        10,
        4,
    );
    let written = store
        .compact_context(
            &summary,
            &[first.id.clone(), second.id.clone()],
            3,
            &mut event,
        )
        .await
        .unwrap();
    assert_eq!(written, CompactionWrite::Stale);

    let after = store
        .list_context_entries(&channel.id, &agent_id)
        .await
        .unwrap();
    assert_eq!(after.len(), 3);
    assert!(after
        .iter()
        .all(|entry| entry.kind != ContextEntryKind::Summary));
    assert!(after[0].excluded);
    assert!(!after[1].excluded);
    assert!(!after[2].excluded);
    assert_eq!(
        after.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(store
        .list_context_events(&channel.id, &agent_id, 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(event.tokens_before, 10);
    assert_eq!(event.tokens_after, 4);
}

#[tokio::test]
async fn compaction_rejects_non_positive_sequence_numbers() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let channel = Channel::conversation("Imported branch", "");
    let agent_id = AgentId::new("agent-coder");
    let settings = ConversationSettings::new(channel.id.clone(), None, "mock-model");
    store
        .create_conversation(
            &workspace.id,
            &channel,
            &settings,
            std::slice::from_ref(&agent_id),
        )
        .await
        .unwrap();

    let mut first = entry(&channel.id, &agent_id, ContextRole::User, "imported");
    first.seq = 0;
    store.insert_context_entry(&first).await.unwrap();
    let mut second = entry(&channel.id, &agent_id, ContextRole::Assistant, "kept");
    second.seq = 1;
    store.insert_context_entry(&second).await.unwrap();

    let mut summary = ContextEntry::summary(
        channel.id.clone(),
        agent_id.clone(),
        "summary that must not be written",
    );
    summary.recount();
    let mut event = ContextEvent::new(
        channel.id.clone(),
        agent_id.clone(),
        ContextEventKind::Compact,
        10,
        4,
    );
    let error = store
        .compact_context(&summary, std::slice::from_ref(&first.id), 1, &mut event)
        .await
        .unwrap_err();
    assert!(matches!(error, cowbot_core::error::CowError::Config(_)));
}

#[tokio::test]
async fn clearing_history_removes_context_entries() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let channel = Channel::conversation("Clear me", "");
    let agent_id = AgentId::new("agent-coder");
    let settings = ConversationSettings::new(channel.id.clone(), None, "mock-model");
    store
        .create_conversation(
            &workspace.id,
            &channel,
            &settings,
            std::slice::from_ref(&agent_id),
        )
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
        .create_conversation(
            &workspace.id,
            &channel,
            &settings,
            std::slice::from_ref(&agent_id),
        )
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

const MIGRATIONS_BEFORE_GROUPS: [&str; 12] = [
    "0001_init.sql",
    "0002_assignments.sql",
    "0003_agent_channels.sql",
    "0004_channel_management.sql",
    "0005_llm_calls.sql",
    "0006_task_title_and_archive.sql",
    "0007_conversations.sql",
    "0008_permission_rules.sql",
    "0009_task_plan.sql",
    "0010_workspace_policy.sql",
    "0011_task_run_group.sql",
    "0012_task_worktrees.sql",
];

fn migrations_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations")
}

async fn create_legacy_database(path: &Path, root: &Path) {
    create_database_with_migrations(path, root, &LEGACY_MIGRATIONS).await;
}

async fn create_database_with_migrations(path: &Path, root: &Path, files: &[&str]) {
    let target = root.join("legacy-migrations");
    fs::create_dir_all(&target).unwrap();
    for name in files {
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

#[tokio::test]
async fn migration_backfills_participant_positions_in_creation_order() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("legacy.db");
    create_database_with_migrations(&path, temp.path(), &MIGRATIONS_BEFORE_GROUPS).await;

    let store = Store::connect(&path).await.unwrap();
    sqlx::query(
        "INSERT INTO workspaces (id, name, created_at)
         VALUES ('w1', 'Legacy', '2026-01-01T00:00:00Z')",
    )
    .execute(store.pool())
    .await
    .unwrap();
    for channel in ["group-1", "group-2"] {
        sqlx::query(
            "INSERT INTO channels (id, workspace_id, name, topic, created_at, agent_id, pinned, kind)
             VALUES (?, 'w1', ?, '', '2026-01-01T00:00:00Z', NULL, 0, 'channel')",
        )
        .bind(channel)
        .bind(channel)
        .execute(store.pool())
        .await
        .unwrap();
    }
    for (channel, agent, joined_at) in [
        ("group-1", "agent-b", "2026-01-01T00:00:00Z"),
        ("group-1", "agent-a", "2026-01-01T00:00:00Z"),
        ("group-1", "agent-c", "2026-01-01T00:00:00Z"),
        ("group-2", "agent-x", "2026-01-01T00:00:02Z"),
        ("group-2", "agent-w", "2026-01-01T00:00:01Z"),
    ] {
        sqlx::query(
            "INSERT INTO channel_participants (channel_id, agent_id, joined_at)
             VALUES (?, ?, ?)",
        )
        .bind(channel)
        .bind(agent)
        .bind(joined_at)
        .execute(store.pool())
        .await
        .unwrap();
    }

    store.migrate().await.unwrap();

    let tied = store
        .list_channel_participants(&ChannelId::new("group-1"))
        .await
        .unwrap();
    let ids: Vec<&str> = tied.iter().map(|p| p.agent_id.0.as_str()).collect();
    assert_eq!(ids, vec!["agent-a", "agent-b", "agent-c"]);
    assert_eq!(
        tied.iter().map(|p| p.position).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );

    let timed = store
        .list_channel_participants(&ChannelId::new("group-2"))
        .await
        .unwrap();
    let ids: Vec<&str> = timed.iter().map(|p| p.agent_id.0.as_str()).collect();
    assert_eq!(ids, vec!["agent-w", "agent-x"]);
    assert_eq!(
        timed.iter().map(|p| p.position).collect::<Vec<_>>(),
        vec![0, 1]
    );
}

#[tokio::test]
async fn group_conversation_keeps_selection_order_and_default_responder() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let channel = Channel::conversation("Design crew", "Group conversation");
    let agents = [
        AgentId::new("agent-coder"),
        AgentId::new("agent-planner"),
        AgentId::new("agent-reviewer"),
    ];
    let settings = ConversationSettings::group(
        channel.id.clone(),
        None,
        "mock-model",
        Some(agents[1].clone()),
    );
    store
        .create_conversation(&workspace.id, &channel, &settings, &agents)
        .await
        .unwrap();

    let participants = store.list_channel_participants(&channel.id).await.unwrap();
    let ids: Vec<&str> = participants
        .iter()
        .map(|participant| participant.agent_id.0.as_str())
        .collect();
    assert_eq!(ids, vec!["agent-coder", "agent-planner", "agent-reviewer"]);
    assert_eq!(
        participants
            .iter()
            .map(|participant| participant.position)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );

    let stored = store
        .get_conversation_settings(&channel.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.mode, ConversationMode::Group);
    assert_eq!(stored.default_responder.as_ref(), Some(&agents[1]));
    assert!(!stored.router_enabled);

    let mut updated = stored.clone();
    updated.default_responder = Some(agents[2].clone());
    updated.router_enabled = true;
    updated.updated_at = Utc::now();
    store.update_conversation_settings(&updated).await.unwrap();
    let reloaded = store
        .get_conversation_settings(&channel.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.default_responder.as_ref(), Some(&agents[2]));
    assert!(reloaded.router_enabled);

    // A restored package keeps the default responder too.
    let mut cleared = reloaded.clone();
    cleared.default_responder = None;
    store.insert_conversation_settings(&cleared).await.unwrap();
    let restored = store
        .get_conversation_settings(&channel.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.default_responder, None);
}

#[tokio::test]
async fn authored_entries_round_trip() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let channel = Channel::conversation("Peer history", "");
    let agents = [AgentId::new("agent-coder"), AgentId::new("agent-planner")];
    let settings = ConversationSettings::group(channel.id.clone(), None, "mock-model", None);
    store
        .create_conversation(&workspace.id, &channel, &settings, &agents)
        .await
        .unwrap();

    let peer = ContextEntry::peer(
        channel.id.clone(),
        agents[0].clone(),
        agents[1].clone(),
        "Planner",
        "The draft is ready for review.",
    );
    store.append_context_entry(&peer).await.unwrap();

    let entries = store
        .list_context_entries(&channel.id, &agents[0])
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].author_agent_id.as_ref(), Some(&agents[1]));
    assert_eq!(entries[0].role, ContextRole::User);
    assert_eq!(
        entries[0].content,
        "[Planner] The draft is ready for review."
    );

    // Package restore keeps the attribution.
    let mut restored = peer.clone();
    restored.id = "restored-entry".to_string();
    store.insert_context_entry(&restored).await.unwrap();
    let entries = store
        .list_context_entries(&channel.id, &agents[0])
        .await
        .unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.id == "restored-entry"
            && entry.author_agent_id.as_ref() == Some(&agents[1])));
}

#[tokio::test]
async fn conversation_turns_round_trip_and_cascade_with_messages_and_channels() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let channel = Channel::conversation("Routing audit", "");
    let agents = [AgentId::new("agent-planner"), AgentId::new("agent-coder")];
    let settings = ConversationSettings::group(channel.id.clone(), None, "mock-model", None);
    store
        .create_conversation(&workspace.id, &channel, &settings, &agents)
        .await
        .unwrap();

    let message = ChannelMessage::new(
        channel.id.clone(),
        Sender::User("local".to_string()),
        "@Planner @Ghost plan the release",
    );
    store.append_message(&message).await.unwrap();

    let mut turn =
        ConversationTurn::new(channel.id.clone(), message.id.clone(), RouteSource::Mention);
    turn.selected_agents = vec![agents[0].clone()];
    turn.unknown_mentions = vec!["Ghost".to_string()];
    turn.reason = Some("explicit mentions".to_string());
    store.insert_conversation_turn(&turn).await.unwrap();
    // A retried post must not duplicate the audit row.
    store.insert_conversation_turn(&turn).await.unwrap();

    let turns = store
        .list_conversation_turns(&channel.id, 10)
        .await
        .unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].source, RouteSource::Mention);
    assert_eq!(turns[0].selected_agents, vec![agents[0].clone()]);

    let mut settled = turns[0].clone();
    settled.task_ids = vec![TaskId::new("task-1")];
    settled.mirrored_at = Some(Utc::now());
    store.update_conversation_turn(&settled).await.unwrap();

    let stored = store
        .get_conversation_turn(&channel.id, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.unknown_mentions, vec!["Ghost".to_string()]);
    assert_eq!(stored.task_ids, vec![TaskId::new("task-1")]);
    assert!(stored.mirrored_at.is_some());

    // Clearing context keeps the routing audit.
    store
        .append_context_entry(&entry(&channel.id, &agents[0], ContextRole::User, "hello"))
        .await
        .unwrap();
    store.clear_context_entries(&channel.id).await.unwrap();
    assert!(store
        .get_conversation_turn(&channel.id, &message.id)
        .await
        .unwrap()
        .is_some());

    // Deleting the message removes its turn.
    store.clear_channel_messages(&channel.id).await.unwrap();
    assert!(store
        .get_conversation_turn(&channel.id, &message.id)
        .await
        .unwrap()
        .is_none());

    // Deleting the channel removes any remaining turns.
    let message = ChannelMessage::new(
        channel.id.clone(),
        Sender::User("local".to_string()),
        "@team sync",
    );
    store.append_message(&message).await.unwrap();
    store
        .insert_conversation_turn(&ConversationTurn::new(
            channel.id.clone(),
            message.id.clone(),
            RouteSource::Silent,
        ))
        .await
        .unwrap();
    store.delete_channel(&channel.id).await.unwrap();
    assert!(store
        .list_conversation_turns(&channel.id, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn per_agent_latest_run_usage_skips_router_calls() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let channel = Channel::conversation("Group usage", "");
    let agents = [AgentId::new("agent-planner"), AgentId::new("agent-coder")];
    let settings = ConversationSettings::group(channel.id.clone(), None, "mock-model", None);
    store
        .create_conversation(&workspace.id, &channel, &settings, &agents)
        .await
        .unwrap();

    let base = Utc::now();
    let record = |kind: LlmCallKind, agent: &AgentId, run: &RunId, tokens: u32, offset: i64| {
        let mut call = LlmCallRecord::new(kind, "mock-model");
        call.channel_id = Some(channel.id.clone());
        call.agent_id = Some(agent.clone());
        call.run_id = Some(run.clone());
        call.usage = Some(TokenUsage {
            uncached_input_tokens: tokens,
            ..TokenUsage::default()
        });
        call.created_at = base + chrono::Duration::seconds(offset);
        call
    };

    let planner_task = Task::new("plan", "answer")
        .assign_to(agents[0].clone())
        .in_channel(channel.id.clone());
    store
        .create_task(&workspace.id, Some(&channel.id), &planner_task)
        .await
        .unwrap();
    let coder_task = Task::new("code", "answer")
        .assign_to(agents[1].clone())
        .in_channel(channel.id.clone());
    store
        .create_task(&workspace.id, Some(&channel.id), &coder_task)
        .await
        .unwrap();

    let first_run = Run::new(planner_task.id.clone(), agents[0].clone());
    let second_run = Run::new(planner_task.id.clone(), agents[0].clone());
    let other_run = Run::new(coder_task.id.clone(), agents[1].clone());
    for run in [&first_run, &second_run, &other_run] {
        store.create_run(run).await.unwrap();
    }

    store
        .record_llm_call(&record(
            LlmCallKind::Agent,
            &agents[0],
            &first_run.id,
            10,
            1,
        ))
        .await
        .unwrap();
    store
        .record_llm_call(&record(
            LlmCallKind::Agent,
            &agents[0],
            &second_run.id,
            25,
            2,
        ))
        .await
        .unwrap();
    // A newer router call on an older run must not become the agent's latest.
    store
        .record_llm_call(&record(
            LlmCallKind::Router,
            &agents[0],
            &first_run.id,
            5,
            3,
        ))
        .await
        .unwrap();
    store
        .record_llm_call(&record(
            LlmCallKind::Agent,
            &agents[1],
            &other_run.id,
            40,
            4,
        ))
        .await
        .unwrap();

    let planner = store
        .latest_run_usage_for_channel_agent(&channel.id, &agents[0])
        .await
        .unwrap();
    assert_eq!(planner.calls, 1);
    assert_eq!(planner.total_input_tokens(), 25);

    let coder = store
        .latest_run_usage_for_channel_agent(&channel.id, &agents[1])
        .await
        .unwrap();
    assert_eq!(coder.calls, 1);
    assert_eq!(coder.total_input_tokens(), 40);

    // The conversation total still counts every call, router included.
    let total = store.sum_llm_usage_for_channel(&channel.id).await.unwrap();
    assert_eq!(total.calls, 4);
    assert_eq!(total.total_input_tokens(), 80);
}
