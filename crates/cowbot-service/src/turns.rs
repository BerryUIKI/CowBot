//! One runner for every turn: direct chat, group reply, channel mention, task.
//!
//! A turn is the unit a user starts and can stop. Every path registers its
//! [`TurnId`] with the same [`TurnRunner`], which publishes the lifecycle
//! events, stores the final message before the terminal event, and turns a stop
//! request into a partial reply with a stopped marker. The chat, group, and task
//! paths call into it; none of them reimplements stopping or persistence
//! (ADR-0039).

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use cowbot_agent::TurnObserver;
use cowbot_bus::EventBus;
use cowbot_core::agent::AgentId;
use cowbot_core::approval::Approval;
use cowbot_core::cancel::CancelSignal;
use cowbot_core::channel::{ChannelId, Sender};
use cowbot_core::conversation::{ContextEntry, ContextRole, TurnId};
use cowbot_core::error::CowResult;
use cowbot_core::event::{Event, EventPayload};
use cowbot_core::task::TaskId;
use cowbot_core::tool::{ToolCall, ToolResult};
use cowbot_core::turn::{PendingApproval, TurnControl, TurnEvent, TurnOutcome, TurnTarget};
use cowbot_store::Store;

use crate::channels::ChannelService;

/// Marker appended to a reply the user stopped, so a reload shows why the
/// answer ends abruptly.
pub const STOPPED_MARKER: &str = "_[Stopped]_";

/// Apply the stopped marker to the text a turn stores.
///
/// Every path that persists a stopped turn uses this, so the marker reads the
/// same wherever the reply is shown.
pub fn stopped_text(text: &str) -> String {
    let trimmed = text.trim_end();
    if trimmed.ends_with(STOPPED_MARKER) {
        return text.to_string();
    }
    if trimmed.is_empty() {
        STOPPED_MARKER.to_string()
    } else {
        format!("{trimmed}\n\n{STOPPED_MARKER}")
    }
}

/// Where a turn's final reply is stored.
#[derive(Debug, Clone)]
pub enum TurnSink {
    /// A conversation reply: the text becomes a message and, when the turn is
    /// a user-created conversation or a task is not involved, an assistant
    /// entry in the participant's context branch.
    Conversation {
        channel_id: ChannelId,
        agent_id: AgentId,
        /// Name shown in a notice when the reply fails or is empty.
        agent_name: String,
        /// Write the reply into the participant's context branch.
        record_context: bool,
    },
    /// The caller stores the output itself (task runs go through the
    /// assignment service); the runner only publishes the terminal event.
    Caller,
}

/// What a runner needs to open a turn.
#[derive(Debug, Clone)]
pub struct TurnSpec {
    pub target: TurnTarget,
    /// Every agent that answers in this turn.
    pub agent_ids: Vec<AgentId>,
    pub sink: TurnSink,
}

/// How a turn ended, as the runner stores and reports it.
#[derive(Debug, Clone, Default)]
pub struct TurnCompletion {
    /// Text the turn produced. For a failure this is the partial reply.
    pub text: String,
    /// The user stopped the turn.
    pub stopped: bool,
    /// The turn failed; the text above is what it managed to produce.
    pub error: Option<String>,
    /// Append the reply to the participant's context branch.
    ///
    /// A completed conversation turn leaves this off: the scheduler appends the
    /// whole recorded loop, including the final answer. A stopped or failed turn
    /// sets it, because the loop's messages are discarded and only the partial
    /// text survives.
    pub record_context: bool,
}

impl TurnCompletion {
    /// The agent finished on its own.
    pub fn done(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            stopped: false,
            error: None,
            record_context: false,
        }
    }

    /// The user stopped the turn.
    pub fn stopped(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            stopped: true,
            error: None,
            record_context: true,
        }
    }

    /// The turn failed, possibly after producing some text.
    pub fn failed(error: impl Into<String>, partial: impl Into<String>) -> Self {
        Self {
            text: partial.into(),
            stopped: false,
            error: Some(error.into()),
            record_context: true,
        }
    }

    /// Append this reply to the participant's context branch when it is stored.
    pub fn recording_context(mut self, record: bool) -> Self {
        self.record_context = record;
        self
    }
}

/// An open turn: its cancellation scope and what to do when it finishes.
struct OpenTurn {
    spec: TurnSpec,
    cancel: CancelSignal,
    channel_id: Option<ChannelId>,
}

/// Upper bound on retained turn state, so a long-lived process cannot grow
/// without bound if a caller forgets [`TurnControl::close`].
const MAX_OPEN_TURNS: usize = 512;

/// Registers turns, publishes their events, and stores their outcome.
#[derive(Clone)]
pub struct TurnRunner {
    store: Store,
    bus: EventBus,
    channels: ChannelService,
    open: Arc<StdMutex<HashMap<TurnId, OpenTurn>>>,
}

impl TurnRunner {
    pub fn new(store: Store, bus: EventBus) -> Self {
        Self {
            channels: ChannelService::new(store.clone(), bus.clone()),
            store,
            bus,
            open: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Open a turn with a fresh id and publish `turn_started`.
    pub fn start(&self, spec: TurnSpec, channel_id: Option<ChannelId>) -> TurnId {
        let turn = TurnId::random();
        if let Ok(mut open) = self.open.lock() {
            if open.len() >= MAX_OPEN_TURNS {
                if let Some(victim) = open.keys().next().cloned() {
                    open.remove(&victim);
                }
            }
            open.insert(
                turn.clone(),
                OpenTurn {
                    cancel: CancelSignal::new(),
                    channel_id,
                    spec: spec.clone(),
                },
            );
        }
        self.publish(TurnEvent::TurnStarted {
            turn_id: turn.clone(),
            target: spec.target,
            agent_ids: spec.agent_ids,
        });
        turn
    }

    /// The observer an agent reports deltas and tool progress to.
    pub fn observer(&self, turn: &TurnId) -> Arc<dyn TurnObserver> {
        Arc::new(RunnerObserver {
            runner: self.clone(),
            turn: turn.clone(),
        })
    }

    fn publish(&self, event: TurnEvent) {
        self.bus.publish(Event::new(EventPayload::Turn(event)));
    }

    /// Publish an approval pause for this turn.
    ///
    /// A pause is a normal turn state, not an error: the same turn resumes once
    /// the decision lands, so no terminal event is published here.
    pub fn approval_required(&self, turn: &TurnId, approval: Approval, call: ToolCall) {
        self.publish(TurnEvent::ApprovalRequired {
            turn_id: turn.clone(),
            approval: PendingApproval { approval, call },
        });
    }

    /// Finish a turn: store the reply, then publish the terminal event.
    ///
    /// A stopped turn keeps the text it produced and stores it with the stopped
    /// marker, so a reload shows what the user saw rather than a spinner.
    pub async fn finish(&self, turn: &TurnId, completion: TurnCompletion) -> TurnOutcome {
        let Some(open) = self.open.lock().ok().and_then(|mut open| open.remove(turn)) else {
            // A turn that was never opened, or already finished, still reports
            // its outcome instead of failing on a duplicate finish.
            return completion.into_outcome();
        };

        let mut text = completion.text;
        if completion.stopped {
            text = stopped_text(&text);
        }

        let stored = match &open.spec.sink {
            TurnSink::Conversation {
                channel_id,
                agent_id,
                agent_name,
                record_context,
            } => {
                self.store_conversation(
                    channel_id,
                    agent_id,
                    agent_name,
                    &text,
                    *record_context && completion.record_context,
                    completion.error.as_deref(),
                )
                .await
            }
            TurnSink::Caller => Ok(()),
        };

        let outcome = match (completion.error, stored) {
            (Some(error), _) => TurnOutcome::Failed {
                error,
                partial: text,
            },
            (None, Ok(())) => TurnOutcome::Done {
                message: text,
                stopped: completion.stopped,
            },
            (None, Err(error)) => TurnOutcome::Failed {
                error: error.to_string(),
                partial: text,
            },
        };

        match &outcome {
            TurnOutcome::Done { message, stopped } => self.publish(TurnEvent::TurnDone {
                turn_id: turn.clone(),
                message: message.clone(),
                stopped: *stopped,
            }),
            TurnOutcome::Failed { error, partial } => self.publish(TurnEvent::TurnError {
                turn_id: turn.clone(),
                error: error.clone(),
                partial: partial.clone(),
            }),
        }
        outcome
    }

    /// Store a conversation reply and report whether it was written.
    #[allow(clippy::too_many_arguments)]
    async fn store_conversation(
        &self,
        channel_id: &ChannelId,
        agent_id: &AgentId,
        agent_name: &str,
        text: &str,
        record_context: bool,
        error: Option<&str>,
    ) -> CowResult<()> {
        let content = text.trim();
        if let Some(error) = error {
            self.channels
                .post_system_notice(
                    channel_id,
                    &format!("{agent_name} could not answer: {error}"),
                )
                .await?;
            return Ok(());
        }
        if content.is_empty() {
            self.channels
                .post_system_notice(
                    channel_id,
                    &format!("{agent_name} finished without a reply."),
                )
                .await?;
            return Ok(());
        }

        self.channels
            .post_message(channel_id, Sender::Agent(agent_id.clone()), content)
            .await?;
        if record_context {
            let mut entry = ContextEntry::new(
                channel_id.clone(),
                agent_id.clone(),
                ContextRole::Assistant,
                content,
            );
            entry.recount();
            self.store.append_context_entry(&entry).await?;
        }
        Ok(())
    }
}

impl TurnCompletion {
    fn into_outcome(self) -> TurnOutcome {
        match self.error {
            Some(error) => TurnOutcome::Failed {
                error,
                partial: self.text,
            },
            None => TurnOutcome::Done {
                message: self.text,
                stopped: self.stopped,
            },
        }
    }
}

impl TurnControl for TurnRunner {
    fn open(&self, turn: TurnId, channel_id: Option<ChannelId>) {
        // A turn is normally opened by [`TurnRunner::start`]; this keeps the
        // trait contract for callers that already own the id.
        if let Ok(mut open) = self.open.lock() {
            open.entry(turn).or_insert_with(|| OpenTurn {
                spec: TurnSpec {
                    target: TurnTarget::Task {
                        task_id: TaskId::new(String::new()),
                        agent_id: AgentId(String::new()),
                    },
                    agent_ids: Vec::new(),
                    sink: TurnSink::Caller,
                },
                cancel: CancelSignal::new(),
                channel_id,
            });
        }
    }

    fn cancel(&self, turn: &TurnId) -> bool {
        let signal = self
            .open
            .lock()
            .ok()
            .and_then(|open| open.get(turn).map(|entry| entry.cancel.clone()));
        match signal {
            Some(signal) => {
                signal.cancel();
                true
            }
            None => false,
        }
    }

    fn cancel_channel(&self, channel_id: &ChannelId) -> usize {
        let signals: Vec<CancelSignal> = self
            .open
            .lock()
            .map(|open| {
                open.values()
                    .filter(|entry| entry.channel_id.as_ref() == Some(channel_id))
                    .map(|entry| entry.cancel.clone())
                    .collect()
            })
            .unwrap_or_default();
        for signal in &signals {
            signal.cancel();
        }
        signals.len()
    }

    fn turn(&self, turn: &TurnId) -> CancelSignal {
        self.open
            .lock()
            .ok()
            .and_then(|open| open.get(turn).map(|entry| entry.cancel.clone()))
            .unwrap_or_default()
    }

    fn close(&self, turn: &TurnId) {
        if let Ok(mut open) = self.open.lock() {
            open.remove(turn);
        }
    }

    fn is_cancelled(&self, turn: &TurnId) -> bool {
        self.open
            .lock()
            .ok()
            .and_then(|open| open.get(turn).map(|entry| entry.cancel.is_cancelled()))
            .unwrap_or(false)
    }
}

/// Publishes the agent loop's progress as turn events.
struct RunnerObserver {
    runner: TurnRunner,
    turn: TurnId,
}

impl TurnObserver for RunnerObserver {
    fn reasoning_delta(&self, text: &str) {
        self.runner.publish(TurnEvent::ReasoningDelta {
            turn_id: self.turn.clone(),
            text: text.to_string(),
        });
    }

    fn text_delta(&self, text: &str) {
        self.runner.publish(TurnEvent::TextDelta {
            turn_id: self.turn.clone(),
            text: text.to_string(),
        });
    }

    fn tool_call_started(&self, call: &ToolCall) {
        self.runner.publish(TurnEvent::ToolCallStarted {
            turn_id: self.turn.clone(),
            call: call.clone(),
        });
    }

    fn tool_call_finished(&self, result: &ToolResult) {
        self.runner.publish(TurnEvent::ToolCallFinished {
            turn_id: self.turn.clone(),
            result: result.clone(),
        });
    }
}
