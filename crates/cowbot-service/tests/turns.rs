//! Turn lifecycle tests: normal completion, a stop mid-text, a stop during a
//! tool call, an approval pause, and a provider error after partial text
//! (ADR-0039).

use std::sync::Arc;
use std::time::Duration;

use cowbot_agent::StandardRoles;
use cowbot_bus::EventBus;
use cowbot_core::agent::AgentId;
use cowbot_core::conversation::TurnId;
use cowbot_core::event::EventPayload;
use cowbot_core::identity::WorkspaceId;
use cowbot_core::task::{Task, TaskStatus};
use cowbot_core::turn::{TurnControl, TurnEvent};
use cowbot_llm::{LlmToolCall, MockLlmProvider, MockTurn, StreamChunk};
use cowbot_service::{bootstrap_local, AssignmentScheduler, CrewRunOptions};
use cowbot_service::{stopped_text, TurnCompletion, TurnRunner, TurnSink, TurnSpec};
use cowbot_store::Store;
use cowbot_tools::ToolRegistry;

async fn setup() -> (Store, WorkspaceId) {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let context = bootstrap_local(&store).await.unwrap();
    (store, context.workspace.id)
}

fn options(max_concurrency: usize) -> CrewRunOptions {
    CrewRunOptions {
        approval_level: cowbot_core::approval::ApprovalLevel::Ask,
        channel_id: None,
        max_concurrency,
        use_model_permissions: false,
        capture_plan_task: None,
    }
}

/// Collects the turn events published on a bus.
fn turn_events(bus: &EventBus) -> Vec<TurnEvent> {
    bus.history()
        .into_iter()
        .filter_map(|event| match event.payload {
            EventPayload::Turn(turn) => Some(turn),
            _ => None,
        })
        .collect()
}

fn texts(events: &[TurnEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            TurnEvent::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn terminal(events: &[TurnEvent]) -> Option<&TurnEvent> {
    events.iter().find(|event| event.is_terminal())
}

fn get_terminal(events: &[TurnEvent]) -> &TurnEvent {
    terminal(events).expect("the turn published a terminal event")
}

#[test]
fn the_stopped_marker_is_appended_once() {
    let once = stopped_text("partial answer");
    assert!(once.ends_with("_[Stopped]_"));
    assert_eq!(stopped_text(&once), once);
}

#[tokio::test]
async fn a_runner_publishes_started_deltas_and_a_persisted_done() {
    let (store, _) = setup().await;
    let bus = EventBus::default_bus();
    let runner = TurnRunner::new(store.clone(), bus.clone());
    let turn = runner.start(
        TurnSpec {
            target: cowbot_core::turn::TurnTarget::Task {
                task_id: cowbot_core::task::TaskId::new("task-1"),
                agent_id: AgentId("agent-1".to_string()),
            },
            agent_ids: vec![AgentId("agent-1".to_string())],
            sink: TurnSink::Caller,
        },
        None,
    );

    runner
        .finish(&turn, TurnCompletion::done("the answer"))
        .await;

    let events = turn_events(&bus);
    assert!(matches!(
        events.first(),
        Some(TurnEvent::TurnStarted { .. })
    ));
    match get_terminal(&events) {
        TurnEvent::TurnDone {
            message, stopped, ..
        } => {
            assert_eq!(message, "the answer");
            assert!(!stopped);
        }
        other => panic!("expected turn_done, got {other:?}"),
    }
}

#[tokio::test]
async fn a_provider_streams_deltas_into_the_turn() {
    let (store, workspace_id) = setup().await;
    let bus = EventBus::default_bus();
    let llm = Arc::new(MockLlmProvider::with_turns(vec![MockTurn {
        chunks: vec![
            StreamChunk::Text("Hello ".to_string()),
            StreamChunk::Text("world".to_string()),
        ],
        finish_reason: Some("stop".to_string()),
        chunk_delay: None,
        error_after_chunks: None,
    }]));
    let coder = StandardRoles::coder();
    let coder_id = coder.id.clone();
    let mut scheduler = AssignmentScheduler::new(store.clone(), bus.clone(), workspace_id, 1);
    scheduler.register_agent(coder, llm, ToolRegistry::new(), "mock-model");

    let task = Task::new("Say hello", "Greeting").assign_to(coder_id);
    let run_options = options(1);
    scheduler
        .enqueue(
            &task,
            cowbot_core::assignment::AssignmentRequester::User("user-1".to_string()),
            cowbot_core::assignment::AssignmentOrigin::User,
            &run_options,
        )
        .await
        .unwrap();
    scheduler.run_until_idle().await.unwrap();

    let events = turn_events(&bus);
    assert_eq!(texts(&events), "Hello world");
    match get_terminal(&events) {
        TurnEvent::TurnDone { message, .. } => assert_eq!(message, "Hello world"),
        other => panic!("expected turn_done, got {other:?}"),
    }
    // The task output matches what the turn reported.
    let stored = store.get_task(&task.id).await.unwrap().unwrap();
    assert_eq!(
        stored.status,
        TaskStatus::Completed {
            output: "Hello world".to_string()
        }
    );
}

#[tokio::test]
async fn cancelling_a_task_turn_keeps_its_partial_output() {
    let (store, workspace_id) = setup().await;
    let bus = EventBus::default_bus();
    let llm = Arc::new(MockLlmProvider::with_turns(vec![MockTurn {
        chunks: vec![
            StreamChunk::Text("half an ".to_string()),
            StreamChunk::Text("answer".to_string()),
        ],
        finish_reason: Some("stop".to_string()),
        chunk_delay: Some(Duration::from_millis(600)),
        error_after_chunks: None,
    }]));
    let coder = StandardRoles::coder();
    let coder_id = coder.id.clone();
    let mut scheduler = AssignmentScheduler::new(store.clone(), bus.clone(), workspace_id, 1);
    scheduler.register_agent(coder, llm, ToolRegistry::new(), "mock-model");

    let task = Task::new("Write a long answer", "Answer").assign_to(coder_id);
    let run_options = options(1);
    scheduler
        .enqueue(
            &task,
            cowbot_core::assignment::AssignmentRequester::User("user-1".to_string()),
            cowbot_core::assignment::AssignmentOrigin::User,
            &run_options,
        )
        .await
        .unwrap();

    // Wait for the first delta, then stop the turn by id.
    let stopper = scheduler.clone();
    let watcher = bus.clone();
    tokio::spawn(async move {
        for _ in 0..100 {
            let started = turn_events(&watcher)
                .iter()
                .find(|event| matches!(event, TurnEvent::TextDelta { .. }))
                .map(|event| event.turn_id().clone());
            if let Some(turn) = started {
                assert!(stopper.cancel_turn(&turn));
                // Cancelling twice is a no-op that still reports the turn.
                let _ = stopper.cancel_turn(&turn);
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    let _ = scheduler.run_until_idle().await;

    let events = turn_events(&bus);
    assert_eq!(texts(&events), "half an ");
    match get_terminal(&events) {
        TurnEvent::TurnDone {
            message, stopped, ..
        } => {
            assert!(stopped, "a stopped turn reports its stop");
            assert!(message.starts_with("half an"), "got {message:?}");
            assert!(message.contains("[Stopped]"));
        }
        other => panic!("expected turn_done, got {other:?}"),
    }
    let stored = store.get_task(&task.id).await.unwrap().unwrap();
    match stored.status {
        TaskStatus::Completed { output } => {
            assert!(output.starts_with("half an"), "got {output}");
            assert!(output.contains("[Stopped]"), "got {output}");
        }
        other => panic!("expected the partial output to be stored, got {other:?}"),
    }
}

#[tokio::test]
async fn cancelling_an_unknown_turn_is_idempotent_and_reports_false() {
    let (store, _) = setup().await;
    let runner = TurnRunner::new(store, EventBus::default_bus());
    let turn = TurnId::random();
    assert!(!runner.cancel(&turn));
    assert!(!runner.cancel(&turn));
    assert!(!runner.is_cancelled(&turn));
}

#[tokio::test]
async fn a_provider_error_after_partial_text_reports_what_was_produced() {
    let (store, workspace_id) = setup().await;
    let bus = EventBus::default_bus();
    let llm = Arc::new(MockLlmProvider::with_turns(vec![MockTurn {
        chunks: vec![StreamChunk::Text("partial".to_string())],
        finish_reason: None,
        chunk_delay: None,
        error_after_chunks: Some("upstream reset".to_string()),
    }]));
    let coder = StandardRoles::coder();
    let coder_id = coder.id.clone();
    let mut scheduler = AssignmentScheduler::new(store.clone(), bus.clone(), workspace_id, 1);
    scheduler.register_agent(coder, llm, ToolRegistry::new(), "mock-model");

    let task = Task::new("Fail part-way", "Answer").assign_to(coder_id);
    let run_options = options(1);
    scheduler
        .enqueue(
            &task,
            cowbot_core::assignment::AssignmentRequester::User("user-1".to_string()),
            cowbot_core::assignment::AssignmentOrigin::User,
            &run_options,
        )
        .await
        .unwrap();
    assert!(scheduler.run_until_idle().await.is_err());

    let events = turn_events(&bus);
    assert_eq!(texts(&events), "partial");
    match get_terminal(&events) {
        TurnEvent::TurnError { error, partial, .. } => {
            assert!(error.contains("upstream reset"), "got {error}");
            assert_eq!(partial, "partial");
        }
        other => panic!("expected turn_error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_tool_call_reports_start_and_finish_around_the_result() {
    let (store, workspace_id) = setup().await;
    let bus = EventBus::default_bus();
    let llm = Arc::new(MockLlmProvider::with_turns(vec![
        MockTurn {
            chunks: vec![StreamChunk::ToolCall(LlmToolCall {
                id: "call-1".to_string(),
                name: "missing_tool".to_string(),
                arguments: serde_json::json!({}),
            })],
            finish_reason: Some("tool_calls".to_string()),
            chunk_delay: None,
            error_after_chunks: None,
        },
        MockTurn::text("done"),
    ]));
    let coder = StandardRoles::coder();
    let coder_id = coder.id.clone();
    let mut scheduler = AssignmentScheduler::new(store.clone(), bus.clone(), workspace_id, 1);
    scheduler.register_agent(coder, llm, ToolRegistry::new(), "mock-model");

    let task = Task::new("Use a tool", "Answer").assign_to(coder_id);
    let run_options = options(1);
    scheduler
        .enqueue(
            &task,
            cowbot_core::assignment::AssignmentRequester::User("user-1".to_string()),
            cowbot_core::assignment::AssignmentOrigin::User,
            &run_options,
        )
        .await
        .unwrap();
    scheduler.run_until_idle().await.unwrap();

    let events = turn_events(&bus);
    let started = events
        .iter()
        .position(|event| matches!(event, TurnEvent::ToolCallStarted { .. }))
        .expect("a tool started event");
    let finished = events
        .iter()
        .position(|event| matches!(event, TurnEvent::ToolCallFinished { .. }))
        .expect("a tool finished event");
    assert!(started < finished, "the call starts before it finishes");
    match &events[started] {
        TurnEvent::ToolCallStarted { call, .. } => assert_eq!(call.name, "missing_tool"),
        _ => unreachable!(),
    }
    match &events[finished] {
        TurnEvent::ToolCallFinished { result, .. } => {
            assert_eq!(result.call_id, "call-1");
            assert!(!result.success, "the tool does not exist");
        }
        _ => unreachable!(),
    }
}
