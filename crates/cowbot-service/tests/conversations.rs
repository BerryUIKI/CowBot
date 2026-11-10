use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cowbot_agent::StandardRoles;
use cowbot_bus::EventBus;
use cowbot_core::agent::AgentProfile;
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::channel::{Channel, ChannelMessage, Sender};
use cowbot_core::conversation::{ContextEntry, ContextEventKind, ContextRole, ConversationMode};
use cowbot_core::error::CowResult;
use cowbot_core::usage::TokenUsage;
use cowbot_llm::{ChatCompletionRequest, ChatCompletionResponse, LlmProvider};
use cowbot_service::{bootstrap_local, ChannelService, ChannelTurnOptions, ConversationService};
use cowbot_store::Store;
use cowbot_tools::ToolRegistry;

struct RecordingLlm {
    requests: Mutex<Vec<ChatCompletionRequest>>,
    answer: Mutex<u32>,
}

impl RecordingLlm {
    fn new() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            answer: Mutex::new(0),
        }
    }

    fn requests(&self) -> Vec<ChatCompletionRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmProvider for RecordingLlm {
    fn name(&self) -> &str {
        "recording"
    }

    async fn chat_complete(
        &self,
        request: &ChatCompletionRequest,
    ) -> CowResult<ChatCompletionResponse> {
        self.requests.lock().unwrap().push(request.clone());
        let mut answer = self.answer.lock().unwrap();
        *answer += 1;
        let summarizing = request
            .messages
            .first()
            .map(|message| {
                message
                    .content
                    .starts_with("You compress conversation history")
            })
            .unwrap_or(false);
        let content = if summarizing {
            "summary text".to_string()
        } else {
            format!("answer {answer}")
        };
        Ok(ChatCompletionResponse {
            content: Some(content),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_string()),
            usage: Some(TokenUsage {
                uncached_input_tokens: 20,
                cached_input_tokens: 80,
                output_tokens: 10,
                ..TokenUsage::default()
            }),
            raw_usage: None,
        })
    }
}

async fn setup() -> (
    Store,
    AgentProfile,
    ConversationService,
    ChannelService,
    Channel,
) {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let context = bootstrap_local(&store).await.unwrap();
    let agent = StandardRoles::coder();
    store
        .upsert_agent(&context.workspace.id, &agent)
        .await
        .unwrap();

    let bus = EventBus::default_bus();
    let service =
        ConversationService::new(store.clone(), bus.clone(), context.workspace.id.clone());
    let channels = ChannelService::new(store.clone(), bus);
    let channel = service
        .create(
            Some("Design review"),
            &agent.id,
            Some("provider-1".to_string()),
            "recording-model",
        )
        .await
        .unwrap();
    (store, agent, service, channels, channel)
}

fn options() -> ChannelTurnOptions {
    ChannelTurnOptions {
        approval_level: ApprovalLevel::Trusted,
        max_concurrency: 2,
        use_model_permissions: false,
        platform_context: None,
    }
}

fn user_message(channel: &Channel, content: &str) -> ChannelMessage {
    ChannelMessage::new(
        channel.id.clone(),
        Sender::User("user-1".to_string()),
        content,
    )
}

#[tokio::test]
async fn conversation_creation_assigns_a_fixed_model_and_participant() {
    let (_, agent, service, _, channel) = setup().await;
    assert_eq!(
        channel.kind,
        cowbot_core::conversation::ChannelKind::Conversation
    );
    assert!(channel.is_conversation());

    let settings = service.settings(&channel.id).await.unwrap();
    assert_eq!(settings.model, "recording-model");
    assert_eq!(settings.provider_id.as_deref(), Some("provider-1"));
    assert_eq!(settings.mode, ConversationMode::Single);
    assert!(!settings.auto_compact);

    let context = service.context(&channel.id).await.unwrap();
    assert_eq!(context.agent_id, agent.id);
    assert_eq!(context.model, "recording-model");
    assert_eq!(context.projection.active_entries, 0);
    assert!(context.projection.system_tokens > 0);
}

#[tokio::test]
async fn replies_build_an_append_only_message_history() {
    let (_, _, service, channels, channel) = setup().await;
    let llm = Arc::new(RecordingLlm::new());

    service
        .reply(
            &channel.id,
            &user_message(&channel, "first question"),
            llm.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();
    service
        .reply(
            &channel.id,
            &user_message(&channel, "second question"),
            llm.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    let requests = llm.requests();
    let second = &requests[requests.len() - 1];
    assert_eq!(second.messages[0].role, cowbot_llm::ChatRole::System);
    assert!(
        second.messages[0].content.contains("coder")
            || second.messages[0].content.contains("Coder")
    );
    assert_eq!(second.messages[1].content, "first question");
    assert_eq!(second.messages[2].content, "answer 1");
    assert_eq!(second.messages[3].content, "second question");
    assert_eq!(
        second.messages[3].role,
        cowbot_llm::ChatRole::User,
        "the latest message is a user turn, not a rendered transcript"
    );

    let context = service.context(&channel.id).await.unwrap();
    assert_eq!(context.projection.active_entries, 4);
    assert!(context.projection.latest_tokens > 0);

    let messages = channels.list_messages(&channel.id, 50).await.unwrap();
    let agent_replies = messages
        .iter()
        .filter(|message| matches!(message.sender, Sender::Agent(_)))
        .count();
    assert_eq!(
        agent_replies, 2,
        "display messages stay separate from context"
    );
}

#[tokio::test]
async fn usage_is_attributed_to_the_conversation_channel() {
    let (store, _, service, _, channel) = setup().await;
    let llm = Arc::new(RecordingLlm::new());

    service
        .reply(
            &channel.id,
            &user_message(&channel, "question"),
            llm.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();
    service
        .reply(
            &channel.id,
            &user_message(&channel, "another"),
            llm.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    let context = service.context(&channel.id).await.unwrap();
    assert_eq!(context.cache_last_turn.cached_input_tokens, 80);
    assert_eq!(context.cache_last_turn.total_input_tokens, 100);
    assert_eq!(context.cache_conversation.cached_input_tokens, 160);
    assert_eq!(context.cache_conversation.total_input_tokens, 200);

    let totals = store.sum_llm_usage_for_channel(&channel.id).await.unwrap();
    assert_eq!(totals.calls, 2);
}

#[tokio::test]
async fn compaction_summarizes_old_entries_and_keeps_recent_ones() {
    let (store, agent, service, channels, channel) = setup().await;
    for index in 0..6 {
        let role = if index % 2 == 0 {
            ContextRole::User
        } else {
            ContextRole::Assistant
        };
        let mut entry = ContextEntry::new(
            channel.id.clone(),
            agent.id.clone(),
            role,
            format!("turn {index} {}", "x".repeat(4_000)),
        );
        entry.recount();
        store.append_context_entry(&entry).await.unwrap();
    }

    let before = service.context(&channel.id).await.unwrap();
    assert_eq!(before.projection.active_entries, 6);

    let llm = Arc::new(RecordingLlm::new());
    let outcome = service
        .compact(&channel.id, llm.clone(), "recording-model")
        .await
        .unwrap();
    assert!(outcome.compacted);
    assert!(outcome.summarized_entries >= 3);
    assert!(outcome.tokens_after < outcome.tokens_before);

    let entries = store
        .list_context_entries(&channel.id, &agent.id)
        .await
        .unwrap();
    let summaries = entries
        .iter()
        .filter(|entry| entry.kind == cowbot_core::conversation::ContextEntryKind::Summary)
        .count();
    assert_eq!(summaries, 1);
    let excluded = entries.iter().filter(|entry| entry.excluded).count();
    assert_eq!(excluded, outcome.summarized_entries as usize);

    let events = store
        .list_context_events(&channel.id, &agent.id, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, ContextEventKind::Compact);
    assert!(events[0].detail.is_some());

    let messages = channels.list_messages(&channel.id, 50).await.unwrap();
    assert!(messages
        .iter()
        .any(|message| message.sender == Sender::System
            && message.content.contains("Context compacted")));

    // A second compaction has nothing to do while the tail fits the budget.
    let again = service
        .compact(&channel.id, llm, "recording-model")
        .await
        .unwrap();
    assert!(!again.compacted);
}

#[tokio::test]
async fn clearing_context_excludes_entries_without_deleting_messages() {
    let (store, agent, service, channels, channel) = setup().await;
    let llm = Arc::new(RecordingLlm::new());
    let message = channels
        .post_message(
            &channel.id,
            Sender::User("user-1".to_string()),
            "remember this",
        )
        .await
        .unwrap();
    service
        .reply(&channel.id, &message, llm, ToolRegistry::new(), &options())
        .await
        .unwrap();

    let cleared = service.clear_context(&channel.id).await.unwrap();
    assert!(cleared >= 2);
    let entries = store
        .list_context_entries(&channel.id, &agent.id)
        .await
        .unwrap();
    assert!(entries.iter().all(|entry| entry.excluded));

    let messages = channels.list_messages(&channel.id, 50).await.unwrap();
    assert!(messages
        .iter()
        .any(|message| message.content == "remember this"));
    assert!(messages
        .iter()
        .any(|message| message.content.contains("Context cleared")));

    let context = service.context(&channel.id).await.unwrap();
    assert_eq!(context.projection.active_entries, 0);
    assert!(context.projection.excluded_entries >= 2);
}

#[tokio::test]
async fn direct_and_group_channels_keep_their_kind_and_behavior() {
    let (store, agent, service, channels, _) = setup().await;
    let direct = channels
        .ensure_agent_channel(
            &store.list_workspaces().await.unwrap().first().unwrap().id,
            &agent,
        )
        .await
        .unwrap();
    assert!(direct.is_direct());
    assert_eq!(direct.agent_id.as_ref(), Some(&agent.id));

    let group = channels
        .create_channel(
            &store.list_workspaces().await.unwrap().first().unwrap().id,
            "general",
            "General",
        )
        .await
        .unwrap();
    assert_eq!(group.kind, cowbot_core::conversation::ChannelKind::Channel);

    // Conversation-only operations reject other channel kinds.
    assert!(service.context(&direct.id).await.is_err());
    assert!(service.context(&group.id).await.is_err());
}
