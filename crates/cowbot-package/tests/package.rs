use cowbot_core::agent::AgentProfile;
use cowbot_core::audit::AuditEvent;
use cowbot_core::channel::{Channel, ChannelMessage, Sender};
use cowbot_core::conversation::{
    ContextEntry, ContextEvent, ContextEventKind, ContextRole, ConversationMode,
    ConversationSettings, ConversationTurn, RouteSource,
};
use cowbot_core::identity::{Membership, Role, User, Workspace};
use cowbot_core::memory::{
    MemoryEntry, MemoryKind, MemorySettings, MemorySource, MemoryStatus, Note,
};
use cowbot_core::permission::{PermissionRule, RuleDecision, RuleSource};
use cowbot_core::task::{Task, TaskStatus};
use cowbot_package::{
    export_workspace, import_workspace, inspect, PackageError, PackageKind, PACKAGE_FORMAT,
};
use cowbot_store::Store;

async fn test_store() -> Store {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    store
}

async fn seed(store: &Store) -> Workspace {
    let user = User::new("Alice", "alice@example.com", "argon2-hash");
    let workspace = Workspace::new("Research Lab");
    store.create_user(&user).await.unwrap();
    store.create_workspace(&workspace).await.unwrap();
    store
        .add_member(&Membership::new(
            user.id.clone(),
            workspace.id.clone(),
            Role::Owner,
        ))
        .await
        .unwrap();

    let channel = Channel::new("general", "Team chatter");
    store.create_channel(&workspace.id, &channel).await.unwrap();
    store
        .append_message(&ChannelMessage::new(
            channel.id.clone(),
            Sender::User(user.id.0.clone()),
            "hello from the export test",
        ))
        .await
        .unwrap();

    let agent = AgentProfile::new("Cow-Coder", "Engineer", "Ship code", "Terse.");
    store.upsert_agent(&workspace.id, &agent).await.unwrap();

    let task = Task::new("Do the thing", "A working thing");
    store.create_task(&workspace.id, None, &task).await.unwrap();

    workspace
}

#[tokio::test]
async fn export_then_import_round_trip() {
    let source = test_store().await;
    let workspace = seed(&source).await;
    let rule = PermissionRule::new(
        workspace.id.clone(),
        "execute_code",
        Some("git status*".to_string()),
        RuleDecision::Allow,
    );
    source.upsert_permission_rule(&rule).await.unwrap();

    let bytes = export_workspace(&source, &workspace.id).await.unwrap();
    assert!(!bytes.is_empty());

    let manifest = inspect(&bytes).unwrap();
    assert_eq!(manifest.format, PACKAGE_FORMAT);
    assert_eq!(manifest.kind, PackageKind::Workspace);
    assert_eq!(manifest.exported_by.workspace_name, "Research Lab");
    assert!(!manifest.checksums.is_empty());

    let target = test_store().await;
    let summary = import_workspace(&target, &bytes).await.unwrap();
    assert_eq!(summary.channels, 1);
    assert_eq!(summary.messages, 1);
    assert_eq!(summary.agents, 1);
    assert_eq!(summary.tasks, 1);
    assert_eq!(summary.permission_rules, 1);
    assert!(summary.workspace_name.ends_with("(imported)"));

    let imported_rules = target
        .list_permission_rules(&summary.workspace_id)
        .await
        .unwrap();
    assert_eq!(imported_rules.len(), 1);
    assert!(!imported_rules[0].enabled);
    assert_eq!(imported_rules[0].source, RuleSource::Imported);
    assert_eq!(imported_rules[0].command.as_deref(), Some("git status*"));
    assert_ne!(imported_rules[0].id, rule.id);

    let channels = target.list_channels(&summary.workspace_id).await.unwrap();
    assert_eq!(channels.len(), 1);
    let messages = target.list_messages(&channels[0].id, 10).await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "hello from the export test");

    let agents = target.list_agents(&summary.workspace_id).await.unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].name, "Cow-Coder");

    let tasks = target.list_tasks(&summary.workspace_id).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(matches!(tasks[0].status, TaskStatus::Pending));
}

#[tokio::test]
async fn memory_notes_and_settings_round_trip_with_redaction() {
    let source = test_store().await;
    let workspace = seed(&source).await;
    let mut agents = source.list_agents(&workspace.id).await.unwrap();
    let agent = agents.remove(0);
    let mut channels = source.list_channels(&workspace.id).await.unwrap();
    let channel = channels.remove(0);

    source
        .save_note(&Note::workspace(
            workspace.id.clone(),
            "Workspace policy: be brief.",
        ))
        .await
        .unwrap();
    source
        .save_note(&Note::conversation(
            workspace.id.clone(),
            channel.id.clone(),
            "Use the design doc. api_key = sk-live-123456",
        ))
        .await
        .unwrap();

    let mut entry = MemoryEntry::new(
        workspace.id.clone(),
        Some(agent.id.clone()),
        Some(channel.id.clone()),
        MemoryKind::Preference,
        "The user prefers Rust. password: hunter2",
        MemorySource::User,
    );
    entry.pinned = true;
    entry.recount();
    source.insert_memory_entry(&entry).await.unwrap();
    let mut pending = MemoryEntry::candidate(
        workspace.id.clone(),
        None,
        Some(channel.id.clone()),
        MemoryKind::Fact,
        "Pending candidate.",
    );
    pending.recount();
    source.insert_memory_entry(&pending).await.unwrap();
    source
        .update_memory_settings(&MemorySettings::new(workspace.id.clone()).with_budgets(500, 900))
        .await
        .unwrap();

    let bytes = export_workspace(&source, &workspace.id).await.unwrap();
    let target = test_store().await;
    let summary = import_workspace(&target, &bytes).await.unwrap();
    assert_eq!(summary.memory_notes, 2);
    assert_eq!(summary.memory_entries, 2);

    let notes = target.list_all_notes(&summary.workspace_id).await.unwrap();
    assert_eq!(notes.len(), 2);
    assert!(notes.iter().all(|note| !note.content.contains("sk-live")));
    let workspace_note = target
        .get_note(&summary.workspace_id, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(workspace_note.content, "Workspace policy: be brief.");
    let conversation_note = target
        .get_note(&summary.workspace_id, Some(&channel.id))
        .await
        .unwrap()
        .unwrap();
    assert!(conversation_note.content.contains("[redacted]"));
    assert_eq!(conversation_note.workspace_id, summary.workspace_id);

    let entries = target
        .list_all_memory_entries(&summary.workspace_id)
        .await
        .unwrap();
    assert_eq!(entries.len(), 2);
    let imported = entries
        .iter()
        .find(|entry| entry.kind == MemoryKind::Preference)
        .expect("the active preference is imported");
    assert_eq!(imported.agent_id.as_ref(), Some(&agent.id));
    assert_eq!(imported.channel_id.as_ref(), Some(&channel.id));
    assert!(imported.pinned);
    assert!(imported.content.contains("[redacted]"));
    assert!(!imported.content.contains("hunter2"));
    assert_ne!(imported.id, entry.id);
    assert_eq!(imported.status, MemoryStatus::Active);
    let imported_pending = entries
        .iter()
        .find(|entry| entry.status == MemoryStatus::Pending)
        .expect("the pending candidate is imported");
    assert!(imported_pending.agent_id.is_none());

    let settings = target
        .get_memory_settings(&summary.workspace_id)
        .await
        .unwrap();
    assert_eq!(settings.notes_budget_tokens, 500);
    assert_eq!(settings.recall_budget_tokens, 900);
}

#[tokio::test]
async fn import_reattributes_audit_events_and_preserves_archived_tasks() {
    let source = test_store().await;
    let workspace = seed(&source).await;
    let event = AuditEvent::new(workspace.id.clone(), "tester", "test.action", "target");
    source.append_audit_event(&event).await.unwrap();

    let mut task = Task::new("Archived task", "Output");
    task.status = TaskStatus::Completed {
        output: "done".to_string(),
    };
    task.archived = true;
    source
        .create_task(&workspace.id, None, &task)
        .await
        .unwrap();

    let bytes = export_workspace(&source, &workspace.id).await.unwrap();
    let target = test_store().await;
    let summary = import_workspace(&target, &bytes).await.unwrap();
    assert_eq!(summary.audit_events, 1);

    let events = target
        .list_audit_events(&summary.workspace_id, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].workspace_id, summary.workspace_id);
    assert_ne!(events[0].id, event.id);

    let tasks = target.list_tasks(&summary.workspace_id).await.unwrap();
    let imported = tasks
        .iter()
        .find(|candidate| candidate.id == task.id)
        .expect("imported archived task");
    assert!(imported.archived);
}

#[tokio::test]
async fn import_creates_a_new_workspace_and_leaves_existing_ones_alone() {
    let source = test_store().await;
    let workspace = seed(&source).await;
    let bytes = export_workspace(&source, &workspace.id).await.unwrap();

    let target = test_store().await;
    seed(&target).await;
    let before = target.list_workspaces().await.unwrap().len();

    let summary = import_workspace(&target, &bytes).await.unwrap();
    let after = target.list_workspaces().await.unwrap();

    assert_eq!(after.len(), before + 1);
    assert_ne!(summary.workspace_id, workspace.id);
    assert!(after.iter().any(|ws| ws.id == summary.workspace_id));
}

#[tokio::test]
async fn tampered_payload_is_rejected() {
    let source = test_store().await;
    let workspace = seed(&source).await;
    let mut bytes = export_workspace(&source, &workspace.id).await.unwrap();

    // Flip a byte in the compressed body; the checksum guard must catch it.
    let midpoint = bytes.len() / 2;
    bytes[midpoint] ^= 0xff;

    let target = test_store().await;
    let result = import_workspace(&target, &bytes).await;
    assert!(result.is_err(), "tampered package must not import");
    assert!(target.list_workspaces().await.unwrap().is_empty());
}

#[tokio::test]
async fn non_zip_input_is_rejected() {
    let target = test_store().await;
    let result = import_workspace(&target, b"not a zip archive").await;
    assert!(matches!(result, Err(PackageError::Zip(_))));
}

#[tokio::test]
async fn a_second_import_of_the_same_package_is_refused_before_it_writes() {
    // Package entity ids travel with the package, so a second import would
    // either take over the first workspace's rows or leave the new workspace
    // naming agents and channels it does not own. It must be refused while the
    // database is still untouched.
    let source = test_store().await;
    let workspace = seed(&source).await;
    let bytes = export_workspace(&source, &workspace.id).await.unwrap();

    let target = test_store().await;
    let first = import_workspace(&target, &bytes).await.unwrap();
    let workspaces_before = target.list_workspaces().await.unwrap().len();
    let agents_before = target.list_agents(&first.workspace_id).await.unwrap().len();
    assert!(agents_before > 0);

    let error = import_workspace(&target, &bytes).await.unwrap_err();
    assert!(
        error.to_string().contains("already exist"),
        "unexpected error: {error}"
    );

    // Nothing was written: no second workspace, no orphaned rows, and the
    // first import is intact.
    assert_eq!(
        target.list_workspaces().await.unwrap().len(),
        workspaces_before
    );
    assert_eq!(
        target.list_agents(&first.workspace_id).await.unwrap().len(),
        agents_before
    );
}

#[tokio::test]
async fn importing_into_the_database_the_workspace_came_from_is_refused() {
    // The CLI imports with `--db`, which may be the database the package was
    // exported from. Ids collide there too, and the export source must survive
    // unchanged.
    let source = test_store().await;
    let workspace = seed(&source).await;
    let bytes = export_workspace(&source, &workspace.id).await.unwrap();
    let agents_before = source.list_agents(&workspace.id).await.unwrap().len();
    let workspaces_before = source.list_workspaces().await.unwrap().len();

    let error = import_workspace(&source, &bytes).await.unwrap_err();
    assert!(
        error.to_string().contains("already exist"),
        "unexpected error: {error}"
    );
    assert_eq!(
        source.list_workspaces().await.unwrap().len(),
        workspaces_before
    );
    assert_eq!(
        source.list_agents(&workspace.id).await.unwrap().len(),
        agents_before
    );
}

#[tokio::test]
async fn conversation_context_round_trips() {
    let source = test_store().await;
    let workspace = seed(&source).await;
    let agent = source
        .list_agents(&workspace.id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();

    let channel = Channel::conversation("Design review", "Conversation with Cow-Coder");
    let settings = ConversationSettings::new(
        channel.id.clone(),
        Some("provider-1".to_string()),
        "mock-model",
    );
    source
        .create_conversation(
            &workspace.id,
            &channel,
            &settings,
            std::slice::from_ref(&agent.id),
        )
        .await
        .unwrap();

    let mut user = ContextEntry::new(
        channel.id.clone(),
        agent.id.clone(),
        ContextRole::User,
        "hello there",
    );
    user.recount();
    source.append_context_entry(&user).await.unwrap();
    let mut assistant = ContextEntry::new(
        channel.id.clone(),
        agent.id.clone(),
        ContextRole::Assistant,
        "hello back",
    );
    assistant.recount();
    source.append_context_entry(&assistant).await.unwrap();
    let event = ContextEvent::new(
        channel.id.clone(),
        agent.id.clone(),
        ContextEventKind::Clear,
        10,
        0,
    );
    source.record_context_event(&event).await.unwrap();

    let bytes = export_workspace(&source, &workspace.id).await.unwrap();
    let target = test_store().await;
    let summary = import_workspace(&target, &bytes).await.unwrap();
    assert_eq!(summary.conversations, 1);
    assert_eq!(summary.context_entries, 2);

    let imported = target
        .get_channel(&channel.id)
        .await
        .unwrap()
        .expect("conversation channel should be imported");
    assert!(imported.is_conversation());
    let imported_settings = target
        .get_conversation_settings(&channel.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(imported_settings.model, "mock-model");
    assert_eq!(imported_settings.provider_id.as_deref(), Some("provider-1"));

    let participants = target.list_channel_participants(&channel.id).await.unwrap();
    assert_eq!(participants.len(), 1);
    assert_eq!(participants[0].agent_id, agent.id);

    let entries = target
        .list_context_entries(&channel.id, &agent.id)
        .await
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].content, "hello there");
    assert_eq!(entries[1].content, "hello back");
    let events = target
        .list_context_events(&channel.id, &agent.id, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
}

#[tokio::test]
async fn group_conversation_round_trips_positions_and_authored_entries() {
    let source = test_store().await;
    let workspace = seed(&source).await;
    let seeded = source
        .list_agents(&workspace.id)
        .await
        .unwrap()
        .into_iter()
        .find(|agent| agent.name == "Cow-Coder")
        .unwrap();
    let planner = AgentProfile::new("Planner", "Architect", "Plan", "Careful");
    let reviewer = AgentProfile::new("Reviewer", "Reviewer", "Review", "Careful");
    for agent in [&planner, &reviewer] {
        source.upsert_agent(&workspace.id, agent).await.unwrap();
    }
    let participants = vec![seeded.id.clone(), planner.id.clone(), reviewer.id.clone()];

    let channel = Channel::conversation("Crew", "Group conversation");
    let settings = ConversationSettings::group(
        channel.id.clone(),
        Some("provider-1".to_string()),
        "mock-model",
        Some(planner.id.clone()),
    );
    source
        .create_conversation(&workspace.id, &channel, &settings, &participants)
        .await
        .unwrap();

    let mut peer = ContextEntry::peer(
        channel.id.clone(),
        seeded.id.clone(),
        planner.id.clone(),
        "Planner",
        "The draft is ready.",
    );
    peer.recount();
    source.append_context_entry(&peer).await.unwrap();

    // Turn rows stay device-local and must never enter a package.
    let message = ChannelMessage::new(
        channel.id.clone(),
        Sender::User("owner".to_string()),
        "@team status",
    );
    source.append_message(&message).await.unwrap();
    let mut turn =
        ConversationTurn::new(channel.id.clone(), message.id.clone(), RouteSource::Mention);
    turn.selected_agents = participants.clone();
    source.insert_conversation_turn(&turn).await.unwrap();

    let bytes = export_workspace(&source, &workspace.id).await.unwrap();
    let manifest = inspect(&bytes).unwrap();
    assert!(
        !manifest
            .checksums
            .keys()
            .any(|path| path.contains("conversation_turns")),
        "turn rows are device-local and must not be packaged"
    );

    let target = test_store().await;
    import_workspace(&target, &bytes).await.unwrap();

    let stored = target.list_channel_participants(&channel.id).await.unwrap();
    assert_eq!(
        stored
            .iter()
            .map(|participant| participant.agent_id.clone())
            .collect::<Vec<_>>(),
        participants
    );
    assert_eq!(
        stored
            .iter()
            .map(|participant| participant.position)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );

    let stored_settings = target
        .get_conversation_settings(&channel.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored_settings.mode, ConversationMode::Group);
    assert_eq!(
        stored_settings.default_responder.as_ref(),
        Some(&planner.id)
    );

    let entries = target
        .list_context_entries(&channel.id, &seeded.id)
        .await
        .unwrap();
    let authored = entries
        .iter()
        .find(|entry| entry.author_agent_id.is_some())
        .expect("authored peer entry should round trip");
    assert_eq!(authored.author_agent_id.as_ref(), Some(&planner.id));
    assert_eq!(authored.content, "[Planner] The draft is ready.");

    assert!(target
        .list_conversation_turns(&channel.id, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn manifest_checksums_cover_every_content_file() {
    let source = test_store().await;
    let workspace = seed(&source).await;
    let bytes = export_workspace(&source, &workspace.id).await.unwrap();
    let manifest = inspect(&bytes).unwrap();

    for path in [
        "workspace.json",
        "agents.json",
        "templates.json",
        "channels.jsonl",
        "messages.jsonl",
        "channel_participants.jsonl",
        "conversation_settings.jsonl",
        "context_entries.jsonl",
        "context_events.jsonl",
        "tasks.json",
        "runs.jsonl",
        "audit.jsonl",
        "permission_rules.jsonl",
    ] {
        assert!(
            manifest.checksums.contains_key(path),
            "manifest is missing a checksum for {path}"
        );
    }
}

/// Rebuild an archive with one extra file, keeping every original entry.
fn append_zip_entry(bytes: &[u8], path: &str, contents: &[u8]) -> Vec<u8> {
    let mut source = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut target = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for index in 0..source.len() {
        let mut file = source.by_index(index).unwrap();
        let name = file.name().unwrap().to_string();
        let mut buffer = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut buffer).unwrap();
        target.start_file(name, options).unwrap();
        std::io::Write::write_all(&mut target, &buffer).unwrap();
    }
    target.start_file(path, options).unwrap();
    std::io::Write::write_all(&mut target, contents).unwrap();
    target.finish().unwrap().into_inner()
}

#[tokio::test]
async fn prompt_snapshots_are_excluded_from_workspace_packages() {
    use cowbot_core::prompt::PromptRole;
    use cowbot_core::prompt_snapshot::{LlmPromptSnapshot, SnapshotMessage};
    use cowbot_core::run::Run;
    use cowbot_core::usage::{LlmCallKind, LlmCallRecord, ESTIMATE_COUNTER};

    let source = test_store().await;
    let workspace = seed(&source).await;
    let agent = source
        .list_agents(&workspace.id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let task = Task::new("Snapshot task", "Output").assign_to(agent.id.clone());
    source
        .create_task(&workspace.id, None, &task)
        .await
        .unwrap();
    let run = Run::new(task.id.clone(), agent.id.clone());
    source.create_run(&run).await.unwrap();
    let mut call = LlmCallRecord::new(LlmCallKind::Agent, "mock-model");
    call.run_id = Some(run.id.clone());
    call.task_id = Some(task.id.clone());
    source.record_llm_call(&call).await.unwrap();
    let snapshot = LlmPromptSnapshot::new(LlmCallKind::Agent, "mock-model")
        .with_messages(vec![SnapshotMessage::new(
            PromptRole::System,
            "SNAPSHOT-ONLY-CONTENT",
        )])
        .seal(&ESTIMATE_COUNTER);
    source
        .record_prompt_snapshot(&workspace.id, &call.id, &snapshot)
        .await
        .unwrap();
    assert!(source
        .get_prompt_snapshot(&call.id)
        .await
        .unwrap()
        .is_some());

    let bytes = export_workspace(&source, &workspace.id).await.unwrap();
    let manifest = inspect(&bytes).unwrap();
    assert!(
        manifest
            .checksums
            .keys()
            .all(|path| !path.contains("prompt")),
        "no snapshot file may be packaged: {:?}",
        manifest.checksums.keys().collect::<Vec<_>>()
    );

    // A package round trip still works, and the target has no snapshots.
    let target = test_store().await;
    let summary = import_workspace(&target, &bytes).await.unwrap();
    assert!(target
        .list_recent_prompt_snapshots(&summary.workspace_id, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_package_carrying_prompt_snapshots_is_rejected() {
    let source = test_store().await;
    let workspace = seed(&source).await;
    let bytes = export_workspace(&source, &workspace.id).await.unwrap();
    let tampered = append_zip_entry(&bytes, "prompt_snapshots.json", b"[]");

    let target = test_store().await;
    let result = import_workspace(&target, &tampered).await;
    assert!(matches!(result, Err(PackageError::UnexpectedFile { .. })));
    assert!(target.list_workspaces().await.unwrap().is_empty());
}
