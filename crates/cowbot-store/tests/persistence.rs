use chrono::{Duration as ChronoDuration, Utc};
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::approval::Approval;
use cowbot_core::audit::AuditEvent;
use cowbot_core::channel::{Channel, ChannelMessage, Sender};
use cowbot_core::identity::{
    ApiToken, Invite, Membership, Role, Session, User, Workspace, WorkspaceId,
};
use cowbot_core::run::{Run, RunStatus, ToolCallRecord};
use cowbot_core::task::{Task, TaskStatus};
use cowbot_core::template::{Template, TemplateKind};
use cowbot_store::Store;
use serde_json::json;

async fn test_store() -> Store {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    store
}

async fn seed_workspace(store: &Store) -> (User, Workspace) {
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
    (user, workspace)
}

#[tokio::test]
async fn identity_and_workspace_roundtrip() {
    let store = test_store().await;
    let (user, workspace) = seed_workspace(&store).await;

    let loaded = store.get_user(&user.id).await.unwrap().unwrap();
    assert_eq!(loaded.email, "alice@example.com");

    let by_email = store
        .get_user_by_email("alice@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_email.id, user.id);

    let membership = store
        .get_membership(&user.id, &workspace.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(membership.role, Role::Owner);

    let members = store.list_members(&workspace.id).await.unwrap();
    assert_eq!(members.len(), 1);

    let token_hash = "session-token-hash";
    let session = Session::new(
        user.id.clone(),
        token_hash,
        chrono::Utc::now() + chrono::Duration::hours(1),
    );
    store.create_session(&session).await.unwrap();
    let loaded_session = store
        .get_session_by_token_hash(token_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded_session.user_id, user.id);
    store.delete_session(&session.id).await.unwrap();
    assert!(store
        .get_session_by_token_hash(token_hash)
        .await
        .unwrap()
        .is_none());

    let api_token = ApiToken::new(
        user.id.clone(),
        workspace.id.clone(),
        "api-token-hash",
        vec!["tasks:read".to_string()],
    );
    store.create_api_token(&api_token).await.unwrap();
    let loaded_token = store
        .get_api_token_by_hash("api-token-hash")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded_token.scopes, vec!["tasks:read"]);
    store.revoke_api_token(&api_token.id).await.unwrap();
    let revoked_token = store
        .get_api_token_by_hash("api-token-hash")
        .await
        .unwrap()
        .unwrap();
    assert!(revoked_token.revoked_at.is_some());

    let invite = Invite::new(
        workspace.id.clone(),
        "invite-token-hash",
        Role::Member,
        chrono::Utc::now() + chrono::Duration::days(1),
    );
    store.create_invite(&invite).await.unwrap();
    let loaded_invite = store
        .find_invite_by_token_hash("invite-token-hash")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded_invite.role, Role::Member);
    store.delete_invite(&invite.id).await.unwrap();
}

#[tokio::test]
async fn collaboration_roundtrip() {
    let store = test_store().await;
    let (user, workspace) = seed_workspace(&store).await;

    let channel = Channel::new("general", "Team coordination");
    store.create_channel(&workspace.id, &channel).await.unwrap();

    let user_message = ChannelMessage::new(
        channel.id.clone(),
        Sender::User(user.id.0.clone()),
        "Start the task",
    );
    store.append_message(&user_message).await.unwrap();

    let agent_message = ChannelMessage::new(
        channel.id.clone(),
        Sender::Agent(AgentId::new("agent-1")),
        "Working on it",
    )
    .with_thread(user_message.id.clone());
    store.append_message(&agent_message).await.unwrap();

    let messages = store.list_messages(&channel.id, 10).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[0].sender, Sender::User(_)));
    assert!(matches!(messages[1].sender, Sender::Agent(_)));
    assert_eq!(messages[1].thread_id, Some(user_message.id.clone()));

    let loaded = store.get_message(&agent_message.id).await.unwrap().unwrap();
    assert_eq!(loaded.content, "Working on it");

    let channels = store.list_channels(&workspace.id).await.unwrap();
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].name, "general");
}

#[tokio::test]
async fn channel_management_roundtrip() {
    let store = test_store().await;
    let (_user, workspace) = seed_workspace(&store).await;

    let channel = Channel::new("general", "Team coordination");
    store.create_channel(&workspace.id, &channel).await.unwrap();

    assert!(store
        .channel_name_taken(&workspace.id, "GENERAL", None)
        .await
        .unwrap());
    assert!(!store
        .channel_name_taken(&workspace.id, "GENERAL", Some(&channel.id))
        .await
        .unwrap());

    store.set_channel_pinned(&channel.id, true).await.unwrap();
    let pinned = store.get_channel(&channel.id).await.unwrap().unwrap();
    assert!(pinned.pinned);

    store
        .rename_channel(&channel.id, "ops", "Operations")
        .await
        .unwrap();
    let renamed = store.get_channel(&channel.id).await.unwrap().unwrap();
    assert_eq!(renamed.name, "ops");
    assert_eq!(renamed.topic, "Operations");
    assert!(renamed.pinned);
    assert!(store
        .channel_name_taken(&workspace.id, "ops", None)
        .await
        .unwrap());
    assert!(!store
        .channel_name_taken(&workspace.id, "ops", Some(&channel.id))
        .await
        .unwrap());

    let message = ChannelMessage::new(channel.id.clone(), Sender::System, "hello");
    store.append_message(&message).await.unwrap();
    assert_eq!(store.clear_channel_messages(&channel.id).await.unwrap(), 1);
    assert!(store
        .list_messages(&channel.id, 10)
        .await
        .unwrap()
        .is_empty());

    store.append_message(&message).await.unwrap();
    store.delete_channel(&channel.id).await.unwrap();
    assert!(store.get_channel(&channel.id).await.unwrap().is_none());
    // Channel deletion cascades to messages.
    assert!(store
        .list_messages(&channel.id, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn direct_channels_are_unique_per_agent() {
    let store = test_store().await;
    let (_user, workspace) = seed_workspace(&store).await;
    let agent = AgentProfile::new("Planner", "Architect", "Plan", "Careful");
    store.upsert_agent(&workspace.id, &agent).await.unwrap();

    let channel = Channel::direct(&agent, "Architect");
    store.create_channel(&workspace.id, &channel).await.unwrap();

    let loaded = store
        .get_channel_for_agent(&workspace.id, &agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.id, channel.id);
    assert_eq!(loaded.agent_id.as_ref(), Some(&agent.id));

    let duplicate = Channel::direct(&agent, "Architect");
    assert!(store
        .create_channel(&workspace.id, &duplicate)
        .await
        .is_err());
}

#[tokio::test]
async fn list_messages_returns_the_latest_window_in_chronological_order() {
    let store = test_store().await;
    let (_user, workspace) = seed_workspace(&store).await;

    let channel = Channel::new("general", "Team coordination");
    store.create_channel(&workspace.id, &channel).await.unwrap();

    let base = Utc::now();
    let mut sent = Vec::new();
    for index in 0..3 {
        let mut message = ChannelMessage::new(
            channel.id.clone(),
            Sender::System,
            format!("message {index}"),
        );
        message.created_at = base + ChronoDuration::seconds(index);
        store.append_message(&message).await.unwrap();
        sent.push(message);
    }

    let messages = store.list_messages(&channel.id, 2).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].id, sent[1].id);
    assert_eq!(messages[1].id, sent[2].id);
}

#[tokio::test]
async fn agents_and_templates_roundtrip() {
    let store = test_store().await;
    let (_user, workspace) = seed_workspace(&store).await;

    let mut agent = cowbot_core::agent::AgentProfile::new(
        "Cow-Coder",
        "Senior Engineer",
        "Write and verify code",
        "Pragmatic and test-driven",
    )
    .with_model("gpt-4o")
    .with_tool("execute_code")
    .with_tool("read_file");
    agent.max_iterations = 20;
    store.upsert_agent(&workspace.id, &agent).await.unwrap();

    let loaded = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(loaded.model.as_deref(), Some("gpt-4o"));
    assert_eq!(loaded.tools, vec!["execute_code", "read_file"]);
    assert_eq!(loaded.max_iterations, 20);

    agent.name = "Cow-Coder v2".to_string();
    store.upsert_agent(&workspace.id, &agent).await.unwrap();
    let loaded = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(loaded.name, "Cow-Coder v2");

    let agents = store.list_agents(&workspace.id).await.unwrap();
    assert_eq!(agents.len(), 1);

    let mut template = Template::new(
        TemplateKind::Agent,
        "Reviewer",
        json!({ "role": "reviewer", "tools": ["read_file"] }),
    );
    template.description = "Code review agent".to_string();
    store.upsert_template(&template).await.unwrap();

    let loaded_template = store.get_template(&template.id).await.unwrap().unwrap();
    assert_eq!(loaded_template.kind, TemplateKind::Agent);
    assert_eq!(loaded_template.payload["role"], "reviewer");

    let templates = store
        .list_templates(Some(TemplateKind::Agent))
        .await
        .unwrap();
    assert_eq!(templates.len(), 1);

    store.delete_template(&template.id).await.unwrap();
    assert!(store.get_template(&template.id).await.unwrap().is_none());

    store.delete_agent(&agent.id).await.unwrap();
    assert!(store.get_agent(&agent.id).await.unwrap().is_none());
}

#[tokio::test]
async fn tasks_runs_approvals_and_audit_roundtrip() {
    let store = test_store().await;
    let (user, workspace) = seed_workspace(&store).await;
    let channel = Channel::new("engineering", "Engineering work");
    store.create_channel(&workspace.id, &channel).await.unwrap();

    let agent_id = AgentId::new("agent-coder");
    let task = Task::new("Implement login", "Working login endpoint").assign_to(agent_id.clone());
    store
        .create_task(&workspace.id, Some(&channel.id), &task)
        .await
        .unwrap();

    let loaded_task = store.get_task(&task.id).await.unwrap().unwrap();
    assert_eq!(loaded_task.description, "Implement login");
    assert_eq!(loaded_task.assigned_agent, Some(agent_id.clone()));

    store
        .update_task_status(
            &task.id,
            &TaskStatus::Completed {
                output: "Login endpoint implemented".to_string(),
            },
        )
        .await
        .unwrap();
    let completed = store.get_task(&task.id).await.unwrap().unwrap();
    assert!(matches!(completed.status, TaskStatus::Completed { .. }));

    let run = Run::new(task.id.clone(), agent_id.clone());
    store.create_run(&run).await.unwrap();
    store
        .update_run_status(&run.id, RunStatus::Completed, Some(chrono::Utc::now()))
        .await
        .unwrap();
    let loaded_run = store.get_run(&run.id).await.unwrap().unwrap();
    assert_eq!(loaded_run.status, RunStatus::Completed);
    assert!(loaded_run.finished_at.is_some());
    assert_eq!(store.list_runs(&task.id).await.unwrap().len(), 1);

    let tool_call = ToolCallRecord::new(
        run.id.clone(),
        "execute_code",
        json!({ "command": "cargo test" }),
    );
    store.record_tool_call(&tool_call).await.unwrap();
    store
        .update_tool_call_result(
            &tool_call.id,
            &json!({ "success": true, "output": "ok" }),
            Some(&user.id.0),
        )
        .await
        .unwrap();
    let tool_calls = store.list_tool_calls(&run.id).await.unwrap();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(
        tool_calls[0].approved_by.as_deref(),
        Some(user.id.0.as_str())
    );

    let approval = Approval::new(task.id.clone(), tool_call.id.clone());
    store.create_approval(&approval).await.unwrap();
    let pending = store.list_pending_approvals(&task.id).await.unwrap();
    assert_eq!(pending.len(), 1);
    store
        .decide_approval(
            &approval.id,
            cowbot_core::approval::ApprovalStatus::Approved,
            &user.id.0,
        )
        .await
        .unwrap();
    let decided = store.get_approval(&approval.id).await.unwrap().unwrap();
    assert_eq!(
        decided.status,
        cowbot_core::approval::ApprovalStatus::Approved
    );
    assert_eq!(decided.decided_by.as_deref(), Some(user.id.0.as_str()));

    let workspace_id: WorkspaceId = workspace.id.clone();
    let event = AuditEvent::new(
        workspace_id.clone(),
        user.id.0.clone(),
        "approval.approved",
        approval.id.0.clone(),
    );
    store.append_audit_event(&event).await.unwrap();
    let events = store.list_audit_events(&workspace_id, 10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, "approval.approved");
}

#[tokio::test]
async fn task_title_and_archive_roundtrip() {
    let store = test_store().await;
    let (_user, workspace) = seed_workspace(&store).await;

    let task = Task::new("First line\nSecond line", "Output").with_title("Short title");
    store.create_task(&workspace.id, None, &task).await.unwrap();

    let loaded = store.get_task(&task.id).await.unwrap().unwrap();
    assert_eq!(loaded.title, "Short title");
    assert!(!loaded.archived);

    store.set_task_archived(&task.id, true).await.unwrap();
    let archived = store.get_task(&task.id).await.unwrap().unwrap();
    assert!(archived.archived);

    store.set_task_archived(&task.id, false).await.unwrap();
    let restored = store.get_task(&task.id).await.unwrap().unwrap();
    assert!(!restored.archived);
}
