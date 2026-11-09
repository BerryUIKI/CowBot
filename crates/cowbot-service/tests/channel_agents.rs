use std::sync::Arc;

use cowbot_bus::EventBus;
use cowbot_core::agent::AgentProfile;
use cowbot_core::channel::Sender;
use cowbot_llm::MockLlmProvider;
use cowbot_service::{bootstrap_local, ChannelAgentService, ChannelService, ChannelTurnOptions};
use cowbot_store::Store;
use cowbot_tools::ToolRegistry;

async fn setup() -> (
    Store,
    ChannelAgentService,
    ChannelService,
    cowbot_core::channel::Channel,
) {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let bus = EventBus::default_bus();
    let context = bootstrap_local(&store).await.unwrap();
    store
        .upsert_agent(
            &context.workspace.id,
            &AgentProfile::new("Planner", "Architect", "Plan", "Careful"),
        )
        .await
        .unwrap();

    let channels = ChannelService::new(store.clone(), bus.clone());
    let channel = channels
        .create_channel(&context.workspace.id, "general", "")
        .await
        .unwrap();
    let agents = ChannelAgentService::new(store.clone(), bus, context.workspace.id.clone());
    (store, agents, channels, channel)
}

fn options() -> ChannelTurnOptions {
    ChannelTurnOptions::default()
}

#[tokio::test]
async fn a_mentioned_agent_answers_in_the_channel() {
    let (store, agents, channels, channel) = setup().await;

    let question = channels
        .post_message(
            &channel.id,
            Sender::User("user-1".to_string()),
            "@Planner what next?",
        )
        .await
        .unwrap();

    let report = agents
        .reply(
            &channel.id,
            &question,
            Arc::new(MockLlmProvider::echo()),
            ToolRegistry::new(),
            "mock-model",
            &options(),
        )
        .await
        .unwrap();

    assert_eq!(report.agents.len(), 1);
    assert_eq!(report.tasks.len(), 1);

    let messages = channels.list_messages(&channel.id, 20).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[1].sender, Sender::Agent(_)));
    assert!(messages[1].content.contains("what next?"));

    // The turn is recorded as normal task history, so it shows up in Tasks.
    let tasks = store
        .list_tasks(&store.list_workspaces().await.unwrap()[0].id)
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(tasks[0].description.contains("#general"));
}

#[tokio::test]
async fn unknown_mentions_produce_a_system_notice() {
    let (_, agents, channels, channel) = setup().await;

    let question = channels
        .post_message(
            &channel.id,
            Sender::User("user-1".to_string()),
            "@Nobody hello",
        )
        .await
        .unwrap();
    let report = agents
        .reply(
            &channel.id,
            &question,
            Arc::new(MockLlmProvider::echo()),
            ToolRegistry::new(),
            "mock-model",
            &options(),
        )
        .await
        .unwrap();

    assert!(report.is_empty());
    assert_eq!(report.unknown_mentions, vec!["Nobody"]);

    let messages = channels.list_messages(&channel.id, 20).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[1].sender, Sender::System));
    assert!(messages[1].content.contains("@Planner"));
}

#[tokio::test]
async fn messages_without_mentions_stay_silent() {
    let (_, agents, channels, channel) = setup().await;

    let question = channels
        .post_message(
            &channel.id,
            Sender::User("user-1".to_string()),
            "just thinking",
        )
        .await
        .unwrap();
    let report = agents
        .reply(
            &channel.id,
            &question,
            Arc::new(MockLlmProvider::echo()),
            ToolRegistry::new(),
            "mock-model",
            &options(),
        )
        .await
        .unwrap();

    assert!(report.is_empty());
    let messages = channels.list_messages(&channel.id, 20).await.unwrap();
    assert_eq!(messages.len(), 1);
}

#[tokio::test]
async fn team_mentions_answer_once_per_agent() {
    let (store, agents, channels, channel) = setup().await;
    let workspace = store.list_workspaces().await.unwrap()[0].id.clone();
    store
        .upsert_agent(
            &workspace,
            &AgentProfile::new("Writer", "Writer", "Write", "Clear"),
        )
        .await
        .unwrap();

    let question = channels
        .post_message(
            &channel.id,
            Sender::User("user-1".to_string()),
            "@team status?",
        )
        .await
        .unwrap();
    let report = agents
        .reply(
            &channel.id,
            &question,
            Arc::new(MockLlmProvider::echo()),
            ToolRegistry::new(),
            "mock-model",
            &options(),
        )
        .await
        .unwrap();

    assert_eq!(report.agents.len(), 2);
    let messages = channels.list_messages(&channel.id, 20).await.unwrap();
    let replies = messages
        .iter()
        .filter(|message| matches!(message.sender, Sender::Agent(_)))
        .count();
    assert_eq!(replies, 2);
}

#[tokio::test]
async fn a_direct_conversation_answers_without_a_mention() {
    let (store, agents, channels, _) = setup().await;
    let workspace = store.list_workspaces().await.unwrap()[0].id.clone();
    let planner = store
        .list_agents(&workspace)
        .await
        .unwrap()
        .into_iter()
        .find(|agent| agent.name == "Planner")
        .unwrap();

    let direct = channels
        .ensure_agent_channel(&workspace, &planner)
        .await
        .unwrap();
    assert_eq!(direct.agent_id.as_ref(), Some(&planner.id));

    // The conversation is provisioned once and reused afterwards.
    let again = channels
        .ensure_agent_channel(&workspace, &planner)
        .await
        .unwrap();
    assert_eq!(again.id, direct.id);

    let question = channels
        .post_message(&direct.id, Sender::User("user-1".to_string()), "what next?")
        .await
        .unwrap();
    let report = agents
        .reply(
            &direct.id,
            &question,
            Arc::new(MockLlmProvider::echo()),
            ToolRegistry::new(),
            "mock-model",
            &options(),
        )
        .await
        .unwrap();

    assert_eq!(report.agents, vec![planner.id]);
    assert_eq!(report.tasks.len(), 1);
    assert!(report.unknown_mentions.is_empty());

    let messages = channels.list_messages(&direct.id, 20).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[1].sender, Sender::Agent(_)));
    assert!(messages[1].content.contains("what next?"));

    let tasks = store.list_tasks(&workspace).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(tasks[0].description.contains("direct conversation"));
}

#[tokio::test]
async fn a_direct_conversation_ignores_mentions_of_others() {
    let (store, agents, channels, _) = setup().await;
    let workspace = store.list_workspaces().await.unwrap()[0].id.clone();
    let planner = store
        .list_agents(&workspace)
        .await
        .unwrap()
        .into_iter()
        .find(|agent| agent.name == "Planner")
        .unwrap();
    store
        .upsert_agent(
            &workspace,
            &AgentProfile::new("Writer", "Writer", "Write", "Clear"),
        )
        .await
        .unwrap();

    let direct = channels
        .ensure_agent_channel(&workspace, &planner)
        .await
        .unwrap();
    let question = channels
        .post_message(
            &direct.id,
            Sender::User("user-1".to_string()),
            "@Writer and @team, what next?",
        )
        .await
        .unwrap();
    let report = agents
        .reply(
            &direct.id,
            &question,
            Arc::new(MockLlmProvider::echo()),
            ToolRegistry::new(),
            "mock-model",
            &options(),
        )
        .await
        .unwrap();

    assert_eq!(report.agents, vec![planner.id]);
    assert_eq!(report.tasks.len(), 1);

    let messages = channels.list_messages(&direct.id, 20).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert!(messages
        .iter()
        .all(|message| !matches!(message.sender, Sender::System)));
}
