use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cowbot_bus::EventBus;
use cowbot_core::agent::AgentProfile;
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::channel::Sender;
use cowbot_core::error::CowResult;
use cowbot_core::identity::WorkspaceId;
use cowbot_core::memory::{MemoryEntry, MemoryKind, MemoryStatus};
use cowbot_core::usage::TokenUsage;
use cowbot_llm::{ChatCompletionRequest, ChatCompletionResponse, LlmProvider};
use cowbot_service::{
    bootstrap_local, ChannelAgentService, ChannelService, ChannelTurnOptions, ConversationService,
    MemoryFilter, MemoryService, MemorySettingsUpdate, MemoryUpdate, NewMemory,
};
use cowbot_store::Store;
use cowbot_tools::ToolRegistry;

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
