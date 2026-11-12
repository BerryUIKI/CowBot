# ADR 0037: Redacted prompt snapshots for LLM call audit

- Status: Accepted
- Date: 2026-11-12
- Discussion: [#178](https://github.com/BerryUIKI/CowBot/issues/178)
- Supersedes: None. Refines [ADR-0029](0029-token-usage-accounting.md) and [ADR-0036](0036-context-preview-and-recall-trace.md).

## Context

`llm_calls` records the model, call kind, normalized usage, and the provider's
raw usage object, but not the prompt. `audit_events` records approval decisions
but not the context that led to a tool call. Rebuilding the prompt afterwards is
not enough for audit: settings, memory, compaction, tool lists, and the model can
all change between the call and the review, so the rebuilt request is a
different request.

ADR-0036 added a read-only `PromptPlan` that rebuilds the *next* request for the
context inspector. That answers "what would be sent", not "what was sent". The
audit counterpart has to capture the request at the moment it is sent.

The constraints are privacy and lifecycle. A request can contain API keys pasted
into a conversation, credentials inside a shell command, a PEM block in a tool
result, or a token in a URL. Snapshots are therefore redacted before storage,
bounded in size, and expire on a workspace retention clock. They are device-local
evidence and never leave the machine.

## Decision

### 1. One sealed JSON document per LLM call

`cowbot-core::prompt_snapshot` defines `LlmPromptSnapshot`: the ordered
`SnapshotMessage`s with their tool calls and arguments, the tool schemas, the
named prompt slots, the memory block with the selected memory entry ids, the
optional context projection, the model, the call kind, a schema version, a
token count, and a content hash. `SnapshotMessage` is deliberately not
`PromptMessage`: a snapshot keeps tool-call arguments verbatim, while a message
in a preview only names the calls.

The document is versioned (`PROMPT_SNAPSHOT_SCHEMA_VERSION`). A reader accepts
every version it knows and refuses a newer one, so an application upgrade never
hides pre-upgrade evidence.

### 2. Sealing is redact, bound, count, hash

`LlmPromptSnapshot::seal` runs four deterministic steps:

1. **Redact.** Every text field is passed through a stricter redactor than the
   memory path: every sensitive `key=value` / `key: value` pair on a line is
   cleared (only its value, not the rest of the line), bearer tokens are
   replaced, words with a known credential prefix are replaced, PEM blocks are
   removed whole, and a JSON value under a sensitive key is cleared at any
   depth. Redaction works per string before serialization, so the document
   stays valid JSON.
2. **Bound.** The payload is kept under 256 KiB. Over-long message bodies are
   truncated with the shared marker, then the oldest non-system messages are
   dropped, then tool schemas and slot bodies are shortened. Every step sets
   `truncated` and counts `omitted_messages`, so a shortened record never
   passes for a complete one.
3. **Count.** `token_count` is the request size under the model's tokenizer:
   every message including framing plus the serialized tool array.
4. **Hash.** `content_hash` is the SHA-256 of the stored document with the hash
   field cleared. Sealing twice produces byte-identical output, so the hash is
   stable; a read that no longer matches is reported instead of trusted.

### 3. A separate table, keyed by the call

Migration `0017_prompt_snapshots.sql` adds `llm_prompt_snapshots`, keyed by
`llm_call_id` and cascading with the call, and
`workspace_prompt_snapshot_settings`. The row carries `workspace_id`,
`schema_version`, `content_hash`, `token_count`, `byte_size`, the truncation and
redaction flags, `created_at`, and the payload. Putting the payload in the hot
`llm_calls` row would bloat every usage query and make retention one large
UPDATE; a separate row also lets `.cowpkg` export stay table-driven.

The workspace id is denormalized on the snapshot row so retention and channel
purges stay scoped even if a later build serves several workspaces from one
database.

### 4. Capture happens where the request exists

The agent runtime captures each iteration's request as it is sent, and the
service captures the router, compaction, and memory-extraction requests at their
call sites. `LlmCallUsage` carries the unsealed snapshot to the recorder, which
adds the prompt slots and memory selection it knows, seals, and stores it beside
the usage row. Usage is written first; a snapshot failure is logged and never
fails the turn.

`LlmUsageRecorder::records_prompts` lets a recorder say whether the caller
should pay for capture at all. The store still gates on the workspace switch, so
the switch is authoritative for every call kind.

### 5. Retention, deletion, and export

The workspace setting `enabled` (default on) and `retention_days` (default 30,
clamped to 1–3650) live in `workspace_prompt_snapshot_settings`. A sweep deletes
rows older than the window without touching `llm_calls`, and it runs when a
workspace opens, when the setting changes, and on demand from Settings → Audit.
Turning recording off stops new evidence but keeps the retention clock: old
evidence still expires.

Deleting a channel deletes the snapshots of the calls made in it. Clearing a
channel's history does not: what a call sent is historical fact, not display
history.

Snapshots are excluded from `.cowpkg` export, as a digest too. A package that
carries a snapshot file is rejected as an unexpected path.

### 6. Surfaces

The client boundary and the Tauri shell expose snapshot listing for a run, the
recent list, a verified detail read, and get/update/purge settings. Task detail
shows a run's recorded prompts in a viewer, and Settings → Audit shows retention
controls beside the newest recorded prompts. A snapshot labelled `redacted`,
`truncated`, or hash-mismatched says so in the viewer.

## Alternatives considered

- **Recompute on demand.** The ADR-0036 preview model. Cheap, but it cannot prove
  what was sent once settings, memory, or the model changed.
- **Store the full prompt inline in `llm_calls`.** Simple, but it bloats the hot
  usage table, makes every usage query read a large column, and entangles
  retention with usage rows.
- **Store only a hash.** Cheap and private, but it proves integrity without
  showing the reviewer anything, which fails the audit purpose.
- **Compress the payload.** Worth doing later; the 256 KiB bound already keeps
  the default retention (30 days) within a modest size.

## Consequences

- A workspace with snapshots on pays one bounded JSON document per agent
  iteration. The default retention and the capture switch bound growth; the
  audit panel shows retained size per row.
- Redaction is heuristic. It is stricter than the memory path and covers the
  common credential shapes, but a secret without a recognizable shape survives.
  Retention is the second line of defence and the viewer labels redacted rows.
- Conversation snapshots repeat context entries that also live in the branch.
  That duplication is the point: audit needs the exact sent request.
- The preview and the snapshot are deliberately different artifacts. "Inspect
  context" shows the next request; a snapshot shows the recorded one.
- Expired rows are deleted when a workspace opens, when the settings change, or
  on demand — not by a background scheduler.
- Linking `audit_events` and `tool_calls` to the call that produced them is out
  of scope; the audit view lists snapshots on their own.
