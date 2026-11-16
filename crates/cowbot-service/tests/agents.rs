use cowbot_bus::EventBus;
use cowbot_core::agent::AgentProfile;
use cowbot_service::{bootstrap_local, AgentService, AgentUpdate, ChannelService};
use cowbot_store::Store;

async fn setup() -> (
    Store,
    AgentService,
    ChannelService,
    cowbot_core::identity::WorkspaceId,
) {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let bus = EventBus::default_bus();
    let context = bootstrap_local(&store).await.unwrap();
    let channels = ChannelService::new(store.clone(), bus.clone());
    let agents = AgentService::new(store.clone(), bus);
    (store, agents, channels, context.workspace.id)
}

#[tokio::test]
async fn update_agent_renames_profile_and_conversation() {
    let (store, agents, channels, workspace) = setup().await;
    let agent = AgentProfile::new("Assistant", "Generalist", "Help", "Careful");
    store.upsert_agent(&workspace, &agent).await.unwrap();
    let direct = channels
        .ensure_agent_channel(&workspace, &agent)
        .await
        .unwrap();

    let updated = agents
        .update_agent(
            &workspace,
            &AgentUpdate {
                id: agent.id.clone(),
                name: "main".to_string(),
                role: "Primary Assistant".to_string(),
                tools: vec![
                    "read_file".to_string(),
                    "create_channel".to_string(),
                    "read_file".to_string(),
                ],
            },
        )
        .await
        .unwrap();

    assert_eq!(updated.name, "main");
    assert_eq!(updated.role, "Primary Assistant");
    assert_eq!(updated.tools, vec!["read_file", "create_channel"]);

    let channels = channels.list_channels(&workspace).await.unwrap();
    let conversation = channels
        .iter()
        .find(|channel| channel.id == direct.id)
        .unwrap();
    assert_eq!(conversation.name, "main");
    assert_eq!(conversation.topic, "Primary Assistant");
}

#[tokio::test]
async fn duplicate_agent_names_are_rejected_case_insensitively() {
    let (store, agents, _channels, workspace) = setup().await;
    let first = AgentProfile::new("main", "Primary Assistant", "Run", "Lead");
    let second = AgentProfile::new("Writer", "Technical Writer", "Write", "Clear");
    store.upsert_agent(&workspace, &first).await.unwrap();
    store.upsert_agent(&workspace, &second).await.unwrap();

    let error = agents
        .update_agent(
            &workspace,
            &AgentUpdate {
                id: second.id.clone(),
                name: "MAIN".to_string(),
                role: "Technical Writer".to_string(),
                tools: vec![],
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("already exists"));

    let renamed = agents
        .update_agent(
            &workspace,
            &AgentUpdate {
                id: second.id.clone(),
                name: "Writer".to_string(),
                role: "Technical Writer".to_string(),
                tools: vec![],
            },
        )
        .await
        .unwrap();
    assert_eq!(renamed.name, "Writer");
}

#[tokio::test]
async fn blank_agent_names_are_rejected() {
    let (store, agents, _channels, workspace) = setup().await;
    let agent = AgentProfile::new("main", "Primary Assistant", "Run", "Lead");
    store.upsert_agent(&workspace, &agent).await.unwrap();

    let error = agents
        .update_agent(
            &workspace,
            &AgentUpdate {
                id: agent.id.clone(),
                name: "   ".to_string(),
                role: "Primary Assistant".to_string(),
                tools: vec![],
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("cannot be empty"));
}

#[tokio::test]
async fn an_agent_of_another_workspace_cannot_be_updated() {
    let (store, agents, _channels, workspace) = setup().await;
    let other = cowbot_core::identity::Workspace::new("Other Team");
    store.create_workspace(&other).await.unwrap();
    let victim = AgentProfile::new("Secret", "Spy", "exfiltrate", "hidden");
    store.upsert_agent(&other.id, &victim).await.unwrap();

    let error = agents
        .update_agent(
            &workspace,
            &AgentUpdate {
                id: victim.id.clone(),
                name: "Captured".to_string(),
                role: "Spy".to_string(),
                tools: vec![],
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("was not found"));

    let stored = store.get_agent(&victim.id).await.unwrap().unwrap();
    assert_eq!(stored.name, "Secret");
    assert!(store
        .agent_in_workspace(&victim.id, &other.id)
        .await
        .unwrap());
}
