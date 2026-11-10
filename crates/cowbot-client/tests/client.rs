use std::sync::Arc;

use cowbot_agent::{AgentInstance, Crew, StandardRoles};
use cowbot_client::{Client, LocalClient, NewTask};
use cowbot_core::agent::AgentId;
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::assignment::AssignmentStatus;
use cowbot_core::channel::Sender;
use cowbot_core::event::EventPayload;
use cowbot_core::run::RunStatus;
use cowbot_core::task::{Task, TaskStatus};
use cowbot_core::template::TemplateDocument;
use cowbot_llm::MockLlmProvider;
use cowbot_tools::ToolRegistry;

#[tokio::test]
async fn local_client_bootstraps_a_session() {
    let client = LocalClient::open_in_memory().await.unwrap();

    let session = client.session().await.unwrap();
    assert_eq!(session.user.name, "Local Owner");
    assert_eq!(session.user.email, "local@cowbot.local");
    assert_eq!(session.workspace.name, "Default Workspace");
    assert!(client.list_channels().await.unwrap().is_empty());
}

#[tokio::test]
async fn local_client_updates_the_user_profile() {
    let client = LocalClient::open_in_memory().await.unwrap();

    let updated = client
        .update_user_profile("  Alice  ", " ALICE@Example.com ")
        .await
        .unwrap();
    assert_eq!(updated.user.name, "Alice");
    assert_eq!(updated.user.email, "alice@example.com");

    let session = client.session().await.unwrap();
    assert_eq!(session.user.name, "Alice");
    assert_eq!(session.user.email, "alice@example.com");
    assert_eq!(session.workspace.name, "Default Workspace");

    assert!(client
        .update_user_profile("", "alice@example.com")
        .await
        .is_err());
    assert!(client
        .update_user_profile("Alice", "not-an-email")
        .await
        .is_err());
}

#[tokio::test]
async fn local_client_manages_channels_and_messages() {
    let client = LocalClient::open_in_memory().await.unwrap();
    let session = client.session().await.unwrap();

    let channel = client
        .create_channel("engineering", "Engineering work")
        .await
        .unwrap();

    let channels = client.list_channels().await.unwrap();
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].id, channel.id);

    let question = client
        .post_message(&channel.id, "Implement the login flow")
        .await
        .unwrap();
    match &question.sender {
        Sender::User(user_id) => assert_eq!(user_id, &session.user.id.0),
        other => panic!("expected the local user sender, got {other:?}"),
    }

    let reply = client
        .post_thread_message(&channel.id, "Working on it", &question.id)
        .await
        .unwrap();
    assert_eq!(reply.thread_id.as_deref(), Some(question.id.as_str()));

    let messages = client.list_messages(&channel.id, 10).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content, "Implement the login flow");
    assert_eq!(messages[1].thread_id.as_deref(), Some(question.id.as_str()));
}

#[tokio::test]
async fn local_client_streams_events() {
    let client = LocalClient::open_in_memory().await.unwrap();
    let mut events = client.subscribe();

    let channel = client
        .create_channel("alerts", "Deployment alerts")
        .await
        .unwrap();
    match events.recv().await.unwrap().payload {
        EventPayload::ChannelCreated(created) => assert_eq!(created.id, channel.id),
        other => panic!("expected ChannelCreated, got {other:?}"),
    }

    let message = client.post_message(&channel.id, "hello").await.unwrap();
    match events.recv().await.unwrap().payload {
        EventPayload::MessagePosted(posted) => assert_eq!(posted.id, message.id),
        other => panic!("expected MessagePosted, got {other:?}"),
    }
}

#[tokio::test]
async fn local_client_creates_and_cancels_tasks() {
    let client = LocalClient::open_in_memory().await.unwrap();
    let agent = AgentId::new("agent-coder");

    let task = client
        .create_task(&NewTask::new("Implement login", "Working code", agent))
        .await
        .unwrap();
    assert_eq!(task.status, TaskStatus::Pending);

    let listed = client.list_tasks().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, task.id);

    let assignments = client.list_assignments(&task.id).await.unwrap();
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].status, AssignmentStatus::Queued);

    client
        .cancel_task(&task.id, "no longer needed")
        .await
        .unwrap();

    let stored = client.get_task(&task.id).await.unwrap().unwrap();
    assert_eq!(stored.status, TaskStatus::Cancelled);
    let assignments = client.list_assignments(&task.id).await.unwrap();
    assert_eq!(assignments[0].status, AssignmentStatus::Cancelled);
}

#[tokio::test]
async fn local_client_lists_and_instantiates_templates() {
    let client = LocalClient::open_in_memory().await.unwrap();

    let templates = client.list_templates().await.unwrap();
    let ids: Vec<&str> = templates
        .iter()
        .map(|template| template.id.as_str())
        .collect();
    assert!(ids.contains(&"builtin-software-team"));
    assert!(ids.contains(&"builtin-research-team"));
    assert!(ids.contains(&"builtin-general-assistant"));

    let agents = client
        .instantiate_agents("builtin-software-team")
        .await
        .unwrap();
    assert_eq!(agents.len(), 3);
    let names: Vec<&str> = agents.iter().map(|agent| agent.name.as_str()).collect();
    assert_eq!(names, vec!["Planner", "Coder", "Reviewer"]);

    let stored = client.list_agents().await.unwrap();
    assert_eq!(stored.len(), 3);
    let stored_names: Vec<&str> = stored.iter().map(|agent| agent.name.as_str()).collect();
    for expected in ["Planner", "Coder", "Reviewer"] {
        assert!(stored_names.contains(&expected));
    }
}

#[tokio::test]
async fn local_client_manages_custom_templates() {
    let client = LocalClient::open_in_memory().await.unwrap();

    let exported = client
        .export_template("builtin-general-assistant")
        .await
        .unwrap();
    let mut document: TemplateDocument = serde_json::from_str(&exported).unwrap();
    document.id = "custom-assistant".to_string();
    document.name = "Custom Assistant".to_string();

    let json = serde_json::to_string(&document).unwrap();
    let imported = client.import_template(&json).await.unwrap();
    assert_eq!(imported.id, "custom-assistant");

    let fetched = client
        .get_template("custom-assistant")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.name, "Custom Assistant");

    let listed = client.list_templates().await.unwrap();
    assert!(listed
        .iter()
        .any(|template| template.id == "custom-assistant"));

    client.delete_template("custom-assistant").await.unwrap();
    assert!(client
        .get_template("custom-assistant")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn local_client_runs_a_crew_and_persists_history() {
    let client = LocalClient::open_in_memory().await.unwrap();

    let profile = StandardRoles::planner();
    let agent_id = profile.id.clone();
    let mut crew = Crew::new();
    crew.add_agent(AgentInstance::new(
        profile,
        Arc::new(MockLlmProvider::echo()),
        ToolRegistry::new(),
        "mock-model",
    ));
    let task = Task::new("Plan the change", "A plan").assign_to(agent_id);
    let task_id = task.id.clone();
    crew.add_task(task);

    let results = client
        .run_crew(&mut crew, ApprovalLevel::Ask)
        .await
        .unwrap();
    assert!(results.contains_key(&task_id));

    let stored = client.get_task(&task_id).await.unwrap().unwrap();
    assert!(matches!(stored.status, TaskStatus::Completed { .. }));

    let assignments = client.list_assignments(&task_id).await.unwrap();
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].status, AssignmentStatus::Completed);

    let runs = client.list_runs(&task_id).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Completed);
}
