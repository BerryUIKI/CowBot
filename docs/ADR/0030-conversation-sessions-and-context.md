# ADR 0030: Conversation sessions and context accounting

- Status: Accepted
- Date: 2026-11-09
- Plan: [M2 — single-agent conversations and the context meter](../plans/M2-conversations-and-context.md)
- Related: [ADR-0024](0024-agent-direct-conversations.md), [ADR-0029](0029-token-usage-accounting.md)
- Supersedes: None

## Context

ADR-0024 gave every agent a direct conversation: a single-addressee channel in
which every user message reaches that agent without an `@mention`. Those
conversations are agent-managed: each turn renders the recent timeline into one
task description, the context is bounded by a fixed history limit, and there is
no visible measure of how much of the model window a turn consumes.

M1 (ADR-0029) added normalized token usage per model call, persisted in
`llm_calls`, plus optional model context-window metadata in the desktop
configuration. The next milestone needs a surface where the user can hold a
long-running exchange with one agent, see the context they are accumulating,
and decide when to compress it.

The agreed direction from the M1 plan is:

- user-created conversations get an independent append-only context branch per
  participant, visible context accounting, and manual plus optional automatic
  compaction;
- agent direct conversations and group channels keep their existing,
  agent-managed context and do not show a meter;
- group conversations, routing, and the main-agent fallback are M3;
- agent memory and shared notes are M4.

A model request must contain real conversation turns, not a transcript
re-rendered into one user message: tool-loop assistant and tool messages have to
survive across turns, and the system prompt must stay stable at the front so
provider prompt caches can hit.

## Decision

### 1. Channels gain a kind

`channels.kind` distinguishes:

- `channel` — a group channel driven by `@mentions` (ADR-0022);
- `direct` — the agent-owned conversation provisioned per agent (ADR-0024);
- `conversation` — a user-created session with its own participant set and
  context branch.

Migration `0007_conversations.sql` adds the column with a `channel` default and
backfills `direct` for rows that already have `agent_id`. Earlier migrations
stay untouched (ADR-0025). `Channel` carries `kind` and defaults to `channel`
when deserializing older payloads.

A conversation row has no `agent_id`: that column keeps its unique
`(workspace_id, agent_id)` index and identifies direct conversations only.
Conversation participants live in `channel_participants`, which allows the M3
group mode without a second migration for membership.

### 2. Conversations have fixed settings

`conversation_settings` stores one row per conversation:

- `mode` — `single` or `group`; M2 creates and serves only `single`, and the
  service rejects group mode until M3;
- `provider_id` and `model` — fixed at creation so a later provider switch in
  Settings does not silently move the conversation to another model;
- `router_enabled` — reserved for M3, always false in M2;
- `auto_compact` and `compact_threshold` — default off and 80 percent.

The desktop resolves `provider_id` against its local provider configuration at
turn time. If the provider was deleted, the active runtime provider is used with
the conversation's fixed model name, so the conversation keeps working.

### 3. Display history and model context are separate

`messages` remains the user-visible timeline. `context_entries` holds the model
context as an append-only branch keyed by `(channel_id, agent_id, seq)`:

- role (`system`, `user`, `assistant`, `tool`), kind (`message`, `summary`),
  content, optional tool calls and tool-call id, an exclusion flag, estimated
  tokens, and a timestamp;
- sequence numbers are allocated inside the branch by SQLite, so concurrent
  writers cannot duplicate a position;
- display messages are never deleted by context operations, and context entries
  are never rewritten during normal use.

A user turn appends the user entry before assembly, then builds the request as:
the agent system prompt (including the workspace platform context), followed by
the active entries in sequence order. Assistant messages with tool calls and the
matching tool results are stored and replayed as such. A turn's assistant and
tool messages are appended only after the run succeeds, so a failed run never
leaves an orphaned tool call in the branch.

The agent runtime gains `execute_messages` and an optional
`ConversationRecorder`; the assignment scheduler keeps a prepared request for
the conversation task and persists the recorded messages on completion. The
context branch therefore does not depend on parsing task descriptions or the
timeline back into messages.

Usage rows gain the conversation channel id: `Task` now carries the channel it
was created in, and the scheduler binds it to the run usage recorder, so the
meter can report cache hit rate per turn and per conversation without joins.

### 4. Context projection and the meter

Projection is deterministic and estimated, never authoritative:

- system tokens: the same system prompt the turn will send;
- tool tokens: the schemas of the agent's assigned tools (the desktop adds
  these because the tool registry is desktop runtime state);
- history tokens: active branch entries except the latest active user entry;
- latest tokens: the latest active user entry;
- total: the sum, compared against a usable window.

The usable window is `context_window - max_output_tokens - 1024` (a buffer for
tokenizer drift). Unknown metadata yields no percentage and the UI shows
absolute token counts. Percent used is anchored at 70% (warning) and 90%
(danger); at 100% sending is blocked.

The conversation header shows a ring with percent used and a popover with
percent used/left, the token ratio, the four categories, and cache hit rate for
the last turn and the whole conversation. Cache hit rate is
`cached_input / total_input` from `llm_calls`; a turn is the set of calls in the
latest run for the channel.

### 5. Compaction is explicit, reversible in display, and audited

Compaction keeps the most recent ~2,000 tokens of active entries (and at least
the last two), moving the split back so a tool result is never separated from
the assistant message that requested it. Older entries are summarized by one
model call with `call_kind = compact`:

- the summary is appended as a `summary` system entry;
- summarized entries are marked `excluded`, never deleted;
- a `context_events` row records kind, token counts before and after, and how
  many entries were summarized;
- a system notice in the timeline explains what happened.

Manual compaction is always available from the header popover and the
conversation menu. Automatic compaction is per conversation, off by default,
and runs before a send when the projected occupancy after the new message would
reach the configured percentage of the usable window. At 100% the desktop
refuses to persist the message and offers compact, clear context, and new
conversation.

Clearing the context marks all active entries excluded, records a `clear`
event, and keeps display messages. Clearing a conversation's history deletes
both display messages and context rows, because leaving hidden context behind
would contradict the visible reset.

### 6. The workspace package carries conversation data

`.cowpkg` export includes participants, settings, context entries, and context
events alongside channels and messages. Import treats the new files as optional
so packages written before this milestone still import; a package without them
creates the conversation channel without a participant and the desktop reports
that state instead of failing.

### 7. Existing surfaces keep their behavior

Direct conversations still render recent timeline history into a task
description and still cannot be deleted. Group channels still use mention
resolution and their approval policy. Only `conversation` channels use the new
context path and show the meter. Channel names remain unique per workspace
across every kind.

## Alternatives considered

- **Reuse direct conversations as the new session type.** Agent-owned channels
  have a unique index per agent and a bounded, rendered history; converting them
  would break existing conversations, prevent multiple sessions with one agent,
  and remove the distinction M1 documented. Rejected.
- **Store the rendered transcript as one user message.** Simpler to assemble,
  but loses tool-call structure, defeats prompt caching, and cannot be compacted
  by entry. Rejected.
- **Rewrite context entries in place on every turn.** Breaks the append-only
  property and makes compaction and audit ambiguous. Rejected; only the
  exclusion flag changes.
- **Compute percentages without model metadata.** A guessed window produces
  wrong alarms and wrong automatic compaction. Unknown is an explicit state.
- **Compact by deleting the oldest messages.** Display history must survive a
  context reset; exclusion keeps the transcript intact while shrinking the
  model request. Rejected.
- **Automatic compaction by default.** Surprise model calls and a rewriting
  context are user-visible behavior; off by default with an explicit threshold
  is predictable.

## Consequences

- New tables and a migration; existing databases backfill channel kinds.
- The scheduler carries a prepared conversation request, a new local-only
  execution path next to task-description execution.
- Context accounting is estimated until the next call returns provider usage;
  the meter is a planning aid, not a billing source.
- Compaction costs one model call and may lose detail; the original entries stay
  in the database and remain visible in the timeline.
- Group conversations can reuse the same schema (`mode = group`, several
  participants, `router_enabled`) without another migration.
- Packages grow by four JSONL files; old packages remain importable.
- Prompt caching improves only if the prefix stays stable; the system prompt
  and sequence order are deterministic by construction (provider-specific cache
  breakpoints remain M5).
