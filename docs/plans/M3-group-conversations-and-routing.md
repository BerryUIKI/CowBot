# M3 — Group conversations and routing

- Status: Implemented (M3.1–M3.7)
- Date: 2026-11-10
- Completed: 2026-11-11
- Related: [ADR-0030](../ADR/0030-conversation-sessions-and-context.md),
  [ADR-0032](../ADR/0032-group-conversations-and-routing.md),
  [ADR-0022](../ADR/0022-channel-mention-turns.md),
  [ADR-0024](../ADR/0024-agent-direct-conversations.md),
  [ADR-0028](../ADR/0028-main-assistant-and-workspace-actions.md),
  [ADR-0029](../ADR/0029-token-usage-accounting.md),
  [M1 usage accounting](M1-usage-accounting.md),
  [M2 conversations and context](M2-conversations-and-context.md)
- Follow-ups: M4 agent memory and shared notes, M5 prompt-cache optimisation.

## Goal

Let a user-created conversation hold several agent participants. A message is
answered by the participants it mentions, by a router-selected subset when
automatic routing is enabled, or by the conversation's default responder. Every
participant keeps an independent append-only context branch, and the
conversation header exposes per-agent context tabs.

This builds on M2. Single-agent conversations, direct conversations, and group
channels keep their existing behavior. The M1 plan's one-line description of M3
also mentioned shared notes; those remain M4. M3's shared state is the group
timeline and the peer-reply mirrors.

## Scope

### In scope

- Group conversations: `kind = conversation` with `mode = group` and two to
  eight ordered participants.
- Creating a conversation with several agents, one fixed provider/model, a
  default responder, and an optional automatic-routing setting.
- Mention routing scoped to the conversation's participants; `@team` and its
  aliases address every participant.
- A mention-aware router: explicit mentions always win; when no participant is
  mentioned and `router_enabled` is on, one model call selects one to three
  participants.
- A persisted default responder as the fallback when nobody is addressed,
  routing selects nobody, or routing fails. It is the Main Assistant when the
  Main Assistant is a participant and the user keeps the default.
- Concurrent responder turns with incremental, settle-order replies and
  per-agent failure isolation.
- Per-participant append-only context branches with a shared group timeline and
  attributed peer replies.
- Per-agent context tabs, token projections, cache summaries, compaction, and
  clear.
- Routing audit: one `conversation_turns` row per group user message, including
  silent and unmatched outcomes.
- One additive migration for participant order, the default responder, peer
  attribution, and turn records.
- Client, Tauri, desktop, package, CLI verification, tests, and documentation.

### Out of scope

- Automatic routing for group channels (`kind = channel`) or direct
  conversations. ADR-0022 behavior stays unchanged.
- Adding, removing, reordering, or muting participants after creation.
  Membership is fixed for the life of a group conversation in M3.
- Project-scoped conversations and project membership (M6).
- Agent long-term memory, shared notes, and retrieval (M4).
- Provider prompt-cache controls and cache breakpoints (M5).
- Per-participant provider or model selection; one fixed model per
  conversation.
- Agent-to-agent triggering, delegation rounds, debate, or autonomous
  multi-agent chatter. Only a user message starts a turn.
- Turn-level cancellation, a retry button for a failed participant, or turn
  grouping in the background panel.
- Streaming responses, user-facing threads, quote strips, and cost display.
- Exporting `conversation_turns` or `llm_calls` in `.cowpkg`; turn records stay
  device-local.
- User accounts as conversation participants; the local user is always the
  sender.

## Architecture summary

```
user posts a message (display message already persisted)
  -> parse mentions against the conversation's participants
  -> if a participant is mentioned: responders = mentioned participants
  -> else if router_enabled and participants > 1:
         one router model call over the roster and recent timeline
         responders = validated router selection (1..=3)
     else when a default responder is configured:
         responders = the default responder
     else:
         responders = none; a transient notice explains the silence
  -> persist one conversation_turns row for the message
  -> append the user entry to every participant branch
  -> assemble one request per responder from its own branch
  -> enqueue one scoped conversation turn per responder
  -> drain with per-task observation; each settled task posts its reply and
     records its assistant/tool messages in its own branch
  -> mirror every successful final reply into the other participants' branches
     as an attributed user entry
  -> routing notices, unknown-mention notices, and failures are display-only
```

M2's conversation path is unchanged for single-agent conversations. The group
path adds routing, turn persistence, observed drain, and peer mirroring around
the existing per-agent turn machinery.

## Product rules

1. A user-created conversation is `single` with one participant or `group`
   with two to eight participants. Mode is derived from the participant count
   and is not changed after creation.
2. Group conversations reuse `ChannelKind::Conversation`; no new channel kind
   is introduced. Group channels and direct conversations are untouched.
3. One provider/model is fixed at conversation creation and used by every
   participant and by the router. A provider deletion falls back to the active
   runtime exactly as in M2.
4. Explicit mentions always win. The router runs only when no participant is
   mentioned.
5. Mentions in a group conversation resolve against its participants, not the
   whole workspace. `@team`, `@all`, `@everyone`, and `@here` expand to every
   participant in participant order.
6. A mention of a workspace agent outside the conversation is rejected with a
   system notice listing the participants. It never adds the agent
   automatically. The Main Assistant is not special here; it participates only
   when it is a selected participant and is the default responder.
7. `router_enabled` is opt-in and defaults to off. When it is off, an
   unaddressed group message goes to the default responder when one is
   configured; otherwise the message is silent with a transient route notice.
8. When routing is on, the router returns zero to three participant names.
   Invalid names are discarded. If no valid participant remains, the default
   responder answers instead.
9. The default responder is a stored participant. At creation it is the
   participant whose name is `main` (case-insensitive, the built-in Main
   Assistant) when present, otherwise the first participant. The creation form
   lets the user choose it. An unset default responder leaves unaddressed
   messages silent with a notice rather than inventing an agent; a stored
   responder that is missing resolves to `main` when present, then to the first
   participant, with a notice. If no participant can be loaded, the turn posts
   a notice and does not call a model.
10. Selected participants run as independent roots in parallel. Each reply is
    posted as its task settles, so fast agents answer without waiting for the
    slowest. The persisted turn row preserves the selection order for
    attribution, and the transcript is honest settle order rather than a
    pretend synchronous order.
11. A failing participant posts its own failure notice; the other participants
    still answer. An empty answer posts a notice instead of a blank message.
    Skips caused by a full branch are reported per agent.
12. Agents in one turn answer the user message independently; they do not see
    each other's same-turn replies until the next turn. Settled replies enter
    the other participants' branches for later turns.
13. Plan mode remains per participant. A multi-responder `PlanOnly` turn
    produces one plan task per participant; the plans are approved from Tasks,
    and consolidated group-plan approval is deferred.
14. The approval level applies to every responder. Several approval cards can
    appear for one user message.
15. Only user messages trigger turns. Agent replies never trigger further
    turns, so group conversations cannot loop on their own.
16. Membership is fixed after creation. Changing the team means creating a new
    group conversation; the display history and branches of an old one stay
    intact.
17. Pre-turn failures (missing channel, store errors, invalid participant
    load) return an error. After tasks are enqueued, a group turn returns a
    report with per-agent `replied`, `failed`, and `skipped` outcomes even when
    every participant fails; the first scheduler error is logged, and task
    status is authoritative.

## Routing

### Mention routing

The service parses mentions with `cowbot-core::mention` and resolves them
against the conversation's participants in mention order. The parser stays
shared with group channels. The caller supplies the participant slice, so
`@team` means the group's participants rather than the workspace roster.

Unknown mentions and non-participant mentions produce one system notice. When a
message mixes valid and invalid mentions, the valid participants answer and the
notice lists the rejected names and available participants. Unknown mentions
are always reported in `ChannelTurnReport.unknown_mentions`; the current path
drops them when at least one agent resolves.

A message with no resolvable mention and no configured default responder writes
a `silent` turn row and posts a transient notice instead of a persisted system
notice.

### Router

The router is a single completion recorded as `LlmCallKind::Router`:

- input: the conversation's participant roster (name, role, goal, in
  participant order), the latest user message, and the recent shared timeline
  (up to ten non-system display messages rendered as `Name: content`, truncated
  to a bounded input budget);
- output: JSON only, `{"agents": ["Name", ...], "reason": "..."}`; a single
  `agent` string is also accepted;
- validation: match names or ids case-insensitively against participants,
  de-duplicate, preserve the router's order, and cap the result at three;
- failure: provider error, a 20-second timeout, malformed JSON, an empty
  selection, or only invalid names falls back to the default responder;
- visibility: the turn row stores the source, selected agents, reason, and
  router call id. The desktop shows an Answered-by line and a routing popover;
  a failure also posts a short system notice before the fallback answer;
- usage: a `LlmCallRecord` with `call_kind = router`, the conversation's
  `channel_id`, and no `run_id`, `task_id`, or `agent_id`. Recording is
  best-effort, matching ADR-0029. Router tokens are excluded from every
  participant's branch and cache summaries, and reported as a separate Routing
  row in the context popover.

The router uses the conversation's fixed model, temperature 0.0, and one
attempt. A confidence field, a separate router model or provider, retries, and
router evaluation are deferred; strict roster validation plus the default
fallback already prevents a bad selection from derailing a turn. Routing is
opt-in because it costs one extra model call per unaddressed message.

This does not contradict the recommendation in
[REFERENCES](../REFERENCES.md) to defer automatic channel routing. That
recommendation targets implicit routing in group channels; M3 stays
deterministic-first, is opt-in per user-created conversation, keeps the Main
Assistant available only as a chosen participant, and leaves ADR-0022 channel
behavior unchanged.

### Default responder

The default responder is the fallback when the user does not address anyone.
It is a persisted participant (`conversation_settings.default_responder`). At
creation the desktop preselects `main` when the Main Assistant is among the
chosen participants, otherwise the first participant. The user can choose a
different participant in the form.

This replaces a name-based global lookup at turn time: the Main Assistant is
the default only when it is deliberately in the group. A stored default that
names a non-participant resolves to `main` when present, then to the first
participant, with a notice. An unset default leaves unaddressed messages
silent with a notice. A group without a loadable participant posts a notice and
does not call a model.

## Context model

- Context stays keyed by `(channel, agent)` in `context_entries`, as in
  ADR-0030. Each participant has its own append-only branch; there is no
  shared branch row.
- The user message is appended to every participant's branch before assembly,
  including participants that do not answer. This keeps every participant
  current for later turns.
- A responder's assistant and tool messages are recorded in its own branch by
  the existing `ConversationRecorder` after a successful run. Tool calls and
  tool results stay only in the owner's branch.
- Every other participant's settled reply is mirrored into a branch as a
  `ContextRole::User` entry with `author_agent_id = Some(speaker)` and content
  `[AgentName] <reply>`. The group system prompt explains the convention so a
  model does not mistake a peer reply for the user or for its own words.
- `project_context` treats an entry with `author_agent_id` as history, never as
  the latest user message. This keeps the context ring and automatic
  compaction honest. Summaries of older entries keep authored entries as
  teammate text.
- System notices, routing notices, unknown-mention notices, and failure notices
  stay display-only and never enter a branch.
- The group system prompt is stable: agent prompt, platform context, the
  participant roster and default responder, and the peer-reply convention,
  followed by the active branch in sequence order. Group turns pass the
  platform context that the M2 conversation path currently omits. Provider
  cache breakpoints remain M5.

### Context tabs and compaction

- `ConversationContext` gains a participant list. Each entry carries the agent
  id and name, its projection, and its own cache summaries. The top-level
  single-agent fields remain the first participant for compatibility.
- The header ring shows the highest occupancy among participants. The popover
  lists one tab per participant with the token categories and cache hit rate
  for that branch.
- Manual compaction and clear are per participant. `clear` excludes only that
  branch and records one context event for it; a clear-all action repeats this
  for every participant. The existing channel-wide clear remains for channel
  history clearing.
- Automatic compaction runs for a responder immediately before its turn when
  that branch crosses the configured threshold, the same threshold and switch
  as M2. Non-responders are not compacted. Auto-compaction is silent per
  branch; if any branch was compacted, one aggregate notice explains it.
- Group sends are not pre-blocked by the composer. The service checks each
  selected branch after routing, compacts it when automatic compaction is on,
  and skips a branch that is still full with a system notice naming the
  participant; the other responders continue. When every selected branch is
  full, the turn posts a notice and no model call runs. The context ring warns
  as participants approach the limit. Single-agent conversations keep M2's
  pre-send block. The desktop supplies per-agent usable-window and tool-schema
  token estimates through `ChannelTurnOptions`, because the service does not
  own the model metadata or the tool registry; CLI callers pass no windows and
  therefore skip the percentage-based group gate.
- Cache summaries: the conversation total sums every call in the channel,
  including router and compaction calls. Per-agent tabs use the latest run for
  that agent and exclude router rows; the UI labels the conversation figure as
  the conversation total and per-agent figures as the latest run.

## Data model

M3 needs one additive migration, `0013_group_conversations.sql`:

- `channel_participants.position INTEGER NOT NULL DEFAULT 0` with an index, so
  participant order is stable and independent of `joined_at` ties; existing
  rows backfill deterministically in file/creation order.
- `conversation_settings.default_responder TEXT NULL`, referencing a
  participant; it is nullable for old rows and single conversations.
- `context_entries.author_agent_id TEXT NULL`, set only on mirrored peer
  replies. Existing rows and older packages deserialize as `None`.
- `conversation_turns`: id, channel_id, message_id (unique), source
  (`mention`, `router`, `default`, `silent`, `unmatched`), selected agent ids
  JSON, unknown mentions JSON, reason, router_call_id, task ids JSON,
  mirrored_at, created_at. It is the persisted routing audit and the
  reload-safe source for the Answered-by line. It cascades when the channel or
  message is deleted; clearing context does not delete turn rows.
- No columns are added to `channels`, `tasks`, `assignments`, or `llm_calls`.
  Peer attribution uses the new `author_agent_id` column rather than a stored
  enum value, so older readers can ignore the field and older packages still
  import.
- `ConversationSettingsUpdate` gains `router_enabled` and
  `default_responder`. Mode is derived from the participant count and is not
  user-editable.
- Participant ordering is selection order. The participant limit is eight;
  single conversations keep exactly one participant. A group cannot be
  converted to a single conversation or the reverse in M3.
- An imported row whose stored mode and participant count disagree is
  normalized on read: one participant is served as single, zero participants
  posts a notice, and a single conversation ignores `router_enabled`. An
  imported roster above eight stays readable with a warning but cannot be
  created through the UI.
- The turn row is the group turn record: one row per group user message,
  including silent and unmatched outcomes. `task_ids` links the created
  participant tasks; `run_group` is not reused and no `turn_id` is added to
  tasks. Turn grouping in the background panel is deferred.
- `conversation_turns` stays device-local. `.cowpkg` exports participant
  positions, the default responder, and authored context entries; route chips
  may be absent after import without corrupting the conversation.

## Client and desktop

- `Client::create_conversation` takes a `NewConversation` value: title, ordered
  agent ids, provider id, model, `router_enabled`, and `default_responder`.
  The single-agent form is the one-agent case. Creation validates one to eight
  unique workspace agents, rejects duplicates, and derives mode.
- New client/Tauri operations: conversation summaries with participants,
  participant listing, turn listing, per-participant context and contexts,
  per-agent compact, clear (per-agent and all), and settings updates for the
  router and default responder.
- `post_message` keeps its M2 pre-send capacity check for single conversations.
  Group conversations skip the pre-check; the service performs the post-routing
  check described above. The background turn reports the selected agents,
  replied, failed, skipped, source, and turn id in `cowbot-channel-reply`.
- New bus event: `ConversationRouted` (turn row). It flows on the existing
  `cowbot-event` channel; no new Tauri event channel. Settings changes return
  through their command, and membership is fixed, so no settings or
  participant event is added.
- Sidebar:
  - the New conversation form gains a group mode with a searchable participant
    list in creation order, a default-responder select, and an Automatic
    routing toggle shown for two or more agents;
  - group conversations show a group avatar and participant count in the
    Conversations section.
- Chat header:
  - participant chips with avatars and a participant-count badge;
  - the context ring and popover with one tab per participant;
  - the Automatic routing switch and default-responder label for group
    conversations.
- Timeline:
  - an Answered-by line under each user message with per-agent source badges
    (Mentioned, Routed, Default) and a popover showing the router reason and
    model when routing ran;
  - routing notices appear as system messages; replies use the existing agent
    attribution;
  - per-agent working rows while responders run, clearing as each reply posts;
  - failure and skip notices name the participant;
  - the empty state explains mentions, routing, and the default responder.
- New group UI components are extracted from `App.tsx` rather than added to
  it. The mention autocomplete and participant context tabs are keyboard
  accessible (roving focus, Escape, aria labels) and the first implementation
  covers a manual keyboard checklist.

## Work breakdown

### M3.1 `cowbot-core`: domain

- `MAX_GROUP_PARTICIPANTS = 8` and `MAX_ROUTED_AGENTS = 3`.
- `ChannelParticipant.position` with a serde default and builder.
- `ConversationSettings.default_responder` and a group constructor;
  `ConversationSettingsUpdate::router_enabled` and `default_responder`.
- `ContextEntry.author_agent_id` with a serde default and a peer constructor.
- `RouteSource` (`mention`, `router`, `default`, `silent`, `unmatched`),
  `TurnId`, and `ConversationTurn`.
- `EventPayload::ConversationRouted`.
- `ChannelTurnReport` outcome fields (`replied`, `failed`, `skipped`, `source`,
  `turn_id`) and `ChannelTurnOptions` capacity inputs.
- Participant-scoped mention resolution helper and participant-count helpers.
- `project_context` treats authored entries as history.
- Pure tests for mention scoping, route serde, authored projection, and
  context aggregation.

### M3.2 `cowbot-store`: schema and repositories

- Migration `0013_group_conversations.sql` as described above, with a
  deterministic position backfill.
- Participant position read/write and ordered listing.
- `conversation_turns` insert, task-id update, list by channel, and delete with
  the channel or message.
- Settings read/write for `default_responder`.
- Context-entry read/write for `author_agent_id`.
- Per-agent latest-run and conversation-total usage queries.
- Tests for migration, multi-participant creation, ordered participants, turn
  round trip, authored entries, settings round trip, and per-agent usage.

### M3.3 `cowbot-service`: group turns without the router

- Group creation with an ordered participant list, a fixed provider/model, and
  a default responder.
- Mention routing scoped to participants, unknown-mention reporting, and the
  silent/unmatched outcomes.
- Default-responder fallback and stale-default resolution.
- One user message fanning out to several scoped conversation turns.
- Per-participant branch append, assembly, peer mirroring with
  `author_agent_id`, and settled-reply posting.
- Turn-row persistence and `ConversationRouted`, including the route source
  and task ids.
- Per-agent context, compaction, clear, and post-routing capacity rules.
- An additive `run_until_idle_observed` scheduler path that reports each
  settled task without changing `run_until_idle` behavior.
- Group concurrency sized to the selected count, capped at eight.
- Tests for a mention turn, `@team`, unknown mentions, silent, fallback,
  multiple responders, partial failure, mirroring, and unchanged
  single/direct/channel behavior.

### M3.4 `cowbot-service` router

- Router request, prompt, JSON extraction, validation, timeout, and fallback.
- `LlmCallKind::Router` usage recording and turn-row linkage.
- A fake-provider test path that returns valid, invalid, empty, timed-out, and
  failing responses.
- Wiring behind `router_enabled`; M3.3 behavior is the fallback.

### M3.5 `cowbot-client` and Tauri

- `NewConversation` creation, conversation summaries, participant listing, turn
  listing, participant-aware context views, per-agent and clear-all context
  operations, and settings updates.
- Per-agent usable-window and tool-token estimates passed to the service for
  the post-routing capacity check.
- Extended reply event payload with outcome counts, selected agents, route
  source, and turn id.
- Client and Tauri tests.

### M3.6 Desktop UI

- Group creation form with participant picker in creation order, default
  responder select, and routing toggle.
- Participant header chips, per-agent context tabs, routing toggle, and
  default-responder label.
- Answered-by line, routing popover, per-agent working rows, failure/skip
  notices, empty state, and group avatars.
- Extract new group components from `App.tsx`; keyboard and aria behavior for
  the mention autocomplete and context tabs.
- `pnpm run build` and the `src-tauri` fmt/test/clippy gates.

### M3.7 Package, CLI, and documentation

- `.cowpkg` round trip for participant positions, default responder, and
  authored context entries; re-sequence legacy all-zero participant positions
  in file order; old packages import with defaults. Keep
  `conversation_turns` out of packages.
- CLI verification for `cowbot message post --wait` on a group conversation;
  no new CLI group-management commands.
- ADR-0032 for group conversations and routing; it amends ADR-0030's
  no-second-migration consequence and records the opt-in routing position
  against REFERENCES 3.7. Update PRD, ARCHITECTURE, ROADMAP, USER_GUIDE,
  PACKAGE_FORMAT, CLAUDE.md, and the docs index.
- Update this plan's status and acceptance checkboxes.

## Test plan

- `cargo test -p cowbot-core`
- `cargo test -p cowbot-store`
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

- Core: mention scoping, `@team` participants, route-source serde, authored
  projection, participant context aggregation, participant-count mode.
- Store: migration 0013, ordered participants, multi-participant creation,
  turn round trip, authored entries, default-responder settings, per-agent
  latest-run usage.
- Service: group creation, mention routing, unknown mentions, silent,
  default fallback, router enabled/disabled, router selection and failures,
  multiple responders, settle-order posting, partial failure, peer mirroring,
  per-agent compaction, clear, capacity skipping, observed drain, and
  unchanged surfaces.
- Client: group operations and a group turn through `reply_to_mentions`.
- Package: group participants, positions, default responder, and authored
  entries round-trip; old packages import; turn records are absent.
- Desktop: build gates plus a manual flow for creation, mention, routing,
  fallback, and per-agent context tabs.

## Acceptance criteria

- [x] A user can create a group conversation with two to eight ordered agents,
      a default responder, and one fixed provider/model.
- [x] `@Agent` addresses that participant; `@team` addresses every participant
      in participant order; non-participant mentions are rejected with a clear
      notice.
- [x] With routing off, an unaddressed message goes to the default responder;
      with no default responder it stays silent with a clear notice.
- [x] With routing on, the router selects one to three participants and its
      choice is visible; invalid, timed-out, or failed routing falls back to the
      default responder.
- [x] Several participants answer one message, failures are isolated, and each
      reply posts as its task settles.
- [x] Every participant has an independent append-only branch containing the
      shared group timeline, with peer replies attributed by
      `author_agent_id` and tool calls only in the owner's branch.
- [x] The context popover shows one tab per participant and compaction and
      clear act on the selected branch.
- [x] A full selected branch is skipped with a notice naming the participant
      while the others answer.
- [x] One `conversation_turns` row is written per group user message and drives
      the Answered-by line after a reload.
- [x] Membership is fixed after creation and the UI explains how to create a
      new group with a different team.
- [x] Single-agent conversations, direct conversations, and group channels
      keep their behavior.
- [x] Workspace packages round-trip group conversations; older packages import;
      turn records stay device-local.
- [x] Workspace and desktop gates pass.

## Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Router misroutes or adds cost | Opt-in toggle default off, one call per unaddressed message, three-responder cap, mentions always win, visible turn record, default fallback |
| Router output is malformed or names unknown agents | Strict JSON extraction, case-insensitive validation against participants, discard invalid names, fallback to the default responder |
| Group context grows quickly; mirrors multiply final replies across branches | Mirror only final replies, not tool loops; per-agent compaction and capacity checks; bounded router input; the eight-participant cap |
| Multiple responders create confusing order | Parallel roots, settle-order posting, explicit sender attribution, an Answered-by line with the persisted selection order |
| One failing participant hides the others | Per-agent failure notices; successful tasks post independently; the report lists what answered, failed, and was skipped |
| Approval-card flood under Ask or Plan | One policy for all responders; multiple cards are allowed; plan tasks are approved from Tasks |
| Default responder is missing or unset | Stale settings resolve to `main` then the first participant with a notice; an unset default stays silent with a notice; no guest agent |
| ADR-0030's no-migration claim is wrong | Migration 0013 is additive; ADR-0032 explicitly amends the consequence and records why position, default responder, attribution, and turn audit need storage |
| Prepared turns live only in scheduler memory | A crash can leave turn/task rows without a prepared request; document resend instead of promising exact recovery; startup reconciliation still covers running assignments |
| Concurrent sends interleave branches | Append-only sequence allocation is authoritative; M3 adds no per-conversation lock and documents settle-order interleaving |
| Package skew with newer fields | New fields are optional with serde defaults; older readers ignore them; authored peer entries degrade to user text in older builds; document the skew |
| Adding an observed drain changes scheduler behavior | Implement `run_until_idle_observed` additively and keep `run_until_idle` unchanged; cover claim failures, scope isolation, dependencies, and concurrent roots |

## Open at implementation

These are implementation details, not open product decisions:

- Exact router prompt wording, history window, input-token budget, and timeout
  handling.
- The precise `ConversationContext` shape and how much of the M2 top-level
  fields it keeps.
- The component split for the new group UI and the exact mention-autocomplete
  keyboard model.
- Whether the Answered-by popover shows the router reason verbatim or a
  shortened form.
- Exact store method names and whether turn-row insertion, task enqueue, and
  task-id update share a transaction boundary.
- The reconciliation policy for a turn row whose participant tasks never ran
  after a crash.

## Deferred decisions

- Participant membership editing, reordering, muting, and branch backfill.
- Per-participant provider or model selection.
- A separate router model/provider, retries, confidence calibration, and router
  evaluation.
- Turn-level cancellation, failed-participant retry, and turn grouping in the
  background panel.
- Agent-to-agent delegation, rounds, debate, or reply-to-agent triggering.
- Consolidated group plans under Plan mode.
- User-facing threads and quote strips.
- Exporting turn or LLM-call records in workspace packages.
- Shared notes and long-term memory (M4).
- Anthropic and Responses cache breakpoints (M5).
- User participants and remote sessions (Phase 2).
