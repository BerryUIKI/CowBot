//! Preview tests: the rebuilt request, its slots, and its recall trace.
//!
//! Every preview must match what the reply path would send and must not write
//! context entries or recall statistics (ADR-0036).

use cowbot_bus::EventBus;
use cowbot_core::agent::AgentProfile;
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::assignment::{AssignmentOrigin, AssignmentRequester};
use cowbot_core::channel::Channel;
use cowbot_core::conversation::{ContextEntry, ContextRole};
use cowbot_core::memory::RecallSkipReason;
use cowbot_core::prompt::{PromptKind, PromptRole, PromptSlotName};
use cowbot_core::run::{Run, RunId};
use cowbot_core::task::{Task, TaskSource};
use cowbot_core::tool::ToolDefinition;
use cowbot_core::usage::{LlmCallKind, LlmCallRecord};
use cowbot_llm::token_counter_for_model;
use cowbot_service::{
    bootstrap_local, AssignmentService, ConversationService, MemoryFilter, MemoryService,
    MemorySettingsUpdate, NewMemory, PreviewService,
};
use cowbot_store::Store;

const MODEL: &str = "gpt-4o";

struct Fixture {
    store: Store,
    workspace_id: cowbot_core::identity::WorkspaceId,
    conversations: ConversationService,
    memory: MemoryService,
    preview: PreviewService,
}

async fn fixture(agents: Vec<AgentProfile>) -> Fixture {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let context = bootstrap_local(&store).await.unwrap();
    for agent in &agents {
        store
            .upsert_agent(&context.workspace.id, agent)
            .await
            .unwrap();
    }
    let bus = EventBus::default_bus();
    Fixture {
        conversations: ConversationService::new(store.clone(), bus, context.workspace.id.clone()),
        memory: MemoryService::new(store.clone(), context.workspace.id.clone()),
        preview: PreviewService::new(store.clone(), context.workspace.id.clone()),
        workspace_id: context.workspace.id,
        store,
    }
}

fn tools() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "read_file".to_string(),
        description: "Read a file from the workspace".to_string(),
        parameters: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    }]
}

async fn append(
    store: &Store,
    channel: &Channel,
    agent: &AgentProfile,
    role: ContextRole,
    content: &str,
) -> ContextEntry {
    let mut entry = ContextEntry::new(channel.id.clone(), agent.id.clone(), role, content);
    entry.recount();
    store.append_context_entry(&entry).await.unwrap();
    entry
}

#[tokio::test]
async fn conversation_preview_rebuilds_the_next_request_without_writing() {
    let planner = AgentProfile::new("Planner", "Architect", "Plan", "Careful");
    let fixture = fixture(vec![planner.clone()]).await;
    let channel = fixture
        .conversations
        .create(
            Some("Design review"),
            &planner.id,
            Some("provider-1".to_string()),
            MODEL,
        )
        .await
        .unwrap();
    append(
        &fixture.store,
        &channel,
        &planner,
        ContextRole::User,
        "What is next?",
    )
    .await;
    append(
        &fixture.store,
        &channel,
        &planner,
        ContextRole::Assistant,
        "Ship the parser.",
    )
    .await;
    fixture
        .memory
        .add_entry(
            NewMemory::user("The user prefers Rust.")
                .with_scope(Some(planner.id.clone()), Some(channel.id.clone()))
                .with_pinned(true),
        )
        .await
        .unwrap();
    let entries_before = fixture
        .store
        .list_context_entries(&channel.id, &planner.id)
        .await
        .unwrap()
        .len();

    let tools = tools();
    let preview = fixture
        .preview
        .conversation(
            &channel.id,
            &planner.id,
            Some("And then?"),
            Some("Projects: cowbot (dev)"),
            false,
            &tools,
        )
        .await
        .unwrap();

    let counter = token_counter_for_model(MODEL);
    assert_eq!(preview.plan.kind, PromptKind::Conversation);
    assert_eq!(preview.plan.model, MODEL);
    assert_eq!(preview.agent_name, "Planner");

    // System prompt, memory block, two branch entries, and the draft.
    let messages = &preview.plan.messages;
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0].role, PromptRole::System);
    assert_eq!(messages[1].role, PromptRole::System);
    assert!(messages[1].content.contains("The user prefers Rust."));
    assert_eq!(messages[2].content, "What is next?");
    assert_eq!(messages[3].content, "Ship the parser.");
    assert_eq!(messages[4].content, "And then?");
    assert!(messages[4].latest);
    assert!(messages[2].entry_id.is_some());
    assert_eq!(messages[4].entry_id, None, "the draft is not persisted");

    // Slot accounting adds up to the counted system prompt.
    let slot_names = preview
        .plan
        .slots
        .iter()
        .map(|slot| slot.name)
        .collect::<Vec<_>>();
    assert_eq!(
        slot_names,
        vec![
            PromptSlotName::Agent,
            PromptSlotName::Platform,
            PromptSlotName::ConversationConvention
        ]
    );
    assert!(preview.plan.slots[1]
        .content
        .contains("Environment: you are running inside CowBot"));
    assert!(
        !preview.plan.slots[1]
            .content
            .contains("Projects: cowbot (dev)"),
        "single conversations ignore the group platform extra"
    );
    let system_content = &messages[0].content;
    assert_eq!(
        preview
            .plan
            .slots
            .iter()
            .map(|slot| slot.tokens)
            .sum::<u32>(),
        counter.count(system_content)
    );
    assert_eq!(messages[0].tokens, counter.count_message(system_content));

    // Projection and per-message accounting use the model counter.
    assert_eq!(
        preview.plan.projection.latest_tokens,
        counter.count_message("And then?")
    );
    assert_eq!(preview.plan.projection.active_entries, 3);
    assert_eq!(messages[4].tokens, counter.count_message("And then?"));
    assert_eq!(
        preview.plan.projection.tool_tokens,
        counter.count(&serde_json::to_string(&tools).unwrap())
    );
    assert_eq!(preview.plan.tools.len(), 1);
    assert_eq!(preview.plan.tools[0].name, "read_file");
    assert_eq!(
        preview.plan.request_tokens(),
        preview.plan.projection.total_tokens() + preview.plan.memory.as_ref().unwrap().tokens
    );
    assert_eq!(
        preview
            .plan
            .messages
            .iter()
            .map(|message| message.tokens)
            .sum::<u32>(),
        preview.plan.request_tokens() - preview.plan.projection.tool_tokens,
        "message costs add up to the request minus the tool schemas"
    );

    // The recall trace selected the pinned entry and explains nothing else.
    assert!(preview.plan.recall.enabled);
    assert_eq!(preview.plan.recall.query, "And then?");
    assert_eq!(preview.plan.recall.candidates.len(), 1);
    assert!(preview.plan.recall.candidates[0].selected);
    assert_eq!(preview.plan.recall.candidates[0].skip_reason, None);

    // The preview wrote nothing: no entry, no recall statistic.
    assert_eq!(
        fixture
            .store
            .list_context_entries(&channel.id, &planner.id)
            .await
            .unwrap()
            .len(),
        entries_before
    );
    let saved = fixture
        .memory
        .list_entries(&MemoryFilter::default())
        .await
        .unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].recall_count, 0);
}

#[tokio::test]
async fn group_preview_names_the_roster_and_default_responder() {
    let planner = AgentProfile::new("Planner", "Architect", "Plan", "Careful");
    let coder = AgentProfile::new("Coder", "Engineer", "Ship", "Careful");
    let fixture = fixture(vec![planner.clone(), coder.clone()]).await;
    let channel = fixture
        .conversations
        .create_group(
            Some("Build it"),
            &[planner.id.clone(), coder.id.clone()],
            Some("provider-1".to_string()),
            MODEL,
            Some(coder.id.clone()),
        )
        .await
        .unwrap();

    let planner_preview = fixture
        .preview
        .conversation(&channel.id, &planner.id, None, None, false, &[])
        .await
        .unwrap();
    let coder_preview = fixture
        .preview
        .conversation(&channel.id, &coder.id, None, None, false, &[])
        .await
        .unwrap();

    assert_eq!(planner_preview.plan.kind, PromptKind::Group);
    assert_eq!(coder_preview.plan.kind, PromptKind::Group);
    for preview in [&planner_preview, &coder_preview] {
        let names = preview
            .plan
            .slots
            .iter()
            .map(|slot| slot.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                PromptSlotName::Agent,
                PromptSlotName::Platform,
                PromptSlotName::Roster,
                PromptSlotName::DefaultResponder,
                PromptSlotName::PeerConvention
            ]
        );
    }
    let roster = &planner_preview.plan.slots[2].content;
    assert!(roster.contains("- Planner"));
    assert!(roster.contains("- Coder"));
    assert!(planner_preview.plan.slots[3]
        .content
        .contains("answered by Coder"));
    assert!(planner_preview.plan.messages[0]
        .content
        .contains("You are Planner"));
    assert!(coder_preview.plan.messages[0]
        .content
        .contains("You are Coder"));
    assert!(
        planner_preview.plan.messages.len() == 1,
        "no branch, no memory, and no draft means only the system prompt"
    );
}

#[tokio::test]
async fn plan_mode_conversation_preview_adds_the_instruction() {
    let planner = AgentProfile::new("Planner", "Architect", "Plan", "Careful");
    let fixture = fixture(vec![planner.clone()]).await;
    let channel = fixture
        .conversations
        .create(None, &planner.id, None, MODEL)
        .await
        .unwrap();

    let preview = fixture
        .preview
        .conversation(&channel.id, &planner.id, None, None, true, &[])
        .await
        .unwrap();

    assert_eq!(
        preview.plan.slots.last().unwrap().name,
        PromptSlotName::ModeInstruction
    );
    assert_eq!(preview.plan.messages.len(), 2);
    assert_eq!(preview.plan.messages[1].role, PromptRole::System);
    assert!(preview.plan.messages[1]
        .content
        .starts_with("You are in Plan mode."));
    assert_eq!(
        preview.plan.projection.system_tokens,
        preview.plan.messages[0].tokens + preview.plan.messages[1].tokens
    );
}

#[tokio::test]
async fn preview_traces_skipped_recall_candidates_without_touching_statistics() {
    let planner = AgentProfile::new("Planner", "Architect", "Plan", "Careful");
    let fixture = fixture(vec![planner.clone()]).await;
    let channel = fixture
        .conversations
        .create(None, &planner.id, None, MODEL)
        .await
        .unwrap();
    fixture
        .memory
        .update_settings(&MemorySettingsUpdate {
            extraction_enabled: None,
            recall_enabled: None,
            notes_budget_tokens: None,
            recall_budget_tokens: Some(100),
        })
        .await
        .unwrap();
    let large = fixture
        .memory
        .add_entry(
            NewMemory::user("x".repeat(600))
                .with_scope(Some(planner.id.clone()), Some(channel.id.clone()))
                .with_pinned(true),
        )
        .await
        .unwrap();
    let small = fixture
        .memory
        .add_entry(
            NewMemory::user("Prefers Rust and short answers.")
                .with_scope(Some(planner.id.clone()), Some(channel.id.clone())),
        )
        .await
        .unwrap();

    let preview = fixture
        .preview
        .conversation(
            &channel.id,
            &planner.id,
            Some("Rust status?"),
            None,
            false,
            &[],
        )
        .await
        .unwrap();

    assert_eq!(preview.plan.recall.query, "Rust status?");
    let candidates = &preview.plan.recall.candidates;
    let large_candidate = candidates
        .iter()
        .find(|candidate| candidate.entry_id == large.entry.id)
        .expect("the pinned entry is always considered");
    assert_eq!(
        large_candidate.skip_reason,
        Some(RecallSkipReason::OverBudget {
            remaining_tokens: 100
        })
    );
    assert!(!large_candidate.selected);
    let small_candidate = candidates
        .iter()
        .find(|candidate| candidate.entry_id == small.entry.id)
        .expect("the search hit is considered");
    assert!(small_candidate.selected);
    assert!(small_candidate.score.is_some());

    // A second preview is identical and still writes nothing.
    let again = fixture
        .preview
        .conversation(
            &channel.id,
            &planner.id,
            Some("Rust status?"),
            None,
            false,
            &[],
        )
        .await
        .unwrap();
    assert_eq!(preview.plan.recall, again.plan.recall);
    let saved = fixture
        .memory
        .list_entries(&MemoryFilter::default())
        .await
        .unwrap();
    assert!(saved.iter().all(|entry| entry.recall_count == 0));
    assert!(saved.iter().all(|entry| entry.last_recalled_at.is_none()));
}

#[tokio::test]
async fn task_run_preview_rebuilds_the_first_request_and_plan_mode() {
    let coder = AgentProfile::new("Coder", "Engineer", "Ship", "Careful");
    let fixture = fixture(vec![coder.clone()]).await;
    let assignments = AssignmentService::new(fixture.store.clone(), EventBus::default_bus());

    let task = Task::new("Write the parser", "A working parser").assign_to(coder.id.clone());
    fixture
        .store
        .create_task(&fixture.workspace_id, None, &task)
        .await
        .unwrap();
    let assignment = assignments
        .enqueue_root(
            &task,
            AssignmentOrigin::User,
            AssignmentRequester::User("user-1".to_string()),
            coder.id.clone(),
            ApprovalLevel::Ask,
        )
        .await
        .unwrap();
    assignments.start(&assignment.id).await.unwrap();
    let run = Run::new(task.id.clone(), coder.id.clone());
    fixture.store.create_run(&run).await.unwrap();

    let preview = fixture
        .preview
        .task_run(&run.id, MODEL, &tools())
        .await
        .unwrap()
        .expect("the run exists");

    assert_eq!(preview.plan.kind, PromptKind::TaskRun);
    assert_eq!(preview.plan.model, MODEL);
    assert_eq!(preview.agent_name, "Coder");
    assert_eq!(preview.task_title, "Write the parser");
    assert_eq!(
        preview
            .plan
            .slots
            .iter()
            .map(|slot| slot.name)
            .collect::<Vec<_>>(),
        vec![PromptSlotName::Agent]
    );
    let last = preview.plan.messages.last().unwrap();
    assert_eq!(last.role, PromptRole::User);
    assert_eq!(
        last.content,
        "Task Description:\nWrite the parser\n\nExpected Output Format:\nA working parser"
    );
    let counter = token_counter_for_model(MODEL);
    assert_eq!(last.tokens, counter.count_message(&last.content));
    assert_eq!(preview.plan.projection.history_tokens, last.tokens);
    assert_eq!(
        preview.plan.projection.tool_tokens,
        counter.count(&serde_json::to_string(&tools()).unwrap())
    );

    // A Plan-mode assignment adds the instruction slot before the task prompt.
    let plan_task = Task::new("Plan the parser", "A plan").assign_to(coder.id.clone());
    fixture
        .store
        .create_task(&fixture.workspace_id, None, &plan_task)
        .await
        .unwrap();
    let plan_assignment = assignments
        .enqueue_root(
            &plan_task,
            AssignmentOrigin::User,
            AssignmentRequester::User("user-1".to_string()),
            coder.id.clone(),
            ApprovalLevel::PlanOnly,
        )
        .await
        .unwrap();
    assignments.start(&plan_assignment.id).await.unwrap();
    let plan_run = Run::new(plan_task.id.clone(), coder.id.clone());
    fixture.store.create_run(&plan_run).await.unwrap();

    let plan_preview = fixture
        .preview
        .task_run(&plan_run.id, MODEL, &[])
        .await
        .unwrap()
        .expect("the run exists");
    assert_eq!(
        plan_preview
            .plan
            .slots
            .iter()
            .map(|slot| slot.name)
            .collect::<Vec<_>>(),
        vec![PromptSlotName::Agent, PromptSlotName::ModeInstruction]
    );
    assert_eq!(plan_preview.plan.messages[1].role, PromptRole::System);
    assert!(plan_preview.plan.messages[1]
        .content
        .starts_with("You are in Plan mode."));

    // A missing run is reported as absent rather than as an error.
    assert!(fixture
        .preview
        .task_run(&RunId::new("missing"), MODEL, &[])
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn task_run_preview_uses_the_recorded_agent_profile_and_model() {
    // Standard crew agents are transient, so the run snapshot must supply the
    // profile; the recorded LLM call supplies the model the run actually used.
    let fixture = fixture(vec![]).await;
    let assignments = AssignmentService::new(fixture.store.clone(), EventBus::default_bus());
    let transient = AgentProfile::new("Cow-Coder", "Engineer", "Ship", "Careful");
    let task = Task::new("Implement it", "A diff").assign_to(transient.id.clone());
    fixture
        .store
        .create_task(&fixture.workspace_id, None, &task)
        .await
        .unwrap();
    let assignment = assignments
        .enqueue_root(
            &task,
            AssignmentOrigin::User,
            AssignmentRequester::User("user-1".to_string()),
            transient.id.clone(),
            ApprovalLevel::Ask,
        )
        .await
        .unwrap();
    assignments.start(&assignment.id).await.unwrap();
    let run = Run::new(task.id.clone(), transient.id.clone()).with_agent_profile(transient.clone());
    fixture.store.create_run(&run).await.unwrap();
    let mut call = LlmCallRecord::new(LlmCallKind::Agent, "gpt-4");
    call.run_id = Some(run.id.clone());
    call.task_id = Some(task.id.clone());
    call.agent_id = Some(transient.id.clone());
    fixture.store.record_llm_call(&call).await.unwrap();

    let preview = fixture
        .preview
        .task_run(&run.id, MODEL, &[])
        .await
        .unwrap()
        .expect("the run exists");
    assert_eq!(preview.agent_name, "Cow-Coder");
    assert!(preview.plan.messages[0]
        .content
        .starts_with("You are Cow-Coder."));
    assert_eq!(preview.plan.model, "gpt-4");
    assert_eq!(
        preview.plan.projection.token_counter.as_deref(),
        Some("cl100k_base")
    );
}

#[tokio::test]
async fn task_run_preview_refuses_conversation_turn_runs() {
    let coder = AgentProfile::new("Coder", "Engineer", "Ship", "Careful");
    let fixture = fixture(vec![coder.clone()]).await;
    let task = Task::new("Reply in the conversation", "An answer")
        .assign_to(coder.id.clone())
        .from_source(TaskSource::ConversationTurn);
    fixture
        .store
        .create_task(&fixture.workspace_id, None, &task)
        .await
        .unwrap();
    let run = Run::new(task.id.clone(), coder.id.clone());
    fixture.store.create_run(&run).await.unwrap();

    let error = fixture
        .preview
        .task_run(&run.id, MODEL, &[])
        .await
        .expect_err("turn runs have their own preview");
    assert!(error.to_string().contains("conversation turn"));
}
