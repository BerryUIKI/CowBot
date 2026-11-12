# ADR 0041: Turn streaming and cancellation

- Status: Accepted
- Date: 2026-11-13
- Discussion: [#180](https://github.com/BerryUIKI/CowBot/issues/180)
- Supersedes: None. Extends the assignment scheduler from [ADR-0019](0019-assignment-scheduler.md), the conversation branches of [ADR-0030](0030-conversation-sessions-and-context.md), and the group routing of [ADR-0032](0032-group-conversations-and-routing.md).

## Context

CowBot has no token-level streaming. `cowbot-llm` returns one complete
response, a conversation turn blocks until the assignment settles, and the
desktop shows a typing indicator instead of progress. The `Client` trait
exposes `cancel_task` but nothing can stop a turn; a user watching a long or
misguided reply has no way to end it, and the text the model already produced
is lost.

The gaps stack. A user must wait for the whole answer before seeing any of it,
cannot stop a bad turn before it spends more tokens, cannot see which tool a
paused turn is waiting on, and loses everything when a provider fails halfway
through a reply.

## Decision

### 1. Providers normalize streaming into one chunk contract

`LlmProvider::chat_stream` delivers `StreamChunk::Text`, `Reasoning`,
`ToolCall`, and `Usage` pieces, and returns the accumulated
`CompletionOutcome`. OpenAI Chat Completions, Anthropic Messages, and OpenAI
Responses implement it natively over server-sent events through a shared SSE
decoder; the non-streaming `chat_complete` stays for callers that never need
deltas, and a default adapter in the trait turns any non-streaming provider
into a one-chunk stream, so nothing breaks when a provider does not stream.

Tool calls stream as argument fragments (OpenAI indexes them, Anthropic closes
a `tool_use` block, Responses emits `function_call_arguments.delta`); the
decoders assemble a complete call before emitting it, because a half-received
argument JSON is not usable.

### 2. Cancellation is a shared signal in core

`cowbot-core::cancel::CancelSignal` is a clonable one-way flag with wakers. The
turn runner trips it, the agent loop checks it between tool calls, and the
provider stream races `cancelled()` against every upstream read, so a stop
drops the in-flight HTTP response instead of waiting for the next chunk. A
stopped request returns `CowError::Cancelled`, which is not a failure: callers
persist what was produced.

### 3. The agent loop observes a turn, it does not own one

`AgentInstance` runs with a `TurnObserver` (text and reasoning deltas, tool
started/finished) and keeps a running `draft` of the text the model produced.
It knows nothing about turn ids, the bus, or persistence — that belongs above
it — but after any outcome, stopped or failed included, the draft holds
whatever the user already saw.

### 4. One runner owns the lifecycle

Every turn — a direct conversation reply, a group reply, a channel mention
turn, a task run — is opened by `cowbot-service::turns::TurnRunner` with a
fresh `TurnId`, streams its progress as `TurnEvent`s on the bus, and is
finished by it after the reply is stored. The services share one runner
registry, so `cancel_turn(turn_id)` reaches a turn wherever it runs, and is
idempotent: an unknown or finished turn returns `false` and does nothing.

The runner persists before publishing the terminal event. A stop keeps the
text it produced and stores it with a `_[Stopped]_` marker; a task turn with
text completes its assignment with that partial output rather than failing it.
A provider failure after partial text reports `turn_error` with the partial
text, so a reload shows what the user saw instead of nothing.

### 5. The turn event contract

One payload carries every lifecycle event: `turn_started`, `text_delta`,
`reasoning_delta`, `tool_call_started`, `tool_call_finished`,
`approval_required`, `turn_done`, `turn_error`. It rides the existing event bus
and the desktop's `cowbot-event` channel, so a client subscribes once and
needs no new transport. An approval pause is a normal state, not an error: the
existing `ApprovalRequested` flow renders the card, the turn stays open while
the gate waits, and the same turn continues when the decision lands.

### 6. The desktop renders the stream and can stop it

The chat view folds each event into a live bubble — reasoning behind a
collapsible, tool chips that tick from running to done, text rendered as it
arrives — with a stop button beside the composer while anything streams. The
terminal event follows persistence, so the stored message replaces the bubble
and a reload shows exactly that state.

## Alternatives considered

**Stream only the final assistant text.** Rejected: tool calls and approvals
are where a user most needs progress, and a stop has to interrupt them.

**Per-path stop logic in chat, group, and task services.** Rejected: three
implementations of the same lifecycle would drift; the issue explicitly
requires one runner.

**Treat a stop as a failure.** Rejected: the task history would show an error
for a deliberate user action, and the partial text — which the user watched
being produced — would be discarded.

**A transport-level stream (SSE/WebSocket).** Deferred to Phase 2: the desktop
uses Tauri events, and the bus payload carries the same semantics a remote
transport will.

## Consequences

- Every provider gains a streaming path; a provider without one still works
  through the default adapter, at the cost of one large text chunk instead of
  incremental deltas.
- A stopped turn's partial reply, with the stopped marker, becomes part of the
  conversation branch and the task output, so the history reflects what was
  kept rather than what was attempted.
- The agent loop now streams by default; `chat_complete` callers are the
  exception (probes, compaction, memory extraction) and keep their behavior.
- Turn ids are ephemeral in-process registry keys, not persisted rows; a turn
  that outlives its process cannot be cancelled, and its assignment settles
  through the existing startup reconciliation.
- Group peer mirroring still happens at settlement; a stopped group reply
  mirrors its partial text the same way a completed one mirrors its answer.
