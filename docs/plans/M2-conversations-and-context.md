# M2 — Single-agent conversations and the context meter

- Status: Implemented
- Date: 2026-11-09
- Related: [ADR-0030](../ADR/0030-conversation-sessions-and-context.md),
  [M1 usage accounting](M1-usage-accounting.md)
- Follow-ups: M3 group conversations and routing, M4 agent memory and shared
  notes, M5 prompt-cache optimisation.

## Goal

Give the desktop user a first-class conversation surface: create a
single-agent session, keep an independent append-only model context for it,
watch how full the model window is, and compress or reset the context without
losing the visible transcript.

This builds on the M1 usage foundation. Direct agent conversations and group
channels keep their existing behavior and do not show a context meter.

## Scope

### In scope

- `channels.kind` (`channel`, `direct`, `conversation`) with a backfill.
- Participants, conversation settings, context entries, and context events.
- Per-`(conversation, agent)` append-only context branches.
- Context assembly as a real message list, including tool-loop messages.
- Context projection with system, tools, history, and latest categories.
- A context ring and popover with percent used/left, token ratio, 70/90 states,
  an unknown-window fallback, and cache hit rate.
- Manual compaction, optional automatic compaction, clear context, and the
  full-context send block.
- Conversation creation, renaming, pinning, deletion, and clearing in the
  desktop sidebar and chat header.
- Workspace package round trip for conversation data.
- Tests and documentation.

### Out of scope

- Group conversations and multi-agent branches (M3).
- Mention routing, the main-agent fallback, and router calls (M3).
- Agent long-term memory, shared notes, FTS, or summaries across surfaces (M4).
- Anthropic `cache_control` / Responses cache keys and TTL (M5).
- Exact tokenizers, pricing, and cost display.

## Architecture summary

```
user posts message
  -> messages (visible timeline)
  -> context_entries (branch): append user entry
  -> assemble system + active branch entries
  -> AssignmentScheduler::enqueue_conversation_turn
  -> AgentInstance::execute_messages
       -> on success: record assistant/tool messages into the branch
  -> post assistant output as a display message
  -> llm_calls rows carry the conversation channel id
```

Projection is derived on read from the branch, the agent prompts, and the
desktop's tool registry. Compaction excludes a prefix of the branch and appends
one summary entry plus a context event.

## Work breakdown

### M2.1 `cowbot-core`: domain

- `ChannelKind`, `ChannelParticipant`, `ConversationMode`,
  `ConversationSettings`, `ConversationSettingsUpdate`.
- `ContextRole`, `ContextEntryKind`, `ContextToolCall`, `ContextEntry`,
  `ContextEventKind`, `ContextEvent`.
- `ContextProjection`, `ConversationContext`, `CompactionOutcome`,
  `CacheUsageSummary`.
- Pure helpers: `project_context`, `compaction_split`, `usable_window`,
  `context_usage_state`.
- `Channel.kind` with a `channel` default; `Channel::conversation`.
- `Task.channel_id` with a serde default and `Task::in_channel`.

### M2.2 `cowbot-store`: schema and repositories

Migration `0007_conversations.sql`:

- `channels.kind` plus an index and a `direct` backfill;
- `channel_participants`;
- `conversation_settings`;
- `context_entries` with `UNIQUE(channel_id, agent_id, seq)`;
- `context_events`.

Repository methods cover atomic conversation creation, participant listing,
settings read/update/restore, sequential entry append, batch append, exclusion,
history clear, compaction (exclude + summary + event in one transaction), event
listing, and per-channel usage totals plus the latest run's totals. Import
restore methods preserve ids, sequences, and timestamps.

### M2.3 `cowbot-agent`

- `ConversationRecorder` receives each assistant/tool message an agent appends.
- `AgentInstance::execute_messages` runs a caller-assembled request; the
  existing `execute_task` builds its messages and calls the same loop.
- The final assistant message is recorded as well.
- A recorder error fails the turn; context persistence is not best-effort.

### M2.4 `cowbot-service`

- `ConversationService`: create, settings, update, context projection, reply,
  compact, clear context.
- Reply appends the user entry, assembles the request, schedules the turn
  through a scoped scheduler, and posts the outcome.
- `AssignmentScheduler` stores a prepared request per conversation task and
  persists the recorded messages only on success; run usage rows include the
  task's channel id.
- `ChannelAgentService` delegates conversation channels to the new service.
- Channel delete refuses direct conversations only; history clear also clears
  conversation context.

### M2.5 `cowbot-client` and Tauri

Client operations: `get_channel`, `create_conversation`,
`conversation_settings`, `update_conversation_settings`,
`conversation_context`, `clear_conversation_context`. Compaction stays an
inherent local method because it needs the runtime LLM handle.

Tauri commands: `create_conversation`, `get_conversation_context`,
`update_conversation_settings`, `compact_conversation`,
`clear_conversation_context`. The context view merges desktop metadata: tool
schema tokens, context window, max output, usable window, percent, state, cache
summaries, and settings.

`post_message` checks capacity before persisting a conversation message, runs
automatic compaction when configured and the projected use crosses the
threshold, blocks at 100%, and resolves the conversation's fixed provider/model
for the reply and compaction calls.

### M2.6 Desktop UI

- A Conversations sidebar section with a New conversation form (title and
  agent), search, pin, rename, delete, compact, and clear-context actions.
- A context ring in the conversation header. Known windows show percent and a
  three-state colour (neutral below 70%, warning at 70%, danger at 90%, full at
  100%); unknown windows show the absolute token count.
- A popover with percent used/left, token ratio, category breakdown, cache hit
  rate for the last turn and the conversation, the fixed model, auto-compaction
  toggle and threshold, and compact/clear/new actions.
- A full-context banner and a disabled composer at 100%, offering compact,
  clear context, and new conversation.
- Conversations render in the existing bubble timeline; agent direct
  conversations and group channels are unchanged.

### M2.7 Package, CLI, and docs

- `.cowpkg` exports and imports participants, settings, context entries, and
  context events. Older packages without those files still import.
- CLI import output reports conversation and context counts.
- This plan, ADR-0030, the ADR index, architecture, roadmap, and user guide.

## Behavioral rules

1. Display messages and model context are separate; context operations never
   delete display history.
2. A conversation's provider and model are fixed at creation; provider deletion
   falls back to the active runtime without changing the model name.
3. The model request is system prompt plus active branch entries in sequence
   order; never one re-rendered transcript.
4. Assistant tool calls and their tool results are recorded together, and only
   after a successful turn.
5. Projection is estimated; provider usage remains authoritative for history.
6. The usable window is window minus max output minus a 1,024-token buffer;
   unknown metadata never yields a percentage.
7. Automatic compaction is off by default and runs only with a known window.
8. Compaction excludes entries, never deletes them, and records a context event.
9. Sending at 100% is refused before the message is persisted.
10. Direct conversations and group channels keep their M1 behavior.

## Test plan

- `cargo test -p cowbot-core`
- `cargo test -p cowbot-store`
- `cargo test -p cowbot-agent`
- `cargo test -p cowbot-service`
- `cargo test -p cowbot-client`
- `cargo test -p cowbot-package`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --check`
- Desktop: `pnpm run build`, `cargo fmt --manifest-path
  desktop/src-tauri/Cargo.toml -- --check`, `cargo test --manifest-path
  desktop/src-tauri/Cargo.toml`, `cargo clippy --manifest-path
  desktop/src-tauri/Cargo.toml --all-targets -- -D warnings`.

Coverage:

- Core: projection categories, compaction split with tool pairs, usable-window
  bounds, usage-state thresholds, cache hit rate, channel-kind and task serde.
- Store: migration backfill, conversation round trip, per-agent sequences,
  atomic compaction, exclusion/clear, channel usage sums, latest-run usage.
- Agent: prepared history execution records the tool loop and final message.
- Service: creation settings, append-only assembly, display/context separation,
  usage attribution, compaction, clear, and non-conversation rejection.
- Client: conversation operations and a turn through `reply_to_mentions`.
- Package: conversation context round trip and manifest coverage.
- Desktop: type-check/build plus the Tauri gates; meter state is computed by
  tested core helpers.

## Acceptance criteria

- [x] User-created single-agent conversations exist with a fixed provider and
      model.
- [x] Each `(conversation, agent)` has an append-only context branch separate
      from display messages.
- [x] The model request preserves system, user, assistant, and tool messages.
- [x] Context usage reports system, tools, history, latest, total, window, and
      percent.
- [x] The header shows a ring and popover with 70/90 states and an unknown-window
      fallback.
- [x] Cache hit rate is shown for the last turn and the conversation.
- [x] Manual compaction works and is audited; automatic compaction is optional
      and defaults off at 80%.
- [x] Sending at 100% is blocked with compact, clear, and new-conversation
      actions.
- [x] Direct conversations and group channels keep working unchanged.
- [x] Workspace packages round-trip conversation data; older packages import.
- [x] Workspace and desktop gates pass.

## Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Estimation drifts from the provider tokenizer | Show estimates as approximate; usage rows stay authoritative; the buffer reduces last-turn overflow |
| Compaction splits a tool call from its result | The split only moves to non-tool entries, and entries are recorded after a successful turn |
| Automatic compaction surprises the user | Off by default, explicit threshold, system notice after every compaction |
| Old databases fail the new migration | Additive migration with a backfill; earlier migrations untouched |
| Old packages lack the new files | Import treats them as optional |
| Provider deleted while a conversation exists | Fall back to the active provider with the fixed model name |
| Large summaries or transcripts | Summary input is capped at 24,000 estimated tokens; omitted entries are noted |
| Context persistence failure | The turn fails instead of silently diverging from the branch |

## Deferred decisions

- Group participation rules and routing (M3).
- Whether compaction should preserve pinned messages or allow a custom keep
  window.
- Automatic compaction trigger placement (before send vs after turn); M2 uses
  before send.
- A configurable buffer or model-specific output reserve.
- Exact tokenizer integration and cost display.
