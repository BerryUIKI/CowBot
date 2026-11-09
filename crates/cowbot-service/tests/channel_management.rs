use cowbot_bus::EventBus;
use cowbot_core::agent::AgentProfile;
use cowbot_core::channel::Sender;
use cowbot_core::identity::WorkspaceId;
use cowbot_service::{bootstrap_local, ChannelService};
use cowbot_store::Store;

async fn setup() -> (Store, ChannelService, WorkspaceId) {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let bus = EventBus::default_bus();
    let context = bootstrap_local(&store).await.unwrap();
    let channels = ChannelService::new(store.clone(), bus);
    (store, channels, context.workspace.id)
}

#[tokio::test]
async fn duplicate_channel_names_are_rejected_case_insensitively() {
    let (_store, channels, workspace) = setup().await;

    channels
        .create_channel(&workspace, "general", "")
        .await
        .unwrap();
    let error = channels
        .create_channel(&workspace, "  GENERAL ", "")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("already exists"));

    let ops = channels
        .create_channel(&workspace, "ops", "")
        .await
        .unwrap();
    let error = channels
        .rename_channel(&workspace, &ops.id, "General", "")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("already exists"));

    // Changing only the case of its own name is allowed.
    let renamed = channels
        .rename_channel(&workspace, &ops.id, "OPS", "Operations")
        .await
        .unwrap();
    assert_eq!(renamed.name, "OPS");
    assert_eq!(renamed.topic, "Operations");
}

#[tokio::test]
async fn blank_channel_names_are_rejected() {
    let (_store, channels, workspace) = setup().await;
    let error = channels
        .create_channel(&workspace, "   ", "")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("cannot be empty"));
}

#[tokio::test]
async fn agent_conversations_cannot_be_deleted_but_can_be_cleared() {
    let (store, channels, workspace) = setup().await;
    let agent = AgentProfile::new("main", "Primary Assistant", "Run the company", "OPC lead");
    store.upsert_agent(&workspace, &agent).await.unwrap();

    let direct = channels
        .ensure_agent_channel(&workspace, &agent)
        .await
        .unwrap();
    channels
        .post_message(&direct.id, Sender::User("user-1".to_string()), "hello")
        .await
        .unwrap();

    let error = channels.delete_channel(&direct.id).await.unwrap_err();
    assert!(error.to_string().contains("cannot be deleted"));
    assert_eq!(
        channels.clear_channel_messages(&direct.id).await.unwrap(),
        1
    );
    assert!(channels
        .list_messages(&direct.id, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn deleting_a_group_channel_cascades_messages() {
    let (_store, channels, workspace) = setup().await;
    let channel = channels
        .create_channel(&workspace, "general", "")
        .await
        .unwrap();
    channels
        .post_message(&channel.id, Sender::System, "hello")
        .await
        .unwrap();

    let deleted = channels.delete_channel(&channel.id).await.unwrap();
    assert_eq!(deleted.id, channel.id);
    assert!(channels.list_channels(&workspace).await.unwrap().is_empty());
    assert!(channels
        .list_messages(&channel.id, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn ensure_agent_channel_reconciles_name_and_role() {
    let (store, channels, workspace) = setup().await;
    let agent = AgentProfile::new("Assistant", "Generalist", "Help", "Careful");
    store.upsert_agent(&workspace, &agent).await.unwrap();

    let direct = channels
        .ensure_agent_channel(&workspace, &agent)
        .await
        .unwrap();
    assert_eq!(direct.name, "Assistant");
    assert_eq!(direct.topic, "Generalist");

    let mut renamed = agent.clone();
    renamed.name = "main".to_string();
    renamed.role = "Primary Assistant".to_string();
    store.upsert_agent(&workspace, &renamed).await.unwrap();

    let synced = channels
        .ensure_agent_channel(&workspace, &renamed)
        .await
        .unwrap();
    assert_eq!(synced.id, direct.id);
    assert_eq!(synced.name, "main");
    assert_eq!(synced.topic, "Primary Assistant");
}

#[tokio::test]
async fn pinning_a_channel_is_persisted() {
    let (_store, channels, workspace) = setup().await;
    let channel = channels
        .create_channel(&workspace, "general", "")
        .await
        .unwrap();
    assert!(!channel.pinned);

    let pinned = channels
        .set_channel_pinned(&channel.id, true)
        .await
        .unwrap();
    assert!(pinned.pinned);
    let listed = channels.list_channels(&workspace).await.unwrap();
    assert!(listed[0].pinned);

    let unpinned = channels
        .set_channel_pinned(&channel.id, false)
        .await
        .unwrap();
    assert!(!unpinned.pinned);
}
