use chrono::Utc;
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::channel::Channel;
use cowbot_core::identity::{User, Workspace};
use cowbot_core::memory::{
    MemoryEntry, MemoryKind, MemorySource, MemoryStatus, Note, DEFAULT_NOTES_BUDGET_TOKENS,
    DEFAULT_RECALL_BUDGET_TOKENS, MAX_MEMORY_BUDGET_TOKENS, MIN_MEMORY_BUDGET_TOKENS,
};
use cowbot_store::Store;

async fn test_store() -> Store {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    store
}

struct Seed {
    workspace: Workspace,
    channel: Channel,
    planner: AgentId,
    coder: AgentId,
}

async fn seed(store: &Store) -> Seed {
    let user = User::new("Alice", "alice@example.com", "argon2-hash");
    let workspace = Workspace::new("Memory Lab");
    store.create_user(&user).await.unwrap();
    store.create_workspace(&workspace).await.unwrap();

    let channel = Channel::conversation("Memory conversation", "");
    store.create_channel(&workspace.id, &channel).await.unwrap();

    let planner = AgentProfile::new("Planner", "Architect", "Plan", "Careful");
    let coder = AgentProfile::new("Coder", "Engineer", "Ship", "Careful");
    store.upsert_agent(&workspace.id, &planner).await.unwrap();
    store.upsert_agent(&workspace.id, &coder).await.unwrap();

    Seed {
        workspace,
        channel,
        planner: planner.id,
        coder: coder.id,
    }
}

fn entry(
    seed: &Seed,
    agent: Option<&AgentId>,
    channel: Option<&Channel>,
    kind: MemoryKind,
    content: &str,
) -> MemoryEntry {
    MemoryEntry::new(
        seed.workspace.id.clone(),
        agent.cloned(),
        channel.map(|channel| channel.id.clone()),
        kind,
        content,
        MemorySource::User,
    )
}

fn search_ids(hits: &[(MemoryEntry, f64)]) -> Vec<String> {
    hits.iter().map(|(entry, _)| entry.id.clone()).collect()
}

#[tokio::test]
async fn notes_replace_in_place_by_scope() {
    let store = test_store().await;
    let seed = seed(&store).await;

    let workspace_note = Note::workspace(seed.workspace.id.clone(), "house rules");
    let stored = store.save_note(&workspace_note).await.unwrap();
    let mut update = Note::workspace(seed.workspace.id.clone(), "updated rules");
    update.updated_at = Utc::now();
    let replaced = store.save_note(&update).await.unwrap();
    assert_eq!(replaced.id, stored.id, "the workspace note keeps its id");
    assert_eq!(replaced.created_at, stored.created_at);
    assert_eq!(replaced.content, "updated rules");

    let conversation_note = Note::conversation(
        seed.workspace.id.clone(),
        seed.channel.id.clone(),
        "channel rules",
    );
    let stored_conversation = store.save_note(&conversation_note).await.unwrap();

    let fetched = store
        .get_note(&seed.workspace.id, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.content, "updated rules");
    let fetched_channel = store
        .get_note(&seed.workspace.id, Some(&seed.channel.id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched_channel.id, stored_conversation.id);

    let all = store.list_all_notes(&seed.workspace.id).await.unwrap();
    assert_eq!(all.len(), 2);
    assert!(all[0].is_workspace_note(), "workspace note sorts first");

    assert_eq!(
        store
            .delete_note(&seed.workspace.id, Some(&seed.channel.id))
            .await
            .unwrap(),
        1
    );
    assert!(store
        .get_note(&seed.workspace.id, Some(&seed.channel.id))
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get_note(&seed.workspace.id, None)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn entries_index_and_update_their_search_text() {
    let store = test_store().await;
    let seed = seed(&store).await;
    let entry = entry(
        &seed,
        Some(&seed.planner),
        None,
        MemoryKind::Fact,
        "The user prefers Rust for services.",
    );
    store.insert_memory_entry(&entry).await.unwrap();

    let hits = store
        .search_memory_entries(&seed.workspace.id, None, Some(&seed.planner), "Rust", 10)
        .await
        .unwrap();
    assert_eq!(search_ids(&hits), vec![entry.id.clone()]);
    assert!(hits[0].1.is_finite(), "bm25 scores may be negative");
    assert_eq!(
        store.get_memory_entry(&entry.id).await.unwrap().unwrap(),
        entry
    );

    let mut updated = entry.clone();
    updated.content = "The user prefers Go now.".to_string();
    updated.recount();
    updated.updated_at = Utc::now();
    store.update_memory_entry(&updated).await.unwrap();

    assert!(store
        .search_memory_entries(&seed.workspace.id, None, Some(&seed.planner), "Rust", 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        search_ids(
            &store
                .search_memory_entries(&seed.workspace.id, None, Some(&seed.planner), "Go", 10)
                .await
                .unwrap()
        ),
        vec![entry.id.clone()]
    );
    let indexed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memory_entries_fts WHERE id = ?")
        .bind(&entry.id)
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(indexed, 1, "the update trigger leaves one index row");

    assert_eq!(store.delete_memory_entry(&entry.id).await.unwrap(), 1);
    assert!(store.get_memory_entry(&entry.id).await.unwrap().is_none());
    let indexed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memory_entries_fts WHERE id = ?")
        .bind(&entry.id)
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(indexed, 0, "the delete trigger removes the index row");
    assert!(store
        .search_memory_entries(&seed.workspace.id, None, Some(&seed.planner), "Go", 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn search_respects_scope_status_and_workspace() {
    let store = test_store().await;
    let seed = seed(&store).await;

    let shared = entry(&seed, None, None, MemoryKind::Fact, "shared alpha");
    let agent = entry(
        &seed,
        Some(&seed.planner),
        None,
        MemoryKind::Fact,
        "agent alpha",
    );
    let conversation = entry(
        &seed,
        None,
        Some(&seed.channel),
        MemoryKind::Fact,
        "conversation alpha",
    );
    let both = entry(
        &seed,
        Some(&seed.planner),
        Some(&seed.channel),
        MemoryKind::Fact,
        "both alpha",
    );
    let other_agent = entry(
        &seed,
        Some(&seed.coder),
        None,
        MemoryKind::Fact,
        "coder alpha",
    );
    let pending = entry(&seed, None, None, MemoryKind::Fact, "pending alpha")
        .with_status(MemoryStatus::Pending);
    let archived = entry(&seed, None, None, MemoryKind::Fact, "archived alpha")
        .with_status(MemoryStatus::Archived);
    for item in [
        &shared,
        &agent,
        &conversation,
        &both,
        &other_agent,
        &pending,
        &archived,
    ] {
        store.insert_memory_entry(item).await.unwrap();
    }

    let in_conversation = store
        .search_memory_entries(
            &seed.workspace.id,
            Some(&seed.channel.id),
            Some(&seed.planner),
            "alpha",
            20,
        )
        .await
        .unwrap();
    let mut ids = search_ids(&in_conversation);
    ids.sort();
    let mut expected = vec![
        shared.id.clone(),
        agent.id.clone(),
        conversation.id.clone(),
        both.id.clone(),
    ];
    expected.sort();
    assert_eq!(ids, expected);

    let workspace_level = store
        .search_memory_entries(&seed.workspace.id, None, Some(&seed.planner), "alpha", 20)
        .await
        .unwrap();
    let mut ids = search_ids(&workspace_level);
    ids.sort();
    let mut expected = vec![shared.id.clone(), agent.id.clone()];
    expected.sort();
    assert_eq!(ids, expected);

    // Another workspace is invisible even with the same content.
    let other_workspace = Workspace::new("Other");
    store.create_workspace(&other_workspace).await.unwrap();
    let mut foreign = MemoryEntry::new(
        other_workspace.id.clone(),
        None,
        None,
        MemoryKind::Fact,
        "foreign alpha",
        MemorySource::User,
    );
    foreign.recount();
    store.insert_memory_entry(&foreign).await.unwrap();
    let ids = search_ids(
        &store
            .search_memory_entries(&seed.workspace.id, None, None, "alpha", 20)
            .await
            .unwrap(),
    );
    assert_eq!(ids, vec![shared.id.clone()]);
    assert_eq!(
        search_ids(
            &store
                .search_memory_entries(&other_workspace.id, None, None, "alpha", 20)
                .await
                .unwrap()
        ),
        vec![foreign.id.clone()]
    );
}

#[tokio::test]
async fn short_queries_and_chinese_text_fall_back_to_like() {
    let store = test_store().await;
    let seed = seed(&store).await;
    let latin = entry(
        &seed,
        None,
        None,
        MemoryKind::Fact,
        "The user prefers Rust.",
    );
    let chinese = entry(
        &seed,
        None,
        None,
        MemoryKind::Note,
        "会议改到周四，地点在三楼。",
    );
    store.insert_memory_entry(&latin).await.unwrap();
    store.insert_memory_entry(&chinese).await.unwrap();

    assert_eq!(
        search_ids(
            &store
                .search_memory_entries(&seed.workspace.id, None, None, "ru", 10)
                .await
                .unwrap()
        ),
        vec![latin.id.clone()]
    );
    assert_eq!(
        search_ids(
            &store
                .search_memory_entries(&seed.workspace.id, None, None, "会议", 10)
                .await
                .unwrap()
        ),
        vec![chinese.id.clone()]
    );
    assert_eq!(
        search_ids(
            &store
                .search_memory_entries(&seed.workspace.id, None, None, "会议改到", 10)
                .await
                .unwrap()
        ),
        vec![chinese.id.clone()]
    );
}

#[tokio::test]
async fn pinned_entries_are_listed_and_recall_stats_advance() {
    let store = test_store().await;
    let seed = seed(&store).await;
    let pinned = entry(&seed, None, None, MemoryKind::Preference, "pinned fact").with_pinned(true);
    let other = entry(&seed, None, None, MemoryKind::Fact, "other fact");
    store.insert_memory_entry(&pinned).await.unwrap();
    store.insert_memory_entry(&other).await.unwrap();

    let listed = store
        .list_pinned_memory_entries(&seed.workspace.id, None, Some(&seed.planner), 10)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, pinned.id);

    store
        .touch_memory_recalls(&[pinned.id.clone(), other.id.clone()])
        .await
        .unwrap();
    let reloaded = store.get_memory_entry(&pinned.id).await.unwrap().unwrap();
    assert_eq!(reloaded.recall_count, 1);
    assert!(reloaded.last_recalled_at.is_some());
}

#[tokio::test]
async fn memory_settings_create_defaults_and_clamp_updates() {
    let store = test_store().await;
    let seed = seed(&store).await;

    let defaults = store.get_memory_settings(&seed.workspace.id).await.unwrap();
    assert!(!defaults.extraction_enabled);
    assert!(defaults.recall_enabled);
    assert_eq!(defaults.notes_budget_tokens, DEFAULT_NOTES_BUDGET_TOKENS);
    assert_eq!(defaults.recall_budget_tokens, DEFAULT_RECALL_BUDGET_TOKENS);

    let mut updated = defaults.clone();
    updated.extraction_enabled = true;
    updated.notes_budget_tokens = 1;
    updated.recall_budget_tokens = 100_000;
    updated.updated_at = Utc::now();
    store.update_memory_settings(&updated).await.unwrap();

    let reloaded = store.get_memory_settings(&seed.workspace.id).await.unwrap();
    assert!(reloaded.extraction_enabled);
    assert_eq!(reloaded.notes_budget_tokens, MIN_MEMORY_BUDGET_TOKENS);
    assert_eq!(reloaded.recall_budget_tokens, MAX_MEMORY_BUDGET_TOKENS);
}

#[tokio::test]
async fn deleting_a_channel_or_workspace_cascades_memory() {
    let store = test_store().await;
    let seed = seed(&store).await;

    let shared = entry(&seed, None, None, MemoryKind::Fact, "shared");
    let agent = entry(&seed, Some(&seed.planner), None, MemoryKind::Fact, "agent");
    let conversation = entry(
        &seed,
        None,
        Some(&seed.channel),
        MemoryKind::Fact,
        "conversation",
    );
    let both = entry(
        &seed,
        Some(&seed.planner),
        Some(&seed.channel),
        MemoryKind::Fact,
        "both",
    );
    for item in [&shared, &agent, &conversation, &both] {
        store.insert_memory_entry(item).await.unwrap();
    }
    store
        .save_note(&Note::conversation(
            seed.workspace.id.clone(),
            seed.channel.id.clone(),
            "channel note",
        ))
        .await
        .unwrap();

    store.delete_channel(&seed.channel.id).await.unwrap();
    assert!(store
        .get_memory_entry(&conversation.id)
        .await
        .unwrap()
        .is_none());
    assert!(store.get_memory_entry(&both.id).await.unwrap().is_none());
    assert!(store.get_memory_entry(&shared.id).await.unwrap().is_some());
    assert!(store.get_memory_entry(&agent.id).await.unwrap().is_some());
    let indexed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM memory_entries_fts WHERE id IN (?, ?)")
            .bind(&conversation.id)
            .bind(&both.id)
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(
        indexed, 0,
        "the delete trigger follows foreign-key cascades"
    );
    assert!(store
        .get_note(&seed.workspace.id, Some(&seed.channel.id))
        .await
        .unwrap()
        .is_none());

    sqlx::query("DELETE FROM workspaces WHERE id = ?")
        .bind(&seed.workspace.id.0)
        .execute(store.pool())
        .await
        .unwrap();
    assert!(store.get_memory_entry(&shared.id).await.unwrap().is_none());
    assert!(store.get_memory_entry(&agent.id).await.unwrap().is_none());
}
