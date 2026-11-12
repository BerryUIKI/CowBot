use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cowbot_bus::EventBus;
use cowbot_core::agent::AgentProfile;
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::assignment::{AssignmentOrigin, AssignmentRequester};
use cowbot_core::channel::Sender;
use cowbot_core::error::CowResult;
use cowbot_core::identity::WorkspaceId;
use cowbot_core::memory::{MemoryEntry, MemoryKind, MemorySource, MemoryStatus};
use cowbot_core::task::Task;
use cowbot_core::usage::{LlmCallKind, TokenUsage};
use cowbot_llm::{
    ChatCompletionRequest, ChatCompletionResponse, ChatRole, LlmProvider, LlmToolCall,
};
use cowbot_service::{
    bootstrap_local, AssignmentScheduler, ChannelAgentService, ChannelService, ChannelTurnOptions,
    ConversationService, CrewRunOptions, MemoryFilter, MemoryService, MemorySettingsUpdate,
    MemoryUpdate, NewMemory,
};
use cowbot_store::Store;
use cowbot_tools::ToolRegistry;

/// Opening line of the extraction request, used to tell the two calls apart.
const EXTRACTION_PROMPT_PREFIX: &str = "You maintain the long-term memory";

struct RecordingLlm {
    requests: Mutex<Vec<ChatCompletionRequest>>,
}

impl RecordingLlm {
    fn new() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
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
        Ok(ChatCompletionResponse {
            content: Some("answer".to_string()),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_string()),
            usage: Some(TokenUsage {
                uncached_input_tokens: 10,
                output_tokens: 5,
                ..TokenUsage::default()
            }),
            raw_usage: None,
        })
    }
}

/// A provider that answers agent turns and extraction calls differently.
struct FakeLlm {
    requests: Mutex<Vec<ChatCompletionRequest>>,
    agent_reply: String,
    extraction_reply: Option<String>,
    extraction_fails: bool,
    next_tool_call: Mutex<Option<LlmToolCall>>,
}

impl FakeLlm {
    fn reply(agent_reply: &str) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            agent_reply: agent_reply.to_string(),
            extraction_reply: None,
            extraction_fails: false,
            next_tool_call: Mutex::new(None),
        }
    }

    fn with_extraction(mut self, reply: &str) -> Self {
        self.extraction_reply = Some(reply.to_string());
        self
    }

    fn with_failed_extraction(mut self) -> Self {
        self.extraction_fails = true;
        self
    }

    fn with_tool_call(self, call: LlmToolCall) -> Self {
        *self.next_tool_call.lock().unwrap() = Some(call);
        self
    }

    fn requests(&self) -> Vec<ChatCompletionRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn extraction_requests(&self) -> Vec<ChatCompletionRequest> {
        self.requests()
            .into_iter()
            .filter(|request| {
                request
                    .messages
                    .first()
                    .is_some_and(|message| message.content.starts_with(EXTRACTION_PROMPT_PREFIX))
            })
            .collect()
    }
}

#[async_trait]
impl LlmProvider for FakeLlm {
    fn name(&self) -> &str {
        "fake"
    }

    async fn chat_complete(
        &self,
        request: &ChatCompletionRequest,
    ) -> CowResult<ChatCompletionResponse> {
        self.requests.lock().unwrap().push(request.clone());
        let is_extraction = request
            .messages
            .first()
            .is_some_and(|message| message.content.starts_with(EXTRACTION_PROMPT_PREFIX));
        if is_extraction {
            if self.extraction_fails {
                return Err(cowbot_core::error::CowError::Llm(
                    "extraction provider failed".to_string(),
                ));
            }
            return Ok(ChatCompletionResponse {
                content: self.extraction_reply.clone(),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_string()),
                usage: Some(TokenUsage {
                    uncached_input_tokens: 10,
                    output_tokens: 5,
                    ..TokenUsage::default()
                }),
                raw_usage: None,
            });
        }

        let tool_call = self.next_tool_call.lock().unwrap().take();
        Ok(ChatCompletionResponse {
            content: Some(self.agent_reply.clone()),
            tool_calls: tool_call.into_iter().collect(),
            finish_reason: Some("stop".to_string()),
            usage: Some(TokenUsage {
                uncached_input_tokens: 10,
                output_tokens: 5,
                ..TokenUsage::default()
            }),
            raw_usage: None,
        })
    }
}

struct Fixture {
    store: Store,
    bus: EventBus,
    workspace_id: WorkspaceId,
    memory: MemoryService,
    conversations: ConversationService,
    channels: ChannelService,
    planner: AgentProfile,
    coder: AgentProfile,
}

async fn fixture() -> Fixture {
    fixture_with_planner_tools(&[]).await
}

async fn fixture_with_planner_tools(tools: &[&str]) -> Fixture {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let context = bootstrap_local(&store).await.unwrap();
    let mut planner = AgentProfile::new("Planner", "Architect", "Plan", "Careful");
    for tool in tools {
        planner.tools.push((*tool).to_string());
    }
    let coder = AgentProfile::new("Coder", "Engineer", "Ship", "Careful");
    for agent in [&planner, &coder] {
        store
            .upsert_agent(&context.workspace.id, agent)
            .await
            .unwrap();
    }
    let bus = EventBus::default_bus();
    Fixture {
        memory: MemoryService::new(store.clone(), context.workspace.id.clone()),
        conversations: ConversationService::new(
            store.clone(),
            bus.clone(),
            context.workspace.id.clone(),
        ),
        channels: ChannelService::new(store.clone(), bus.clone()),
        store,
        bus,
        workspace_id: context.workspace.id,
        planner,
        coder,
    }
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

fn memory_messages(request: &ChatCompletionRequest) -> Vec<&str> {
    request
        .messages
        .iter()
        .filter(|message| message.content.starts_with("Memory (reference data"))
        .map(|message| message.content.as_str())
        .collect()
}

#[tokio::test]
async fn single_turn_injects_notes_and_active_memory_only() {
    let fixture = fixture().await;
    let conversation = fixture
        .conversations
        .create(Some("Memory Lab"), &fixture.planner.id, None, "mock-model")
        .await
        .unwrap();

    fixture
        .memory
        .save_workspace_note("Be concise.")
        .await
        .unwrap();
    fixture
        .memory
        .save_conversation_note(&conversation.id, "Use the design doc.")
        .await
        .unwrap();
    let active = fixture
        .memory
        .add_entry(
            NewMemory::user("The user prefers Rust for services.")
                .with_scope(Some(fixture.planner.id.clone()), None),
        )
        .await
        .unwrap()
        .entry;
    let pending = MemoryEntry::candidate(
        fixture.workspace_id.clone(),
        Some(fixture.planner.id.clone()),
        None,
        MemoryKind::Fact,
        "pending memory about Rust",
    );
    fixture.store.insert_memory_entry(&pending).await.unwrap();
    let archived = fixture
        .memory
        .add_entry(
            NewMemory::user("archived memory about Rust")
                .with_scope(Some(fixture.planner.id.clone()), None),
        )
        .await
        .unwrap()
        .entry;
    fixture.memory.archive_entry(&archived.id).await.unwrap();

    let message = fixture
        .channels
        .post_message(
            &conversation.id,
            Sender::User("user-1".to_string()),
            "Which language should the new service use?",
        )
        .await
        .unwrap();
    let llm = Arc::new(RecordingLlm::new());
    fixture
        .conversations
        .reply(
            &conversation.id,
            &message,
            llm.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    let requests = llm.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].messages[0].role, cowbot_llm::ChatRole::System);
    let blocks = memory_messages(&requests[0]);
    assert_eq!(blocks.len(), 1, "the memory block is a system message");
    let block = blocks[0];
    let workspace_position = block.find("Be concise.").expect("workspace note");
    let conversation_position = block
        .find("Use the design doc.")
        .expect("conversation note");
    assert!(
        workspace_position < conversation_position,
        "the workspace note precedes the conversation note"
    );
    assert!(block.contains("prefers Rust"));
    assert!(!block.contains("pending memory"));
    assert!(!block.contains("archived memory"));

    let reloaded = fixture
        .store
        .get_memory_entry(&active.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.recall_count, 1);
}

#[tokio::test]
async fn group_turn_recalls_each_participant_separately() {
    let fixture = fixture().await;
    let channel = fixture
        .conversations
        .create_group(
            Some("Crew"),
            &[fixture.planner.id.clone(), fixture.coder.id.clone()],
            None,
            "mock-model",
            Some(fixture.planner.id.clone()),
        )
        .await
        .unwrap();

    fixture
        .memory
        .add_entry(
            NewMemory::user("Planner-only memory about Rust")
                .with_scope(Some(fixture.planner.id.clone()), Some(channel.id.clone())),
        )
        .await
        .unwrap();
    fixture
        .memory
        .add_entry(
            NewMemory::user("Coder-only memory about Rust")
                .with_scope(Some(fixture.coder.id.clone()), Some(channel.id.clone())),
        )
        .await
        .unwrap();
    fixture
        .memory
        .add_entry(
            NewMemory::user("Shared memory about Rust").with_scope(None, Some(channel.id.clone())),
        )
        .await
        .unwrap();

    let message = fixture
        .channels
        .post_message(
            &channel.id,
            Sender::User("user-1".to_string()),
            "@team Rust status",
        )
        .await
        .unwrap();
    let llm = Arc::new(RecordingLlm::new());
    fixture
        .conversations
        .reply(
            &channel.id,
            &message,
            llm.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    let requests = llm.requests();
    assert_eq!(requests.len(), 2);
    let blocks = requests
        .iter()
        .flat_map(memory_messages)
        .collect::<Vec<&str>>();
    assert_eq!(blocks.len(), 2);
    let planner_block = blocks
        .iter()
        .find(|block| block.contains("Planner-only"))
        .expect("the planner recalls its own entry");
    assert!(planner_block.contains("Shared memory"));
    assert!(!planner_block.contains("Coder-only"));

    let coder_block = blocks
        .iter()
        .find(|block| block.contains("Coder-only"))
        .expect("the coder recalls its own entry");
    assert!(coder_block.contains("Shared memory"));
    assert!(!coder_block.contains("Planner-only"));
}

#[tokio::test]
async fn direct_conversation_injects_memory_into_the_task_description() {
    let fixture = fixture().await;
    let direct = fixture
        .channels
        .ensure_agent_channel(&fixture.workspace_id, &fixture.planner)
        .await
        .unwrap();
    fixture
        .memory
        .save_workspace_note("Direct note.")
        .await
        .unwrap();
    fixture
        .memory
        .add_entry(
            NewMemory::user("The user prefers Rust.")
                .with_scope(Some(fixture.planner.id.clone()), None),
        )
        .await
        .unwrap();

    let message = fixture
        .channels
        .post_message(
            &direct.id,
            Sender::User("user-1".to_string()),
            "Do I still prefer Rust for services?",
        )
        .await
        .unwrap();
    let agents = ChannelAgentService::new(
        fixture.store.clone(),
        fixture.bus.clone(),
        fixture.workspace_id.clone(),
    );
    let llm = Arc::new(RecordingLlm::new());
    agents
        .reply(
            &direct.id,
            &message,
            llm.clone(),
            ToolRegistry::new(),
            "mock-model",
            &options(),
        )
        .await
        .unwrap();

    let requests = llm.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].messages.iter().any(|message| {
        message.content.contains("Memory (reference data")
            && message.content.contains("Direct note.")
            && message.content.contains("prefers Rust")
    }));
}

#[tokio::test]
async fn channel_mention_turns_do_not_receive_memory() {
    let fixture = fixture().await;
    let channel = fixture
        .channels
        .create_channel(&fixture.workspace_id, "general", "Team")
        .await
        .unwrap();
    fixture
        .memory
        .save_workspace_note("Channel note.")
        .await
        .unwrap();
    fixture
        .memory
        .add_entry(NewMemory::user("Pinned memory about Rust").with_pinned(true))
        .await
        .unwrap();

    let message = fixture
        .channels
        .post_message(
            &channel.id,
            Sender::User("user-1".to_string()),
            "@Planner status",
        )
        .await
        .unwrap();
    let agents = ChannelAgentService::new(
        fixture.store.clone(),
        fixture.bus.clone(),
        fixture.workspace_id.clone(),
    );
    let llm = Arc::new(RecordingLlm::new());
    agents
        .reply(
            &channel.id,
            &message,
            llm.clone(),
            ToolRegistry::new(),
            "mock-model",
            &options(),
        )
        .await
        .unwrap();

    assert!(llm
        .requests()
        .iter()
        .all(|request| memory_messages(request).is_empty()));
}

#[tokio::test]
async fn disabled_recall_removes_the_memory_block() {
    let fixture = fixture().await;
    let conversation = fixture
        .conversations
        .create(Some("Quiet"), &fixture.planner.id, None, "mock-model")
        .await
        .unwrap();
    fixture
        .memory
        .save_workspace_note("Should not appear.")
        .await
        .unwrap();
    fixture
        .memory
        .update_settings(&MemorySettingsUpdate {
            recall_enabled: Some(false),
            ..MemorySettingsUpdate::default()
        })
        .await
        .unwrap();

    let message = fixture
        .channels
        .post_message(
            &conversation.id,
            Sender::User("user-1".to_string()),
            "hello",
        )
        .await
        .unwrap();
    let llm = Arc::new(RecordingLlm::new());
    fixture
        .conversations
        .reply(
            &conversation.id,
            &message,
            llm.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();
    assert!(memory_messages(&llm.requests()[0]).is_empty());

    // Re-enabling brings the note back, so the test proves the switch.
    fixture
        .memory
        .update_settings(&MemorySettingsUpdate {
            recall_enabled: Some(true),
            ..MemorySettingsUpdate::default()
        })
        .await
        .unwrap();
    let llm = Arc::new(RecordingLlm::new());
    fixture
        .conversations
        .reply(
            &conversation.id,
            &message,
            llm.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();
    let requests = llm.requests();
    let blocks = memory_messages(&requests[0]);
    assert_eq!(blocks.len(), 1);
    assert!(blocks[0].contains("Should not appear."));
}

#[tokio::test]
async fn service_operations_validate_redact_and_isolate() {
    let fixture = fixture().await;

    let redacted = fixture
        .memory
        .add_entry(NewMemory::user("api_key = sk-live-123456"))
        .await
        .unwrap();
    assert!(redacted.redacted);
    assert!(redacted.entry.content.contains("[redacted]"));
    assert!(!redacted.entry.content.contains("sk-live"));

    let note = fixture
        .memory
        .save_workspace_note("password: hunter2")
        .await
        .unwrap();
    assert!(note.redacted);
    assert!(note.note.content.contains("[redacted]"));

    assert!(fixture
        .memory
        .add_entry(NewMemory::user("x".repeat(2_001)))
        .await
        .is_err());

    let group_channel = fixture
        .channels
        .create_channel(&fixture.workspace_id, "group", "")
        .await
        .unwrap();
    assert!(fixture
        .memory
        .save_conversation_note(&group_channel.id, "not a conversation note")
        .await
        .is_err());

    let pending = MemoryEntry::candidate(
        fixture.workspace_id.clone(),
        None,
        None,
        MemoryKind::Preference,
        "pending candidate",
    );
    fixture.store.insert_memory_entry(&pending).await.unwrap();
    let listed = fixture
        .memory
        .list_entries(&MemoryFilter {
            status: Some(MemoryStatus::Pending),
            ..MemoryFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    let approved = fixture.memory.approve_entry(&pending.id).await.unwrap();
    assert_eq!(approved.status, MemoryStatus::Active);
    let updated = fixture
        .memory
        .update_entry(
            &pending.id,
            &MemoryUpdate {
                content: Some("edited candidate".to_string()),
                pinned: Some(true),
                ..MemoryUpdate::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.entry.content, "edited candidate");
    assert!(updated.entry.pinned);
    assert_eq!(fixture.memory.delete_entry(&pending.id).await.unwrap(), 1);

    // A second workspace's entries are invisible.
    let other_workspace = cowbot_core::identity::Workspace::new("Other");
    fixture
        .store
        .create_workspace(&other_workspace)
        .await
        .unwrap();
    let mut foreign = MemoryEntry::new(
        other_workspace.id.clone(),
        None,
        None,
        MemoryKind::Fact,
        "foreign memory",
        cowbot_core::memory::MemorySource::User,
    );
    foreign.recount();
    fixture.store.insert_memory_entry(&foreign).await.unwrap();
    let listed = fixture
        .memory
        .list_entries(&MemoryFilter::default())
        .await
        .unwrap();
    assert!(listed.iter().all(|entry| entry.id != foreign.id));
}

#[tokio::test]
async fn preview_recall_does_not_touch_statistics() {
    let fixture = fixture().await;
    let entry = fixture
        .memory
        .add_entry(
            NewMemory::user("The user prefers Rust.")
                .with_scope(Some(fixture.planner.id.clone()), None),
        )
        .await
        .unwrap()
        .entry;

    let preview = fixture
        .memory
        .preview_recall(None, &fixture.planner.id, "Rust status")
        .await
        .unwrap()
        .expect("the entry is recalled");
    assert!(preview.block.contains("prefers Rust"));
    assert_eq!(preview.entry_ids, vec![entry.id.clone()]);
    let untouched = fixture
        .store
        .get_memory_entry(&entry.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(untouched.recall_count, 0);

    let recalled = fixture
        .memory
        .recall_for_turn(None, &fixture.planner.id, "Rust status")
        .await
        .unwrap()
        .expect("the entry is recalled");
    assert_eq!(recalled.entry_ids, vec![entry.id.clone()]);
    let touched = fixture
        .store
        .get_memory_entry(&entry.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(touched.recall_count, 1);
}

#[tokio::test]
async fn remember_tool_writes_an_agent_owned_entry() {
    let fixture = fixture_with_planner_tools(&["remember"]).await;
    let conversation = fixture
        .conversations
        .create(Some("Memory Lab"), &fixture.planner.id, None, "mock-model")
        .await
        .unwrap();
    let message = fixture
        .channels
        .post_message(
            &conversation.id,
            Sender::User("user-1".to_string()),
            "Remember that I prefer Rust.",
        )
        .await
        .unwrap();
    let llm = Arc::new(FakeLlm::reply("Noted.").with_tool_call(LlmToolCall {
        id: "call-1".to_string(),
        name: "remember".to_string(),
        arguments: serde_json::json!({
            "content": "The user prefers Rust for services.",
            "kind": "preference",
            "pinned": true
        }),
    }));

    fixture
        .conversations
        .reply(
            &conversation.id,
            &message,
            llm.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    let entries = fixture
        .memory
        .list_entries(&MemoryFilter::default())
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.agent_id.as_ref(), Some(&fixture.planner.id));
    assert_eq!(entry.channel_id.as_ref(), Some(&conversation.id));
    assert_eq!(entry.kind, MemoryKind::Preference);
    assert_eq!(entry.source, MemorySource::Agent);
    assert!(entry.pinned);
    assert!(entry.is_active());

    let requests = llm.requests();
    let tool_message = requests
        .last()
        .and_then(|request| {
            request
                .messages
                .iter()
                .find(|message| message.role == ChatRole::Tool)
        })
        .expect("the tool result reached the model");
    assert!(tool_message.content.contains("Saved memory entry"));
}

#[tokio::test]
async fn extraction_stores_pending_candidates_after_a_single_turn() {
    let fixture = fixture().await;
    let conversation = fixture
        .conversations
        .create(Some("Memory Lab"), &fixture.planner.id, None, "mock-model")
        .await
        .unwrap();
    fixture
        .memory
        .update_settings(&MemorySettingsUpdate {
            extraction_enabled: Some(true),
            ..MemorySettingsUpdate::default()
        })
        .await
        .unwrap();
    fixture
        .memory
        .add_entry(NewMemory::user("The user prefers Rust.").with_scope(
            Some(fixture.planner.id.clone()),
            Some(conversation.id.clone()),
        ))
        .await
        .unwrap();

    let message = fixture
        .channels
        .post_message(
            &conversation.id,
            Sender::User("user-1".to_string()),
            "What stack should we use?",
        )
        .await
        .unwrap();
    let extraction = serde_json::json!({
        "memories": [
            {"content": "The user prefers Rust.", "kind": "preference"},
            {"content": "Use PostgreSQL for storage.", "kind": "decision"},
            {"content": "x".repeat(401), "kind": "fact"},
            {"content": "Deploys happen on Fridays.", "kind": "fact"},
            {"content": "Unknown kinds are dropped.", "kind": "rumour"},
            {"content": "API key is sk-live-123456", "kind": "fact"}
        ]
    })
    .to_string();
    let llm = Arc::new(FakeLlm::reply("Use Rust and PostgreSQL.").with_extraction(&extraction));

    fixture
        .conversations
        .reply(
            &conversation.id,
            &message,
            llm.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    let extraction_requests = llm.extraction_requests();
    assert_eq!(extraction_requests.len(), 1);
    let input = &extraction_requests[0].messages[1].content;
    assert!(
        input.contains("Memory Lab"),
        "the conversation name is sent"
    );
    assert!(input.contains("What stack should we use?"));
    assert!(input.contains("Use Rust and PostgreSQL."));

    let pending = fixture.memory.pending_entries().await.unwrap();
    assert_eq!(
        pending.len(),
        3,
        "invalid, duplicate, and oversized candidates are dropped"
    );
    assert!(pending
        .iter()
        .all(|entry| entry.source == MemorySource::Extraction));
    assert!(pending
        .iter()
        .all(|entry| entry.agent_id.as_ref() == Some(&fixture.planner.id)));
    assert!(pending
        .iter()
        .all(|entry| entry.channel_id.as_ref() == Some(&conversation.id)));
    assert!(pending
        .iter()
        .all(|entry| entry.source_ref.as_deref() == Some(message.id.as_str())));
    let contents = pending
        .iter()
        .map(|entry| entry.content.as_str())
        .collect::<Vec<_>>();
    assert!(contents.contains(&"Use PostgreSQL for storage."));
    assert!(contents.contains(&"Deploys happen on Fridays."));
    assert!(
        contents
            .iter()
            .any(|content| content.contains("[redacted]")),
        "credentials in a candidate are redacted"
    );
    assert!(contents.iter().all(|content| !content.contains("sk-live")));

    let calls = fixture
        .store
        .list_llm_calls_for_channel(&conversation.id)
        .await
        .unwrap();
    let extracts = calls
        .iter()
        .filter(|call| call.call_kind == LlmCallKind::MemoryExtract)
        .collect::<Vec<_>>();
    assert_eq!(extracts.len(), 1);
    assert!(extracts[0].run_id.is_none());
    assert!(extracts[0].task_id.is_none());
    assert_eq!(extracts[0].channel_id.as_ref(), Some(&conversation.id));
}

#[tokio::test]
async fn disabled_extraction_makes_no_call() {
    let fixture = fixture().await;
    let conversation = fixture
        .conversations
        .create(Some("Quiet"), &fixture.planner.id, None, "mock-model")
        .await
        .unwrap();
    let message = fixture
        .channels
        .post_message(
            &conversation.id,
            Sender::User("user-1".to_string()),
            "hello",
        )
        .await
        .unwrap();
    let llm = Arc::new(
        FakeLlm::reply("answer")
            .with_extraction("{\"memories\": [{\"content\": \"x\", \"kind\": \"fact\"}]}"),
    );

    fixture
        .conversations
        .reply(
            &conversation.id,
            &message,
            llm.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();

    assert!(llm.extraction_requests().is_empty());
    assert!(fixture.memory.pending_entries().await.unwrap().is_empty());
}

#[tokio::test]
async fn invalid_empty_or_failed_extraction_keeps_the_reply_and_stores_nothing() {
    let fixture = fixture().await;
    fixture
        .memory
        .update_settings(&MemorySettingsUpdate {
            extraction_enabled: Some(true),
            ..MemorySettingsUpdate::default()
        })
        .await
        .unwrap();
    let conversation = fixture
        .conversations
        .create(Some("Noise"), &fixture.planner.id, None, "mock-model")
        .await
        .unwrap();
    let message = fixture
        .channels
        .post_message(
            &conversation.id,
            Sender::User("user-1".to_string()),
            "hello",
        )
        .await
        .unwrap();

    let invalid = Arc::new(FakeLlm::reply("fine").with_extraction("not json at all"));
    let report = fixture
        .conversations
        .reply(
            &conversation.id,
            &message,
            invalid.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();
    assert_eq!(report.agents, vec![fixture.planner.id.clone()]);
    assert_eq!(invalid.extraction_requests().len(), 1);

    let empty = Arc::new(FakeLlm::reply("fine").with_extraction("{\"memories\": []}"));
    fixture
        .conversations
        .reply(
            &conversation.id,
            &message,
            empty.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();
    assert_eq!(empty.extraction_requests().len(), 1);

    let failed = Arc::new(FakeLlm::reply("fine").with_failed_extraction());
    fixture
        .conversations
        .reply(
            &conversation.id,
            &message,
            failed.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();
    assert_eq!(failed.extraction_requests().len(), 1);

    assert!(fixture.memory.pending_entries().await.unwrap().is_empty());
}

#[tokio::test]
async fn group_turn_stores_one_shared_candidate_set() {
    let fixture = fixture().await;
    let channel = fixture
        .conversations
        .create_group(
            Some("Crew"),
            &[fixture.planner.id.clone(), fixture.coder.id.clone()],
            None,
            "mock-model",
            Some(fixture.planner.id.clone()),
        )
        .await
        .unwrap();
    fixture
        .memory
        .update_settings(&MemorySettingsUpdate {
            extraction_enabled: Some(true),
            ..MemorySettingsUpdate::default()
        })
        .await
        .unwrap();

    let message = fixture
        .channels
        .post_message(&channel.id, Sender::User("user-1".to_string()), "status?")
        .await
        .unwrap();
    let llm = Arc::new(FakeLlm::reply("answer").with_extraction(
        "{\"memories\": [{\"content\": \"The team ships on Fridays.\", \"kind\": \"fact\"}]}",
    ));
    let report = fixture
        .conversations
        .reply(
            &channel.id,
            &message,
            llm.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();
    assert_eq!(report.replied, vec![fixture.planner.id.clone()]);
    assert_eq!(llm.extraction_requests().len(), 1);

    let pending = fixture.memory.pending_entries().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(
        pending[0].agent_id.is_none(),
        "a group candidate is conversation-shared"
    );
    assert_eq!(pending[0].channel_id.as_ref(), Some(&channel.id));
    assert_eq!(
        pending[0].source_ref.as_deref(),
        report.turn_id.as_ref().map(|turn| turn.0.as_str())
    );

    let calls = fixture
        .store
        .list_llm_calls_for_channel(&channel.id)
        .await
        .unwrap();
    let extracts = calls
        .iter()
        .filter(|call| call.call_kind == LlmCallKind::MemoryExtract)
        .collect::<Vec<_>>();
    assert_eq!(extracts.len(), 1);
    assert!(extracts[0].agent_id.is_none());
}

#[tokio::test]
async fn silent_turn_makes_no_extraction_call() {
    let fixture = fixture().await;
    let channel = fixture
        .conversations
        .create_group(
            Some("Quiet Crew"),
            &[fixture.planner.id.clone(), fixture.coder.id.clone()],
            None,
            "mock-model",
            None,
        )
        .await
        .unwrap();
    fixture
        .memory
        .update_settings(&MemorySettingsUpdate {
            extraction_enabled: Some(true),
            ..MemorySettingsUpdate::default()
        })
        .await
        .unwrap();

    let message = fixture
        .channels
        .post_message(&channel.id, Sender::User("user-1".to_string()), "hello")
        .await
        .unwrap();
    let llm = Arc::new(
        FakeLlm::reply("unused")
            .with_extraction("{\"memories\": [{\"content\": \"x\", \"kind\": \"fact\"}]}"),
    );
    let report = fixture
        .conversations
        .reply(
            &channel.id,
            &message,
            llm.clone(),
            ToolRegistry::new(),
            &options(),
        )
        .await
        .unwrap();
    assert_eq!(
        report.route,
        Some(cowbot_core::conversation::RouteSource::Silent)
    );
    assert!(
        llm.requests().is_empty(),
        "a silent turn makes no model call at all"
    );
    assert!(fixture.memory.pending_entries().await.unwrap().is_empty());
}

#[tokio::test]
async fn direct_conversation_extracts_after_its_reply() {
    let fixture = fixture().await;
    let direct = fixture
        .channels
        .ensure_agent_channel(&fixture.workspace_id, &fixture.planner)
        .await
        .unwrap();
    fixture
        .memory
        .update_settings(&MemorySettingsUpdate {
            extraction_enabled: Some(true),
            ..MemorySettingsUpdate::default()
        })
        .await
        .unwrap();

    let message = fixture
        .channels
        .post_message(
            &direct.id,
            Sender::User("user-1".to_string()),
            "Do I like Rust?",
        )
        .await
        .unwrap();
    let agents = ChannelAgentService::new(
        fixture.store.clone(),
        fixture.bus.clone(),
        fixture.workspace_id.clone(),
    );
    let llm = Arc::new(FakeLlm::reply("Yes.").with_extraction(
        "{\"memories\": [{\"content\": \"The user likes Rust.\", \"kind\": \"preference\"}]}",
    ));
    agents
        .reply(
            &direct.id,
            &message,
            llm.clone(),
            ToolRegistry::new(),
            "mock-model",
            &options(),
        )
        .await
        .unwrap();

    assert_eq!(llm.extraction_requests().len(), 1);
    let pending = fixture.memory.pending_entries().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].agent_id.as_ref(), Some(&fixture.planner.id));
    assert_eq!(pending[0].channel_id.as_ref(), Some(&direct.id));
    assert_eq!(pending[0].source_ref.as_deref(), Some(message.id.as_str()));
}

#[tokio::test]
async fn extraction_parses_code_fenced_json_and_rejects_bad_shapes() {
    let parsed = cowbot_service::parse_extraction_response(
        "Here you go:\n```json\n{\"memories\":[{\"content\":\" A fact. \",\"kind\":\"fact\"}]}\n```",
    )
    .expect("the fenced object parses");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].content, "A fact.");
    assert_eq!(parsed[0].kind, MemoryKind::Fact);

    assert!(cowbot_service::parse_extraction_response("[]").is_none());
    assert!(cowbot_service::parse_extraction_response("{\"items\": []}").is_none());
    assert_eq!(
        cowbot_service::parse_extraction_response(
            "{\"memories\":[{\"content\":\"\",\"kind\":\"fact\"},{\"content\":\"ok\",\"kind\":\"summary\"}]}"
        ),
        Some(vec![])
    );
}

fn run_options() -> CrewRunOptions {
    CrewRunOptions {
        approval_level: ApprovalLevel::Trusted,
        ..CrewRunOptions::default()
    }
}

async fn run_task(
    fixture: &Fixture,
    task: &Task,
    llm: Arc<RecordingLlm>,
    task_memory: bool,
) -> Vec<ChatCompletionRequest> {
    let mut scheduler = AssignmentScheduler::new(
        fixture.store.clone(),
        fixture.bus.clone(),
        fixture.workspace_id.clone(),
        1,
    );
    if task_memory {
        scheduler = scheduler.with_task_memory();
    }
    scheduler.register_agent(
        fixture.planner.clone(),
        llm.clone(),
        ToolRegistry::new(),
        "mock-model",
    );
    scheduler
        .enqueue(
            task,
            AssignmentRequester::User("user-1".to_string()),
            AssignmentOrigin::User,
            &run_options(),
        )
        .await
        .unwrap();
    scheduler.run_until_idle().await.unwrap();
    llm.requests()
}

#[tokio::test]
async fn task_execution_injects_notes_and_recalled_memory_before_instructions() {
    let fixture = fixture().await;
    fixture
        .memory
        .save_workspace_note("Task note.")
        .await
        .unwrap();
    let entry = fixture
        .memory
        .add_entry(
            NewMemory::user("The deploy target is staging.")
                .with_scope(Some(fixture.planner.id.clone()), None),
        )
        .await
        .unwrap()
        .entry;
    let task = Task::new("Deploy the service to staging", "A short summary")
        .assign_to(fixture.planner.id.clone());

    let requests = run_task(&fixture, &task, Arc::new(RecordingLlm::new()), true).await;
    assert_eq!(requests.len(), 1);
    let messages = &requests[0].messages;
    let memory_index = messages
        .iter()
        .position(|message| message.content.starts_with("Memory (reference data"))
        .expect("the task receives a memory block");
    let task_index = messages
        .iter()
        .position(|message| message.content.starts_with("Task Description:"))
        .expect("the task instructions follow the memory");
    assert!(
        memory_index < task_index,
        "memory is a system message before the task instructions"
    );
    assert_eq!(messages[memory_index].role, ChatRole::System);
    let block = &messages[memory_index].content;
    assert!(block.contains("Task note."));
    assert!(block.contains("deploy target is staging"));

    let reloaded = fixture
        .store
        .get_memory_entry(&entry.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.recall_count, 1);
}

#[tokio::test]
async fn task_memory_uses_the_task_channel_scope() {
    let fixture = fixture().await;
    let conversation = fixture
        .conversations
        .create(Some("Deploy Talk"), &fixture.planner.id, None, "mock-model")
        .await
        .unwrap();
    let other = fixture
        .conversations
        .create(Some("Other Talk"), &fixture.coder.id, None, "mock-model")
        .await
        .unwrap();
    fixture
        .memory
        .save_conversation_note(&conversation.id, "Release carefully.")
        .await
        .unwrap();
    fixture
        .memory
        .save_conversation_note(&other.id, "Other channel note.")
        .await
        .unwrap();
    fixture
        .memory
        .add_entry(
            NewMemory::user("The release branch is cut from dev.")
                .with_scope(None, Some(conversation.id.clone())),
        )
        .await
        .unwrap();
    fixture
        .memory
        .add_entry(
            NewMemory::user("The release branch is cut from main.")
                .with_scope(None, Some(other.id.clone())),
        )
        .await
        .unwrap();

    let task = Task::new("Cut the release branch", "A summary")
        .in_channel(conversation.id.clone())
        .assign_to(fixture.planner.id.clone());
    let requests = run_task(&fixture, &task, Arc::new(RecordingLlm::new()), true).await;
    let block = requests[0]
        .messages
        .iter()
        .find(|message| message.content.starts_with("Memory (reference data"))
        .expect("the task receives a memory block");
    assert!(block.content.contains("Release carefully."));
    assert!(block.content.contains("cut from dev"));
    assert!(!block.content.contains("Other channel note."));
    assert!(!block.content.contains("cut from main"));
}

#[tokio::test]
async fn task_memory_never_crosses_workspaces() {
    let fixture = fixture().await;
    let other_workspace = cowbot_core::identity::Workspace::new("Other");
    fixture
        .store
        .create_workspace(&other_workspace)
        .await
        .unwrap();
    let other_memory = MemoryService::new(fixture.store.clone(), other_workspace.id.clone());
    other_memory
        .save_workspace_note("Other workspace note.")
        .await
        .unwrap();
    other_memory
        .add_entry(NewMemory::user("The deploy target is staging."))
        .await
        .unwrap();

    let task = Task::new("Deploy the service to staging", "A short summary")
        .assign_to(fixture.planner.id.clone());
    let requests = run_task(&fixture, &task, Arc::new(RecordingLlm::new()), true).await;
    assert!(requests[0]
        .messages
        .iter()
        .all(|message| !message.content.starts_with("Memory (reference data")));
}

#[tokio::test]
async fn task_memory_stays_off_for_schedulers_that_do_not_opt_in() {
    let fixture = fixture().await;
    fixture
        .memory
        .save_workspace_note("Should not be injected.")
        .await
        .unwrap();
    let task =
        Task::new("Deploy the service", "A short summary").assign_to(fixture.planner.id.clone());

    let requests = run_task(&fixture, &task, Arc::new(RecordingLlm::new()), false).await;
    assert!(requests[0]
        .messages
        .iter()
        .all(|message| !message.content.starts_with("Memory (reference data")));
}

#[tokio::test]
async fn task_execution_without_memory_is_unchanged() {
    let fixture = fixture().await;
    let task =
        Task::new("Deploy the service", "A short summary").assign_to(fixture.planner.id.clone());
    let llm = Arc::new(RecordingLlm::new());

    let requests = run_task(&fixture, &task, llm, true).await;
    assert_eq!(requests.len(), 1);
    assert!(requests[0]
        .messages
        .iter()
        .all(|message| !message.content.starts_with("Memory (reference data")));
    assert_eq!(
        requests[0].messages.last().unwrap().content,
        "Task Description:\nDeploy the service\n\nExpected Output Format:\nA short summary"
    );
}
