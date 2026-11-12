use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cowbot_agent::StandardRoles;
use cowbot_bus::EventBus;
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::channel::{Channel, ChannelMessage, Sender};
use cowbot_core::conversation::{
    compaction_split_by, ContextEntry, ContextEntryKind, ContextEventKind, ContextRole,
    ConversationMode, ConversationSettingsUpdate, RouteSource,
};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::usage::{TokenSource, TokenUsage, MESSAGE_OVERHEAD_TOKENS};
use cowbot_llm::{ChatCompletionRequest, ChatCompletionResponse, LlmProvider};
use cowbot_service::{
    bootstrap_local, ChannelService, ChannelTurnOptions, ConversationService, ParticipantCapacity,
};
use cowbot_store::Store;
use cowbot_tools::ToolRegistry;
use tokio::sync::{Barrier, Notify};

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

fn test_response(content: String) -> ChatCompletionResponse {
    ChatCompletionResponse {
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
    }
}

fn is_summarizer_request(request: &ChatCompletionRequest) -> bool {
    request
        .messages
        .first()
        .map(|message| {
            message
                .content
                .starts_with("You compress conversation history")
        })
        .unwrap_or(false)
}

/// Blocks the summarizing request until the test releases it.
struct GatedLlm {
    entered: Notify,
    release: Notify,
    summarizer_calls: Mutex<usize>,
    answers: Mutex<u32>,
}

impl GatedLlm {
    fn new() -> Self {
        Self {
            entered: Notify::new(),
            release: Notify::new(),
            summarizer_calls: Mutex::new(0),
            answers: Mutex::new(0),
        }
    }

    fn summarizer_calls(&self) -> usize {
        *self.summarizer_calls.lock().unwrap()
    }
}

#[async_trait]
impl LlmProvider for GatedLlm {
    fn name(&self) -> &str {
        "gated"
    }

    async fn chat_complete(
        &self,
        request: &ChatCompletionRequest,
    ) -> CowResult<ChatCompletionResponse> {
        if is_summarizer_request(request) {
            *self.summarizer_calls.lock().unwrap() += 1;
            self.entered.notify_one();
            self.release.notified().await;
            return Ok(test_response("summary text".to_string()));
        }
        let mut answers = self.answers.lock().unwrap();
        *answers += 1;
        Ok(test_response(format!("answer {answers}")))
    }
}

/// Requires two summarizer calls to enter before either returns.
struct BarrierLlm {
    barrier: Barrier,
    summarizer_calls: Mutex<usize>,
}

impl BarrierLlm {
    fn new() -> Self {
        Self {
            barrier: Barrier::new(2),
            summarizer_calls: Mutex::new(0),
        }
    }

    fn summarizer_calls(&self) -> usize {
        *self.summarizer_calls.lock().unwrap()
    }
}

#[async_trait]
impl LlmProvider for BarrierLlm {
    fn name(&self) -> &str {
        "barrier"
    }

    async fn chat_complete(
        &self,
        request: &ChatCompletionRequest,
    ) -> CowResult<ChatCompletionResponse> {
        if is_summarizer_request(request) {
            *self.summarizer_calls.lock().unwrap() += 1;
            self.barrier.wait().await;
            return Ok(test_response("summary text".to_string()));
        }
        Ok(test_response("answer".to_string()))
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
        participant_capacity: Vec::new(),
    }
}

fn user_message(channel: &Channel, content: &str) -> ChannelMessage {
    ChannelMessage::new(
        channel.id.clone(),
        Sender::User("user-1".to_string()),
        content,
    )
}

struct GroupFixture {
    store: Store,
    service: ConversationService,
    channels: ChannelService,
    participants: Vec<AgentProfile>,
    channel: Channel,
}

async fn group_fixture(default_responder: Option<AgentId>) -> GroupFixture {
    let participants = vec![
        AgentProfile::new("Planner", "Architect", "Plan", "Careful"),
        AgentProfile::new("Coder", "Engineer", "Ship", "Careful"),
    ];
    group_fixture_with(participants, default_responder).await
}

async fn group_fixture_with(
    participants: Vec<AgentProfile>,
    default_responder: Option<AgentId>,
) -> GroupFixture {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let context = bootstrap_local(&store).await.unwrap();
    for agent in &participants {
        store
            .upsert_agent(&context.workspace.id, agent)
            .await
            .unwrap();
    }
    let bus = EventBus::default_bus();
    let service =
        ConversationService::new(store.clone(), bus.clone(), context.workspace.id.clone());
    let channels = ChannelService::new(store.clone(), bus);
    let ids = participants
        .iter()
        .map(|agent| agent.id.clone())
        .collect::<Vec<_>>();
    let channel = service
        .create_group(
            Some("Build crew"),
            &ids,
            Some("provider-1".to_string()),
            "recording-model",
            default_responder,
        )
        .await
        .unwrap();
    GroupFixture {
        store,
        service,
        channels,
        participants,
        channel,
    }
}

async fn post_user(channels: &ChannelService, channel: &Channel, content: &str) -> ChannelMessage {
    channels
        .post_message(&channel.id, Sender::User("user-1".to_string()), content)
        .await
        .unwrap()
}

/// Append entries large enough that the compaction split has something to do.
async fn seed_large_entries(
    store: &Store,
    channel: &Channel,
    agent_id: &AgentId,
    count: usize,
) -> Vec<ContextEntry> {
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let role = if index % 2 == 0 {
            ContextRole::User
        } else {
            ContextRole::Assistant
        };
        let mut entry = ContextEntry::new(
            channel.id.clone(),
            agent_id.clone(),
            role,
            format!("turn {index} {}", "x".repeat(4_000)),
        );
        entry.recount();
        store.append_context_entry(&entry).await.unwrap();
        entries.push(entry);
    }
    entries
}

/// The split the service computes for its estimator-counted test model.
///
/// `recording-model` is unrecognized, so entries cost their persisted estimate
/// plus one message-framing allowance each (ADR-0035).
fn service_split(entries: &[ContextEntry]) -> usize {
    compaction_split_by(entries, |entry| {
        entry.tokens.saturating_add(MESSAGE_OVERHEAD_TOKENS)
    })
}

fn agent_replies(messages: &[ChannelMessage]) -> Vec<AgentId> {
    messages
        .iter()
        .filter_map(|message| match &message.sender {
            Sender::Agent(id) => Some(id.clone()),
            _ => None,
        })
        .collect()
}

struct FailingLlm {
    fail_on: String,
}

enum RouterReply {
    Json(&'static str),
    Error,
}

struct RouterLlm {
    reply: RouterReply,
    requests: Mutex<Vec<ChatCompletionRequest>>,
}

impl RouterLlm {
    fn new(reply: RouterReply) -> Self {
        Self {
            reply,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn router_requests(&self) -> Vec<ChatCompletionRequest> {
        self.requests.lock().unwrap().clone()
    }
}

fn router_response(content: Option<String>) -> ChatCompletionResponse {
    ChatCompletionResponse {
        content,
        tool_calls: Vec::new(),
        finish_reason: Some("stop".to_string()),
        usage: Some(TokenUsage {
            uncached_input_tokens: 60,
            cached_input_tokens: 40,
            output_tokens: 5,
            ..TokenUsage::default()
        }),
        raw_usage: None,
    }
}

#[async_trait]
impl LlmProvider for RouterLlm {
    fn name(&self) -> &str {
        "router"
    }

    async fn chat_complete(
        &self,
        request: &ChatCompletionRequest,
    ) -> CowResult<ChatCompletionResponse> {
        let routing = request
            .messages
            .first()
            .map(|message| {
                message
                    .content
                    .starts_with("You route messages in a group conversation")
            })
            .unwrap_or(false);
        if routing {
            self.requests.lock().unwrap().push(request.clone());
            return match self.reply {
                RouterReply::Json(content) => Ok(router_response(Some(content.to_string()))),
                RouterReply::Error => {
                    Err(CowError::Agent("router provider unavailable".to_string()))
                }
            };
        }
        Ok(router_response(Some("agent answer".to_string())))
    }
}

#[async_trait]
impl LlmProvider for FailingLlm {
    fn name(&self) -> &str {
        "failing"
    }

    async fn chat_complete(
        &self,
        request: &ChatCompletionRequest,
    ) -> CowResult<ChatCompletionResponse> {
        let system = request
            .messages
            .first()
            .map(|message| message.content.as_str())
            .unwrap_or_default();
        if system.contains(&self.fail_on) {
            return Err(CowError::Agent(format!(
                "{} failed in the test",
                self.fail_on
            )));
        }
        Ok(ChatCompletionResponse {
            content: Some("still here".to_string()),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_string()),
            usage: None,
            raw_usage: None,
        })
    }
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
    // An unrecognized model is counted with the character estimator and says so.
    assert_eq!(context.projection.token_source, TokenSource::Estimate);
    assert_eq!(
        context.projection.token_counter.as_deref(),
        Some("character estimate")
    );
    assert!(context.projection.is_estimated());
}

#[tokio::test]
async fn context_counts_entries_with_the_conversation_model_tokenizer() {
    let (store, agent, service, _, _) = setup().await;
    let channel = service
        .create(Some("Counting"), &agent.id, None, "gpt-4o")
        .await
        .unwrap();

    let mut entry = ContextEntry::new(
        channel.id.clone(),
        agent.id.clone(),
        ContextRole::User,
        "hello world",
    );
    // The persisted estimate is model-independent and must not drive the meter.
    entry.tokens = 999;
    store.append_context_entry(&entry).await.unwrap();

    let context = service.context(&channel.id).await.unwrap();
    assert_eq!(context.projection.token_source, TokenSource::Tiktoken);
    assert_eq!(
        context.projection.token_counter.as_deref(),
        Some("o200k_base")
    );
    assert!(!context.projection.is_estimated());
    // `o200k_base` counts "hello world" as two tokens, plus message framing.
    assert_eq!(
        context.projection.latest_tokens,
        2 + MESSAGE_OVERHEAD_TOKENS
    );
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
async fn compaction_keeps_entries_appended_during_the_summary_call() {
    let (store, agent, service, _channels, channel) = setup().await;
    seed_large_entries(&store, &channel, &agent.id, 6).await;

    let before = store
        .list_context_entries(&channel.id, &agent.id)
        .await
        .unwrap();
    let active = before
        .iter()
        .filter(|entry| !entry.excluded)
        .cloned()
        .collect::<Vec<_>>();
    let cutoff = active[service_split(&active)].seq;

    let llm = Arc::new(GatedLlm::new());
    let task = {
        let service = service.clone();
        let channel_id = channel.id.clone();
        let llm = llm.clone();
        tokio::spawn(async move { service.compact(&channel_id, llm, "recording-model").await })
    };

    tokio::time::timeout(Duration::from_secs(5), llm.entered.notified())
        .await
        .expect("the summarizer should be requested");

    let mut appended = ContextEntry::new(
        channel.id.clone(),
        agent.id.clone(),
        ContextRole::User,
        "appended while the summary was running",
    );
    appended.recount();
    let appended_seq = store.append_context_entry(&appended).await.unwrap();

    llm.release.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("compaction should finish")
        .unwrap()
        .unwrap();
    assert!(outcome.compacted);
    assert_eq!(llm.summarizer_calls(), 1);

    let entries = store
        .list_context_entries(&channel.id, &agent.id)
        .await
        .unwrap();
    let summary = entries
        .iter()
        .find(|entry| entry.kind == ContextEntryKind::Summary)
        .expect("one summary");
    assert_eq!(summary.seq, cutoff);
    let appended = entries
        .iter()
        .find(|entry| entry.id == appended.id)
        .expect("the appended entry stays");
    assert_eq!(appended.seq, appended_seq + 1);
    assert!(appended.seq > summary.seq);

    let active_tokens: u32 = entries
        .iter()
        .filter(|entry| !entry.excluded)
        .map(|entry| entry.tokens)
        .sum();
    assert_eq!(outcome.tokens_after, active_tokens);
    assert!(outcome.tokens_before > outcome.tokens_after);
}

#[tokio::test]
async fn concurrent_compactions_on_one_branch_return_in_progress() {
    let (store, agent, service, _channels, channel) = setup().await;
    seed_large_entries(&store, &channel, &agent.id, 6).await;

    let llm = Arc::new(GatedLlm::new());
    let first = {
        let service = service.clone();
        let channel_id = channel.id.clone();
        let llm = llm.clone();
        tokio::spawn(async move { service.compact(&channel_id, llm, "recording-model").await })
    };
    tokio::time::timeout(Duration::from_secs(5), llm.entered.notified())
        .await
        .expect("the first summarizer should be requested");

    let second = service
        .compact(&channel.id, llm.clone(), "recording-model")
        .await
        .unwrap();
    assert!(!second.compacted);
    assert!(second.in_progress);
    assert!(second.reason.is_some());

    llm.release.notify_one();
    let first = tokio::time::timeout(Duration::from_secs(5), first)
        .await
        .expect("the first compaction should finish")
        .unwrap()
        .unwrap();
    assert!(first.compacted);
    assert_eq!(llm.summarizer_calls(), 1);

    let entries = store
        .list_context_entries(&channel.id, &agent.id)
        .await
        .unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.kind == ContextEntryKind::Summary)
            .count(),
        1
    );
    assert_eq!(
        store
            .list_context_events(&channel.id, &agent.id, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn compaction_times_out_without_writing() {
    let (store, agent, service, _channels, channel) = setup().await;
    seed_large_entries(&store, &channel, &agent.id, 6).await;

    let llm = Arc::new(GatedLlm::new());
    let task = {
        let service = service.clone();
        let channel_id = channel.id.clone();
        let llm = llm.clone();
        tokio::spawn(async move { service.compact(&channel_id, llm, "recording-model").await })
    };
    tokio::time::timeout(Duration::from_secs(5), llm.entered.notified())
        .await
        .expect("the summarizer should be requested");

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(121)).await;
    let outcome = task.await.unwrap().unwrap();
    tokio::time::resume();

    assert!(!outcome.compacted);
    assert!(!outcome.in_progress);
    assert!(outcome
        .reason
        .as_deref()
        .is_some_and(|reason| reason.contains("timed out")));

    let entries = store
        .list_context_entries(&channel.id, &agent.id)
        .await
        .unwrap();
    assert!(!entries
        .iter()
        .any(|entry| entry.kind == ContextEntryKind::Summary));
}

#[tokio::test]
async fn compaction_rejects_invalid_sequences_before_the_model_call() {
    let (store, agent, service, _channels, channel) = setup().await;
    let mut imported = ContextEntry::new(
        channel.id.clone(),
        agent.id.clone(),
        ContextRole::User,
        "imported with a zero sequence",
    );
    imported.seq = 0;
    store.insert_context_entry(&imported).await.unwrap();

    let llm = Arc::new(RecordingLlm::new());
    let outcome = service
        .compact(&channel.id, llm.clone(), "recording-model")
        .await
        .unwrap();
    assert!(!outcome.compacted);
    assert!(!outcome.in_progress);
    assert!(outcome
        .reason
        .as_deref()
        .is_some_and(|reason| reason.contains("invalid sequence")));
    assert!(llm.requests().is_empty());
}

#[tokio::test]
async fn service_instances_sharing_the_registry_serialize_one_branch() {
    let (store, agent, _service, _channels, channel) = setup().await;
    seed_large_entries(&store, &channel, &agent.id, 6).await;
    let context = bootstrap_local(&store).await.unwrap();
    let locks = Arc::new(cowbot_service::CompactionLocks::new());
    let first_service = ConversationService::new(
        store.clone(),
        EventBus::default_bus(),
        context.workspace.id.clone(),
    )
    .with_compaction_locks(locks.clone());
    let second_service = ConversationService::new(
        store.clone(),
        EventBus::default_bus(),
        context.workspace.id.clone(),
    )
    .with_compaction_locks(locks);

    let llm = Arc::new(GatedLlm::new());
    let first = {
        let service = first_service.clone();
        let channel_id = channel.id.clone();
        let llm = llm.clone();
        tokio::spawn(async move { service.compact(&channel_id, llm, "recording-model").await })
    };
    tokio::time::timeout(Duration::from_secs(5), llm.entered.notified())
        .await
        .expect("the first summarizer should be requested");

    let second = second_service
        .compact(&channel.id, llm.clone(), "recording-model")
        .await
        .unwrap();
    assert!(!second.compacted);
    assert!(second.in_progress);

    llm.release.notify_one();
    let first = tokio::time::timeout(Duration::from_secs(5), first)
        .await
        .expect("the first compaction should finish")
        .unwrap()
        .unwrap();
    assert!(first.compacted);
    assert_eq!(llm.summarizer_calls(), 1);
}

#[tokio::test]
async fn independent_branches_compact_concurrently() {
    let fixture = group_fixture(None).await;
    for participant in &fixture.participants {
        seed_large_entries(&fixture.store, &fixture.channel, &participant.id, 6).await;
    }

    let llm = Arc::new(BarrierLlm::new());
    let first = {
        let service = fixture.service.clone();
        let channel_id = fixture.channel.id.clone();
        let agent_id = fixture.participants[0].id.clone();
        let llm = llm.clone();
        tokio::spawn(async move {
            service
                .compact_participant(&channel_id, &agent_id, llm, "recording-model")
                .await
        })
    };
    let second = {
        let service = fixture.service.clone();
        let channel_id = fixture.channel.id.clone();
        let agent_id = fixture.participants[1].id.clone();
        let llm = llm.clone();
        tokio::spawn(async move {
            service
                .compact_participant(&channel_id, &agent_id, llm, "recording-model")
                .await
        })
    };

    let (first, second) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(first, second)
    })
    .await
    .expect("both branches should reach the summarizer barrier");
    assert!(first.unwrap().unwrap().compacted);
    assert!(second.unwrap().unwrap().compacted);
    assert_eq!(llm.summarizer_calls(), 2);

    for participant in &fixture.participants {
        let entries = fixture
            .store
            .list_context_entries(&fixture.channel.id, &participant.id)
            .await
            .unwrap();
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.kind == ContextEntryKind::Summary)
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn group_auto_compaction_skips_a_branch_that_is_already_compacting() {
    let fixture = group_fixture(None).await;
    let planner = fixture.participants[0].clone();
    seed_large_entries(&fixture.store, &fixture.channel, &planner.id, 6).await;
    let mut settings = fixture.service.settings(&fixture.channel.id).await.unwrap();
    settings.auto_compact = true;
    settings.compact_threshold = 1;
    fixture
        .store
        .update_conversation_settings(&settings)
        .await
        .unwrap();

    let llm = Arc::new(GatedLlm::new());
    let manual = {
        let service = fixture.service.clone();
        let channel_id = fixture.channel.id.clone();
        let agent_id = planner.id.clone();
        let llm = llm.clone();
        tokio::spawn(async move {
            service
                .compact_participant(&channel_id, &agent_id, llm, "recording-model")
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), llm.entered.notified())
        .await
        .expect("the manual summarizer should be requested");

    let message = post_user(&fixture.channels, &fixture.channel, "@Planner status").await;
    let mut turn_options = options();
    turn_options.participant_capacity = vec![
        ParticipantCapacity {
            agent_id: planner.id.clone(),
            usable_window: 60_000,
            tool_tokens: 0,
        },
        ParticipantCapacity {
            agent_id: fixture.participants[1].id.clone(),
            usable_window: 60_000,
            tool_tokens: 0,
        },
    ];
    let report = fixture
        .service
        .reply(
            &fixture.channel.id,
            &message,
            llm.clone(),
            ToolRegistry::new(),
            &turn_options,
        )
        .await
        .unwrap();
    assert_eq!(report.replied, vec![planner.id.clone()]);

    let messages = fixture
        .channels
        .list_messages(&fixture.channel.id, 50)
        .await
        .unwrap();
    assert!(!messages.iter().any(|message| {
        message.sender == Sender::System
            && message
                .content
                .contains("Context compacted automatically for Planner")
    }));

    llm.release.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(5), manual)
        .await
        .expect("the manual compaction should finish")
        .unwrap()
        .unwrap();
    assert!(outcome.compacted);
    let entries = fixture
        .store
        .list_context_entries(&fixture.channel.id, &planner.id)
        .await
        .unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.kind == ContextEntryKind::Summary)
            .count(),
        1
    );
}

#[tokio::test]
async fn group_auto_compaction_reports_an_in_progress_branch_instead_of_full_advice() {
    let fixture = group_fixture(None).await;
    let planner = fixture.participants[0].clone();
    let coder = fixture.participants[1].clone();
    seed_large_entries(&fixture.store, &fixture.channel, &planner.id, 6).await;
    let mut settings = fixture.service.settings(&fixture.channel.id).await.unwrap();
    settings.auto_compact = true;
    settings.compact_threshold = 1;
    fixture
        .store
        .update_conversation_settings(&settings)
        .await
        .unwrap();

    let llm = Arc::new(GatedLlm::new());
    let manual = {
        let service = fixture.service.clone();
        let channel_id = fixture.channel.id.clone();
        let agent_id = planner.id.clone();
        let llm = llm.clone();
        tokio::spawn(async move {
            service
                .compact_participant(&channel_id, &agent_id, llm, "recording-model")
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), llm.entered.notified())
        .await
        .expect("the manual summarizer should be requested");

    let message = post_user(&fixture.channels, &fixture.channel, "@team status update").await;
    let mut turn_options = options();
    turn_options.participant_capacity = vec![ParticipantCapacity {
        agent_id: planner.id.clone(),
        usable_window: 1,
        tool_tokens: 0,
    }];
    let report = fixture
        .service
        .reply(
            &fixture.channel.id,
            &message,
            llm.clone(),
            ToolRegistry::new(),
            &turn_options,
        )
        .await
        .unwrap();
    assert_eq!(report.skipped, vec![planner.id.clone()]);
    assert_eq!(report.replied, vec![coder.id.clone()]);

    let messages = fixture
        .channels
        .list_messages(&fixture.channel.id, 50)
        .await
        .unwrap();
    assert!(messages.iter().any(|message| {
        message.sender == Sender::System
            && message.content.contains("being compacted")
            && !message.content.contains("context is full")
    }));

    llm.release.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(5), manual)
        .await
        .expect("the manual compaction should finish")
        .unwrap()
        .unwrap();
    assert!(outcome.compacted);
}

#[tokio::test]
async fn clearing_context_during_compaction_writes_nothing() {
    let (store, agent, service, channels, channel) = setup().await;
    seed_large_entries(&store, &channel, &agent.id, 6).await;

    let llm = Arc::new(GatedLlm::new());
    let task = {
        let service = service.clone();
        let channel_id = channel.id.clone();
        let llm = llm.clone();
        tokio::spawn(async move { service.compact(&channel_id, llm, "recording-model").await })
    };
    tokio::time::timeout(Duration::from_secs(5), llm.entered.notified())
        .await
        .expect("the summarizer should be requested");

    let cleared = service.clear_context(&channel.id).await.unwrap();
    assert!(cleared >= 6);
    llm.release.notify_one();

    let outcome = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("compaction should finish")
        .unwrap()
        .unwrap();
    assert!(!outcome.compacted);
    assert!(outcome.reason.is_some());

    let entries = store
        .list_context_entries(&channel.id, &agent.id)
        .await
        .unwrap();
    assert!(!entries
        .iter()
        .any(|entry| entry.kind == ContextEntryKind::Summary));
    let messages = channels.list_messages(&channel.id, 50).await.unwrap();
    assert!(!messages
        .iter()
        .any(|message| message.content.contains("Context compacted:")));
}

#[tokio::test]
async fn compaction_reports_a_stale_prefix_when_an_entry_is_excluded_during_the_call() {
    let (store, agent, service, _channels, channel) = setup().await;
    seed_large_entries(&store, &channel, &agent.id, 6).await;

    let llm = Arc::new(GatedLlm::new());
    let task = {
        let service = service.clone();
        let channel_id = channel.id.clone();
        let llm = llm.clone();
        tokio::spawn(async move { service.compact(&channel_id, llm, "recording-model").await })
    };
    tokio::time::timeout(Duration::from_secs(5), llm.entered.notified())
        .await
        .expect("the summarizer should be requested");

    let entries = store
        .list_context_entries(&channel.id, &agent.id)
        .await
        .unwrap();
    let active = entries
        .iter()
        .filter(|entry| !entry.excluded)
        .cloned()
        .collect::<Vec<_>>();
    let summarized = &active[..service_split(&active)];
    store
        .exclude_context_entries(std::slice::from_ref(&summarized[0].id))
        .await
        .unwrap();

    llm.release.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("compaction should finish")
        .unwrap()
        .unwrap();
    assert!(!outcome.compacted);
    assert!(!outcome.in_progress);
    assert!(outcome
        .reason
        .as_deref()
        .is_some_and(|reason| reason.contains("changed")));

    let entries = store
        .list_context_entries(&channel.id, &agent.id)
        .await
        .unwrap();
    assert!(!entries
        .iter()
        .any(|entry| entry.kind == ContextEntryKind::Summary));
}

#[tokio::test]
async fn clearing_one_participant_during_compaction_writes_nothing() {
    let (store, agent, service, _channels, channel) = setup().await;
    seed_large_entries(&store, &channel, &agent.id, 6).await;

    let llm = Arc::new(GatedLlm::new());
    let task = {
        let service = service.clone();
        let channel_id = channel.id.clone();
        let llm = llm.clone();
        tokio::spawn(async move { service.compact(&channel_id, llm, "recording-model").await })
    };
    tokio::time::timeout(Duration::from_secs(5), llm.entered.notified())
        .await
        .expect("the summarizer should be requested");

    let cleared = service
        .clear_participant_context(&channel.id, &agent.id)
        .await
        .unwrap();
    assert!(cleared >= 6);
    llm.release.notify_one();

    let outcome = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("compaction should finish")
        .unwrap()
        .unwrap();
    assert!(!outcome.compacted);
    assert!(outcome.reason.is_some());

    let entries = store
        .list_context_entries(&channel.id, &agent.id)
        .await
        .unwrap();
    assert!(!entries
        .iter()
        .any(|entry| entry.kind == ContextEntryKind::Summary));
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

#[tokio::test]
async fn group_creation_validates_participants_and_default_responder() {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let context = bootstrap_local(&store).await.unwrap();
    let planner = AgentProfile::new("Planner", "Architect", "Plan", "Careful");
    let coder = AgentProfile::new("Coder", "Engineer", "Ship", "Careful");
    for agent in [&planner, &coder] {
        store
            .upsert_agent(&context.workspace.id, agent)
            .await
            .unwrap();
    }
    let bus = EventBus::default_bus();
    let service = ConversationService::new(store.clone(), bus, context.workspace.id.clone());

    assert!(service
        .create_group(
            None,
            std::slice::from_ref(&planner.id),
            None,
            "mock-model",
            None
        )
        .await
        .is_err());
    assert!(service
        .create_group(
            None,
            &[planner.id.clone(), planner.id.clone()],
            None,
            "mock-model",
            None
        )
        .await
        .is_err());
    assert!(service
        .create_group(
            None,
            &[planner.id.clone(), coder.id.clone()],
            None,
            "mock-model",
            Some(AgentId::new("missing-agent"))
        )
        .await
        .is_err());
    let too_many = vec![planner.id.clone(); 9];
    assert!(service
        .create_group(None, &too_many, None, "mock-model", None)
        .await
        .is_err());

    let channel = service
        .create_group(
            Some("Valid crew"),
            &[planner.id.clone(), coder.id.clone()],
            Some("provider-1".to_string()),
            "mock-model",
            Some(coder.id.clone()),
        )
        .await
        .unwrap();
    let settings = service.settings(&channel.id).await.unwrap();
    assert_eq!(settings.mode, ConversationMode::Group);
    assert_eq!(settings.default_responder.as_ref(), Some(&coder.id));
    let stored = store.list_channel_participants(&channel.id).await.unwrap();
    assert_eq!(
        stored
            .iter()
            .map(|participant| participant.position)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
}

#[tokio::test]
async fn group_mention_routes_only_to_the_named_participant() {
    let fixture = group_fixture(None).await;
    let message = post_user(
        &fixture.channels,
        &fixture.channel,
        "@Coder plan the release",
    )
    .await;

    let report = fixture
        .service
        .reply(
            &fixture.channel.id,
            &message,
            Arc::new(RecordingLlm::new()),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    let coder = fixture.participants[1].id.clone();
    assert_eq!(report.agents, vec![coder.clone()]);
    assert_eq!(report.tasks.len(), 1);
    assert_eq!(report.replied, vec![coder.clone()]);
    assert!(report.unknown_mentions.is_empty());

    let messages = fixture
        .channels
        .list_messages(&fixture.channel.id, 50)
        .await
        .unwrap();
    assert_eq!(agent_replies(&messages), vec![coder.clone()]);

    let turn = fixture
        .store
        .get_conversation_turn(&fixture.channel.id, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.source, RouteSource::Mention);
    assert_eq!(turn.selected_agents, vec![coder.clone()]);
    assert_eq!(turn.task_ids, report.tasks);
    assert!(turn.mirrored_at.is_some());

    // The user message enters every branch; only the responder gets an answer.
    for agent in &fixture.participants {
        let entries = fixture
            .store
            .list_context_entries(&fixture.channel.id, &agent.id)
            .await
            .unwrap();
        assert!(entries.iter().any(|entry| {
            entry.role == ContextRole::User
                && entry.content == "@Coder plan the release"
                && entry.author_agent_id.is_none()
        }));
    }
    let planner_entries = fixture
        .store
        .list_context_entries(&fixture.channel.id, &fixture.participants[0].id)
        .await
        .unwrap();
    assert!(!planner_entries
        .iter()
        .any(|entry| entry.role == ContextRole::Assistant));
}

#[tokio::test]
async fn group_broadcast_routes_to_every_participant_and_mirrors_peer_replies() {
    let fixture = group_fixture(None).await;
    let message = post_user(&fixture.channels, &fixture.channel, "@team status update").await;

    let report = fixture
        .service
        .reply(
            &fixture.channel.id,
            &message,
            Arc::new(RecordingLlm::new()),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    assert_eq!(report.agents.len(), 2);
    assert_eq!(report.tasks.len(), 2);
    assert_eq!(report.replied.len(), 2);
    assert!(report.failed.is_empty());
    assert!(report.skipped.is_empty());

    let messages = fixture
        .channels
        .list_messages(&fixture.channel.id, 50)
        .await
        .unwrap();
    let replied = agent_replies(&messages);
    assert_eq!(replied.len(), 2);

    // Each branch keeps the other participant's settled reply as authored
    // teammate history, never as the user's words.
    for (index, agent) in fixture.participants.iter().enumerate() {
        let other = &fixture.participants[1 - index];
        let entries = fixture
            .store
            .list_context_entries(&fixture.channel.id, &agent.id)
            .await
            .unwrap();
        let mirrored = entries
            .iter()
            .find(|entry| entry.author_agent_id.as_ref() == Some(&other.id))
            .expect("peer reply is mirrored into every other branch");
        assert_eq!(mirrored.role, ContextRole::User);
        assert!(mirrored.content.starts_with(&format!("[{}]", other.name)));
    }
}

#[tokio::test]
async fn group_unknown_mentions_are_reported_without_a_reply() {
    let fixture = group_fixture(None).await;
    let message = post_user(&fixture.channels, &fixture.channel, "@Ghost are you there?").await;

    let report = fixture
        .service
        .reply(
            &fixture.channel.id,
            &message,
            Arc::new(RecordingLlm::new()),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    assert!(report.agents.is_empty());
    assert!(report.tasks.is_empty());
    assert_eq!(report.unknown_mentions, vec!["Ghost".to_string()]);

    let messages = fixture
        .channels
        .list_messages(&fixture.channel.id, 50)
        .await
        .unwrap();
    assert!(agent_replies(&messages).is_empty());
    assert!(messages.iter().any(|message| {
        message.sender == Sender::System
            && message.content.contains("@Ghost")
            && message.content.contains("@Planner")
    }));

    let turn = fixture
        .store
        .get_conversation_turn(&fixture.channel.id, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.source, RouteSource::Unmatched);
    assert!(turn.selected_agents.is_empty());
    assert_eq!(turn.unknown_mentions, vec!["Ghost".to_string()]);
}

#[tokio::test]
async fn group_mixed_mentions_answer_and_report_the_unknown_names() {
    let fixture = group_fixture(None).await;
    let message = post_user(&fixture.channels, &fixture.channel, "@Coder @Ghost ship it").await;

    let llm = RecordingLlm::new();
    let report = fixture
        .service
        .reply(
            &fixture.channel.id,
            &message,
            Arc::new(llm),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    assert_eq!(report.agents, vec![fixture.participants[1].id.clone()]);
    assert_eq!(report.unknown_mentions, vec!["Ghost".to_string()]);
    let messages = fixture
        .channels
        .list_messages(&fixture.channel.id, 50)
        .await
        .unwrap();
    assert_eq!(
        agent_replies(&messages),
        vec![fixture.participants[1].id.clone()]
    );
    assert!(messages
        .iter()
        .any(|message| message.sender == Sender::System && message.content.contains("@Ghost")));
    let turn = fixture
        .store
        .get_conversation_turn(&fixture.channel.id, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.source, RouteSource::Mention);
    assert_eq!(turn.unknown_mentions, vec!["Ghost".to_string()]);
}

#[tokio::test]
async fn group_without_a_mention_or_default_responder_is_silent() {
    let fixture = group_fixture(None).await;
    let message = post_user(&fixture.channels, &fixture.channel, "status update").await;

    let report = fixture
        .service
        .reply(
            &fixture.channel.id,
            &message,
            Arc::new(RecordingLlm::new()),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    assert!(report.agents.is_empty());
    assert!(report.tasks.is_empty());
    let messages = fixture
        .channels
        .list_messages(&fixture.channel.id, 50)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1, "a silent turn posts no notice");
    assert!(agent_replies(&messages).is_empty());

    let turn = fixture
        .store
        .get_conversation_turn(&fixture.channel.id, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.source, RouteSource::Silent);
    assert!(turn.task_ids.is_empty());
}

#[tokio::test]
async fn group_default_responder_answers_an_unaddressed_message() {
    let fixture = group_fixture(None).await;
    let planner = fixture.participants[0].id.clone();
    fixture
        .service
        .update_settings(
            &fixture.channel.id,
            &ConversationSettingsUpdate {
                auto_compact: None,
                compact_threshold: None,
                router_enabled: None,
                default_responder: Some(planner.clone()),
            },
        )
        .await
        .unwrap();

    let message = post_user(&fixture.channels, &fixture.channel, "status update").await;
    let report = fixture
        .service
        .reply(
            &fixture.channel.id,
            &message,
            Arc::new(RecordingLlm::new()),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    assert_eq!(report.agents, vec![planner.clone()]);
    assert_eq!(report.replied, vec![planner.clone()]);
    let messages = fixture
        .channels
        .list_messages(&fixture.channel.id, 50)
        .await
        .unwrap();
    assert_eq!(agent_replies(&messages), vec![planner.clone()]);
    let turn = fixture
        .store
        .get_conversation_turn(&fixture.channel.id, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.source, RouteSource::Default);
}

#[tokio::test]
async fn stale_default_responder_falls_back_to_main_with_a_notice() {
    let main = AgentProfile::new("main", "Primary Assistant", "Coordinate", "Careful");
    let planner = AgentProfile::new("Planner", "Architect", "Plan", "Careful");
    let coder = AgentProfile::new("Coder", "Engineer", "Ship", "Careful");
    let participants = vec![planner, coder, main.clone()];
    let fixture = group_fixture_with(participants, None).await;

    let mut settings = fixture.service.settings(&fixture.channel.id).await.unwrap();
    settings.default_responder = Some(AgentId::new("missing-agent"));
    fixture
        .store
        .update_conversation_settings(&settings)
        .await
        .unwrap();

    let message = post_user(&fixture.channels, &fixture.channel, "status update").await;
    let report = fixture
        .service
        .reply(
            &fixture.channel.id,
            &message,
            Arc::new(RecordingLlm::new()),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    assert_eq!(report.agents, vec![main.id.clone()]);
    let messages = fixture
        .channels
        .list_messages(&fixture.channel.id, 50)
        .await
        .unwrap();
    assert!(messages.iter().any(|message| {
        message.sender == Sender::System
            && message.content.contains("no longer a participant")
            && message.content.contains("main")
    }));
}

#[tokio::test]
async fn group_capacity_skips_a_full_branch_and_answers_with_the_others() {
    let fixture = group_fixture(None).await;
    let message = post_user(&fixture.channels, &fixture.channel, "@team status update").await;
    let planner = fixture.participants[0].id.clone();
    let coder = fixture.participants[1].id.clone();

    let mut turn_options = options();
    turn_options.participant_capacity = vec![ParticipantCapacity {
        agent_id: planner.clone(),
        usable_window: 1,
        tool_tokens: 0,
    }];
    let report = fixture
        .service
        .reply(
            &fixture.channel.id,
            &message,
            Arc::new(RecordingLlm::new()),
            ToolRegistry::new(),
            &turn_options,
        )
        .await
        .unwrap();

    assert_eq!(report.skipped, vec![planner.clone()]);
    assert_eq!(report.agents, vec![coder.clone()]);
    assert_eq!(report.replied, vec![coder.clone()]);
    let messages = fixture
        .channels
        .list_messages(&fixture.channel.id, 50)
        .await
        .unwrap();
    assert!(messages.iter().any(|message| {
        message.sender == Sender::System && message.content.contains("Planner was skipped")
    }));
    let turn = fixture
        .store
        .get_conversation_turn(&fixture.channel.id, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.selected_agents, vec![coder]);
}

#[tokio::test]
async fn group_per_participant_context_and_clear() {
    let fixture = group_fixture(None).await;
    let message = post_user(&fixture.channels, &fixture.channel, "@team status update").await;
    fixture
        .service
        .reply(
            &fixture.channel.id,
            &message,
            Arc::new(RecordingLlm::new()),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    let context = fixture.service.context(&fixture.channel.id).await.unwrap();
    assert_eq!(context.participants.len(), 2);
    assert_eq!(context.agent_id, fixture.participants[0].id);
    assert_eq!(context.participants[0].agent_name, "Planner");
    assert_eq!(context.participants[1].agent_id, fixture.participants[1].id);
    assert!(context.participants[0].projection.active_entries > 0);
    assert!(context.participants[1].projection.active_entries > 0);
    assert!(context.participants[0].cache_last_run.calls > 0);

    let coder = fixture.participants[1].id.clone();
    let cleared = fixture
        .service
        .clear_participant_context(&fixture.channel.id, &coder)
        .await
        .unwrap();
    assert!(cleared > 0);

    let coder_entries = fixture
        .store
        .list_context_entries(&fixture.channel.id, &coder)
        .await
        .unwrap();
    assert!(coder_entries.iter().all(|entry| entry.excluded));
    let planner_entries = fixture
        .store
        .list_context_entries(&fixture.channel.id, &fixture.participants[0].id)
        .await
        .unwrap();
    assert!(planner_entries.iter().any(|entry| !entry.excluded));

    let context = fixture.service.context(&fixture.channel.id).await.unwrap();
    assert_eq!(context.participants[1].projection.active_entries, 0);
    assert!(context.participants[0].projection.active_entries > 0);
}

#[tokio::test]
async fn group_auto_compacts_a_branch_before_answering() {
    let fixture = group_fixture(None).await;
    let planner = fixture.participants[0].clone();
    for index in 0..6 {
        let role = if index % 2 == 0 {
            ContextRole::User
        } else {
            ContextRole::Assistant
        };
        let mut entry = ContextEntry::new(
            fixture.channel.id.clone(),
            planner.id.clone(),
            role,
            format!("turn {index} {}", "x".repeat(4_000)),
        );
        entry.recount();
        fixture.store.append_context_entry(&entry).await.unwrap();
    }
    let mut settings = fixture.service.settings(&fixture.channel.id).await.unwrap();
    settings.auto_compact = true;
    settings.compact_threshold = 1;
    fixture
        .store
        .update_conversation_settings(&settings)
        .await
        .unwrap();

    let message = post_user(&fixture.channels, &fixture.channel, "@Planner status").await;
    let mut turn_options = options();
    turn_options.participant_capacity = vec![
        ParticipantCapacity {
            agent_id: planner.id.clone(),
            usable_window: 60_000,
            tool_tokens: 0,
        },
        ParticipantCapacity {
            agent_id: fixture.participants[1].id.clone(),
            usable_window: 60_000,
            tool_tokens: 0,
        },
    ];
    let report = fixture
        .service
        .reply(
            &fixture.channel.id,
            &message,
            Arc::new(RecordingLlm::new()),
            ToolRegistry::new(),
            &turn_options,
        )
        .await
        .unwrap();

    assert_eq!(report.replied, vec![planner.id.clone()]);
    let messages = fixture
        .channels
        .list_messages(&fixture.channel.id, 50)
        .await
        .unwrap();
    assert!(messages.iter().any(|message| {
        message.sender == Sender::System
            && message
                .content
                .contains("Context compacted automatically for Planner")
    }));
    let entries = fixture
        .store
        .list_context_entries(&fixture.channel.id, &planner.id)
        .await
        .unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.kind == ContextEntryKind::Summary));
}

#[tokio::test]
async fn group_partial_failure_keeps_the_other_answers() {
    let participants = vec![
        AgentProfile::new("Planner", "Architect", "Plan", "Careful"),
        AgentProfile::new("Coder", "Engineer", "Ship", "fails-on-request-marker"),
    ];
    let fixture = group_fixture_with(participants, None).await;
    let planner = fixture.participants[0].id.clone();
    let coder = fixture.participants[1].id.clone();
    let message = post_user(&fixture.channels, &fixture.channel, "@team status update").await;

    let report = fixture
        .service
        .reply(
            &fixture.channel.id,
            &message,
            Arc::new(FailingLlm {
                fail_on: "fails-on-request-marker".to_string(),
            }),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    assert_eq!(report.replied, vec![planner.clone()]);
    assert_eq!(report.failed, vec![coder.clone()]);
    let messages = fixture
        .channels
        .list_messages(&fixture.channel.id, 50)
        .await
        .unwrap();
    assert_eq!(agent_replies(&messages), vec![planner]);
    assert!(messages.iter().any(|message| {
        message.sender == Sender::System && message.content.contains("Coder could not answer")
    }));
}

async fn enable_router(fixture: &GroupFixture, default_responder: Option<AgentId>) {
    fixture
        .service
        .update_settings(
            &fixture.channel.id,
            &ConversationSettingsUpdate {
                auto_compact: None,
                compact_threshold: None,
                router_enabled: Some(true),
                default_responder,
            },
        )
        .await
        .unwrap();
}

async fn run_routed_turn(
    fixture: &GroupFixture,
    llm: Arc<RouterLlm>,
    content: &str,
) -> (ChannelMessage, cowbot_service::ChannelTurnReport) {
    let message = post_user(&fixture.channels, &fixture.channel, content).await;
    let report = fixture
        .service
        .reply(
            &fixture.channel.id,
            &message,
            llm,
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();
    (message, report)
}

#[tokio::test]
async fn router_selects_participants_and_records_usage() {
    let fixture = group_fixture(None).await;
    enable_router(&fixture, None).await;
    let coder = fixture.participants[1].id.clone();
    let llm = Arc::new(RouterLlm::new(RouterReply::Json(
        r#"{"agents":["Coder"],"reason":"implementation work"}"#,
    )));

    let (message, report) = run_routed_turn(&fixture, llm.clone(), "who should take this?").await;

    assert_eq!(report.agents, vec![coder.clone()]);
    assert_eq!(report.replied, vec![coder.clone()]);
    let turn = fixture
        .store
        .get_conversation_turn(&fixture.channel.id, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.source, RouteSource::Router);
    assert_eq!(turn.selected_agents, vec![coder.clone()]);
    assert_eq!(turn.reason.as_deref(), Some("implementation work"));
    assert!(turn.router_call_id.is_some());

    let requests = llm.router_requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.temperature, Some(0.0));
    assert!(request.tools.is_empty());
    let prompt = &request.messages[1].content;
    assert!(prompt.contains("- Planner — Architect: Plan"));
    assert!(prompt.contains("- Coder — Engineer: Ship"));
    assert!(prompt.contains("Latest message from the user:\nwho should take this?"));

    // Router usage is billed to the channel but never to the agent's run.
    let total = fixture
        .store
        .sum_llm_usage_for_channel(&fixture.channel.id)
        .await
        .unwrap();
    assert_eq!(total.calls, 2);
    let coder_usage = fixture
        .store
        .latest_run_usage_for_channel_agent(&fixture.channel.id, &coder)
        .await
        .unwrap();
    assert_eq!(coder_usage.calls, 1);
}

#[tokio::test]
async fn router_accepts_a_single_agent_name_case_insensitively() {
    let fixture = group_fixture(None).await;
    enable_router(&fixture, None).await;
    let coder = fixture.participants[1].id.clone();
    let llm = Arc::new(RouterLlm::new(RouterReply::Json(
        r#"{"agent":"coder","reason":"coding task"}"#,
    )));

    let (message, report) = run_routed_turn(&fixture, llm, "who should take this?").await;

    assert_eq!(report.agents, vec![coder.clone()]);
    let turn = fixture
        .store
        .get_conversation_turn(&fixture.channel.id, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.source, RouteSource::Router);
    assert_eq!(turn.selected_agents, vec![coder]);
    assert_eq!(turn.reason.as_deref(), Some("coding task"));
}

#[tokio::test]
async fn explicit_mentions_skip_the_router() {
    let fixture = group_fixture(None).await;
    enable_router(&fixture, None).await;
    let coder = fixture.participants[1].id.clone();
    let llm = Arc::new(RouterLlm::new(RouterReply::Json(
        r#"{"agents":["Planner"]}"#,
    )));

    let (message, report) = run_routed_turn(&fixture, llm.clone(), "@Coder ship it").await;

    assert_eq!(report.agents, vec![coder]);
    assert!(llm.router_requests().is_empty());
    let turn = fixture
        .store
        .get_conversation_turn(&fixture.channel.id, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.source, RouteSource::Mention);
}

#[tokio::test]
async fn router_invalid_json_falls_back_to_the_default_responder() {
    let fixture = group_fixture(None).await;
    let planner = fixture.participants[0].id.clone();
    enable_router(&fixture, Some(planner.clone())).await;
    let llm = Arc::new(RouterLlm::new(RouterReply::Json("not json at all")));

    let (message, report) = run_routed_turn(&fixture, llm, "who should take this?").await;

    assert_eq!(report.agents, vec![planner]);
    let messages = fixture
        .channels
        .list_messages(&fixture.channel.id, 50)
        .await
        .unwrap();
    assert!(messages.iter().any(|message| {
        message.sender == Sender::System
            && message.content.contains("Routing failed")
            && message.content.contains("the router returned invalid JSON")
    }));
    let turn = fixture
        .store
        .get_conversation_turn(&fixture.channel.id, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.source, RouteSource::Default);
    assert_eq!(
        turn.reason.as_deref(),
        Some("the router returned invalid JSON")
    );
    assert!(turn.router_call_id.is_some());
}

#[tokio::test]
async fn router_empty_selection_falls_back_to_the_default_responder() {
    let fixture = group_fixture(None).await;
    let planner = fixture.participants[0].id.clone();
    enable_router(&fixture, Some(planner.clone())).await;
    let llm = Arc::new(RouterLlm::new(RouterReply::Json(
        r#"{"agents":[],"reason":"nobody fits"}"#,
    )));

    let (message, report) = run_routed_turn(&fixture, llm, "who should take this?").await;

    assert_eq!(report.agents, vec![planner]);
    let turn = fixture
        .store
        .get_conversation_turn(&fixture.channel.id, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.source, RouteSource::Default);
    assert_eq!(turn.reason.as_deref(), Some("the router selected nobody"));
}

#[tokio::test]
async fn router_unknown_names_fall_back_to_the_default_responder() {
    let fixture = group_fixture(None).await;
    let planner = fixture.participants[0].id.clone();
    enable_router(&fixture, Some(planner.clone())).await;
    let llm = Arc::new(RouterLlm::new(RouterReply::Json(
        r#"{"agents":["Ghost"],"reason":"unknown"}"#,
    )));

    let (message, report) = run_routed_turn(&fixture, llm, "who should take this?").await;

    assert_eq!(report.agents, vec![planner]);
    let turn = fixture
        .store
        .get_conversation_turn(&fixture.channel.id, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.source, RouteSource::Default);
    assert_eq!(
        turn.reason.as_deref(),
        Some("the router named no participant")
    );
}

#[tokio::test]
async fn router_provider_error_falls_back_to_the_default_responder() {
    let fixture = group_fixture(None).await;
    let planner = fixture.participants[0].id.clone();
    enable_router(&fixture, Some(planner.clone())).await;
    let llm = Arc::new(RouterLlm::new(RouterReply::Error));

    let (message, report) = run_routed_turn(&fixture, llm, "who should take this?").await;

    assert_eq!(report.agents, vec![planner]);
    let turn = fixture
        .store
        .get_conversation_turn(&fixture.channel.id, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.source, RouteSource::Default);
    let reason = turn.reason.unwrap_or_default();
    assert!(
        reason.contains("the router call failed"),
        "unexpected reason: {reason}"
    );
}
