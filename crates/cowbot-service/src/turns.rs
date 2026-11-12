//! One runner for every turn: direct chat, group reply, channel mention, task.
//!
//! A turn is the unit a user starts and can stop. Every path registers its
//! [`TurnId`] with the same [`TurnRunner`], which assigns the id, publishes the
//! lifecycle events, persists the final message before the terminal event, and
//! turns a stop request into a partial reply with a `stopped` marker. The chat,
//! group, and task paths call into it; none of them reimplements stopping or
//! persistence (ADR-0039).

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use cowbot_agent::TurnObserver;
use cowbot_bus::EventBus;
use cowbot_core::approval::{Approval, ApprovalStatus};
use cowbot_core::cancel::CancelSignal;
use cowbot_core::channel::{ChannelId, Sender};
use cowbot_core::conversation::{ContextEntry, TurnId};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::event::{Event, EventPayload};
use cowbot_core::task::{TaskId, TaskStatus};
use cowbot_core::tool::{ToolCall, ToolResult};
use cowbot_core::turn::{PendingApproval, TurnControl, TurnEvent, TurnOutcome, TurnTarget};
use cowbot_store::Store;

use crate::channels::ChannelService;

/// Marker appended to a reply the user stopped, so a reload shows why the
/// answer ends abruptly.
pub const STOPPED_MARKER: &str = "_[Stopped]_";

/// Where a turn's final reply is written.
#[derive(Debug, Clone)]
pub enum TurnSink {
    /// A conversation reply: the text becomes a message and, for a
    /// user-created conversation, an assistant entry in the participant's
    /// context branch.
    Conversation {
        channel_id: ChannelId,
        agent_id: cowbot_core::agent::AgentId,
        /// Write the reply into the participant's context branch.
        record_context: bool,
    },
    /// A task run: the text becomes the task's output.
    Task { task_id: TaskId },
}

/// What a runner needs to open a turn.
#[derive(Debug, Clone)]
pub struct TurnSpec {
    pub target: TurnTarget,
    /// Every agent that answers in this turn.
    pub agent_ids: Vec<cowbot_core::agent::AgentId>,
    pub sink: TurnSink,
}

/// An open turn: its cancellation scope and everything needed to finish it.
struct OpenTurn {
    spec: TurnSpec,
    cancel: CancelSignal,
    channel_id: Option<ChannelId>,
}

/// Upper bound on retained turn state, so a long-lived process cannot grow
/// without bound if a caller forgets [`TurnControl::close`].
const MAX_OPEN_TURNS: usize = 512;

/// Registers turns, publishes their events, and persists their outcome.
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
        self.open.insert(
            turn.clone(),
            OpenTurn {
                cancel: CancelSignal::new(),
                channel_id,
                spec: spec.clone(),
            },
        );
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

    fn spec(&self, turn: &TurnId) -> Option<TurnSpec> {
        self.open
            .lock()
            .ok()
            .and_then(|open| open.get(turn).map(|entry| entry.spec.clone()))
    }

    /// Finish a turn: persist first, then publish the terminal event.
    ///
    /// A stopped turn keeps the text it produced and is persisted with the
    /// stopped marker; a failed turn with partial text is persisted too, so a
    /// reload never loses what the user already saw (ADR-0039).
    pub async fn finish(&self, turn: &TurnId, result: CowResult<String>) -> TurnOutcome {
        let Some(open) = self.open.lock().ok().and_then(|mut open| open.remove(turn)) else {
            // A turn that was never opened (or already finished) still reports
            // its outcome instead of panicking on a duplicate finish.
            return match result {
                Ok(message) => TurnOutcome::Done {
                    message,
                    stopped: false,
                },
                Err(error) => TurnOutcome::Failed {
                    error: error.to_string(),
                    partial: String::new(),
                },
            };
        };

        let stopped = open.cancel.is_cancelled();
        let outcome = match result {
            Ok(message) => {
                // The caller keeps the model's own text; the stopped marker is
                // added only where the reply is stored.
                let text = message;
                let persisted = self.persist(&open.spec.sink, &text, stopped).await;
                match persisted {
                    Ok(()) => TurnOutcome::Done {
                        message: text,
                        stopped,
                    },
                    Err(error) => TurnOutcome::Failed {
                        error: error.to_string(),
                        partial: text,
                    },
                }
            }
            Err(CowError::Cancelled) => {
                // An empty stop: nothing was produced before the signal. The
                // sink still records that the turn stopped on purpose.
                let text = String::new();
                let persisted = self.persist(&open.spec.sink, &text, true).await;
                match persisted {
                    Ok(()) => TurnOutcome::Done {
                        message: text,
                        stopped: true,
                    },
                    Err(error) => TurnOutcome::Failed {
                        error: error.to_string(),
                        partial: text,
                    },
                }
            }
            Err(error) => TurnOutcome::Failed {
                error: error.to_string(),
                partial: String::new(),
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

    /// Write the final text where the turn's sink keeps it.
    async fn persist(&self, sink: &TurnSink, text: &str, stopped: bool) -> CowResult<()> {
        match sink {
            TurnSink::Task { task_id } => {
                let mut text = text.to_string();
                if stopped {
                    text = stopped_text(&text);
                }
                let output = text.trim();
                let status = if output.is_empty() {
                    TaskStatus::Completed {
                        output: "Task stopped before it produced output.".to_string(),
                    }
                } else {
                    TaskStatus::Completed {
                        output: text.clone(),
                    }
                };
                self.store.update_task_status(task_id, &status).await?;
                self.bus.publish(Event::new(EventPayload::TaskCompleted {
                    task_id: task_id.clone(),
                    output: text,
                }));
                Ok(())
            }
            TurnSink::Conversation {
                channel_id,
                agent_id,
                record_context,
            } => {
                let content = text.trim();
                if content.is_empty() {
                    self.channels
                        .post_system_notice(
                            channel_id,
                            if stopped {
                                "The reply was stopped before it produced text."
                            } else {
                                "The agent finished without a reply."
                            },
                        )
                        .await?;
                    return Ok(());
                }
                self.channels
                    .post_message(channel_id, Sender::Agent(agent_id.clone()), content)
                    .await?;
                if *record_context {
                    let mut entry = ContextEntry::new(
                        channel_id.clone(),
                        agent_id.clone(),
                        cowbot_core::conversation::ContextRole::Assistant,
                        content,
                    );
                    entry.recount();
                    self.store.append_context_entry(&entry).await?;
                }
                Ok(())
            }
        }
    }
}

/// Append the stopped marker when the text does not already carry it.
fn stopped_text(text: &str) -> String {
    let trimmed = text.trim_end();
    if trimmed.contains(STOPPED_MARKER) {
        return text.to_string();
    }
    if trimmed.is_empty() {
        STOPPED_MARKER.to_string()
    } else {
        format!("{trimmed}\n\n{STOPPED_MARKER}")
    }
}

impl TurnControl for TurnRunner {
    fn open(&self, turn: TurnId, channel_id: Option<ChannelId>) {
        if let Ok(mut open) = self.open.lock() {
            if open.len() >= MAX_OPEN_TURNS {
                // Evict the oldest entry; the cap exists so a leaked turn
                // cannot grow this map without bound.
                if let Some(victim) = open.keys().next().cloned() {
                    open.remove(&victim);
                }
            }
            open.entry(turn).or_insert_with(|| OpenTurn {
                spec: TurnSpec {
                    target: TurnTarget::Task {
                        task_id: TaskId::new(String::new()),
                        agent_id: cowbot_core::agent::AgentId(String::new()),
                    },
                    agent_ids: Vec::new(),
                    sink: TurnSink::Task {
                        task_id: TaskId::new(String::new()),
                    },
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

/// Announce a tool call that is waiting for a human decision.
///
/// An approval pause is a normal turn state, not an error: the same turn
/// resumes once the decision lands (ADR-0039).
pub fn publish_approval_required(
    bus: &EventBus,
    turn: &TurnId,
    approval: Approval,
    call: ToolCall,
) {
    if approval.status != ApprovalStatus::Pending {
        return;
    }
    bus.publish(Event::new(EventPayload::Turn(
        TurnEvent::ApprovalRequired {
            turn_id: turn.clone(),
            approval: PendingApproval { approval, call },
        },
    )));
}
