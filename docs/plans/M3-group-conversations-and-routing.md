# M3 — Group conversations and routing

- Status: Planned (not started)
- Date: 2026-11-10
- Related: [ADR-0030](../ADR/0030-conversation-sessions-and-context.md),
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
  eight participants.
- Creating a conversation with several agents, one fixed provider/model, an
  optional default responder, and an optional automatic-routing setting.
- Mention routing scoped to the conversation's participants; `@team` and its
  aliases address every participant.
- A mention-aware router: explicit mentions always win; when no participant is
  mentioned and `router_enabled` is on, one model call selects one to three
  participants.
- A persisted default responder as the fallback when nobody is addressed,
  routing selects nobody, or routing fails. It defaults to the Main Assistant
  when the Main Assistant is a participant.
- Concurrent responder turns with deterministic reply order and per-agent
  failure isolation.
- Per-participant append-only context branches with a shared group timeline.
- Per-agent context tabs, token projections, cache summaries, compaction, and
  clear.
- One additive migration for participant position, the default responder, and
  routing audit.
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
- Turn-level cancellation and turn grouping in the background panel.
- Streaming responses, user-facing threads, and cost display.
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
     else or when routing finds nobody or fails:
         responders = the conversation's default responder
  -> persist a conversation_routes row for the message
  -> append the user entry to every participant branch
  -> assemble one request per responder from its own branch
  -> enqueue one scoped conversation turn per responder
  -> each turn records its assistant/tool messages in its own branch
  -> post settled replies (agent attribution) in responder order
  -> mirror every settled reply into the other participants' branches as a
     peer entry
  -> routing notices, unknown-mention notices, and failures are display-only
```

M2's conversation path is unchanged for single-agent conversations. The group
path adds routing, route persistence, and peer mirroring around the existing
per-agent turn machinery.

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
   when it is a selected participant.
7. `router_enabled` is opt-in and defaults to off. When it is off, an
   unaddressed group message goes to the default responder.
8. When routing is on, the router returns zero to three participant names.
   Invalid names are discarded. If no valid participant remains, the default
   responder answers instead.
9. The default responder is a stored participant. At creation it is the
   participant whose name is `main` (case-insensitive, the built-in Main
   Assistant) when present, otherwise the first participant. The creation form
   lets the user choose it. If it is missing or not a participant when a turn
   runs, the service uses the first participant and posts a notice; if the
   participant list is empty, it posts a notice and does not call a model.
10. A user message can run several responder turns concurrently. Responses are
    posted in responder order after the turn settles, not in completion order,
    so the transcript is deterministic.
11. A failing participant posts its own failure notice; the other participants
    still answer. An empty answer posts a notice instead of a blank message.
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

### Router

The router is a single completion recorded as `LlmCallKind::Router`:

- input: the conversation's participant roster (name, role, goal, in
  participant order), the latest user message, and the recent shared timeline
  (up to eight non-system messages rendered as `Name: content`, truncated to a
  bounded input budget);
- output: JSON only, `{"agents": ["Name", ...], "confidence": 0.0-1.0,
  "reason": "..."}`; a single `agent` string is also accepted;
- validation: match names or ids case-insensitively against participants,
  de-duplicate, preserve the router's order, and cap the result at three;
- confidence: a missing confidence is treated as 0.5; a confidence below 0.4
  rejects the selection and falls back to the default responder;
- failure: provider error, a 20-second timeout, malformed JSON, an empty
  selection, or only invalid names falls back to the default responder;
- visibility: the route row stores the source, selected agents, confidence,
  reason, and router call id. The desktop shows an Answered-by line and a
  routing popover; a failure also posts a short system notice before the
  fallback answer;
- usage: a `LlmCallRecord` with `call_kind = router`, the conversation's
  `channel_id`, and no `run_id`, `task_id`, or `agent_id`. Recording is
  best-effort, matching ADR-0029. Router tokens are excluded from every
  participant's branch and cache summaries, and reported as a separate Routing
  row in the context popover.

The router uses the conversation's fixed model, temperature 0.0, and one
attempt. A separate router model or provider is deferred. Routing is opt-in
because it costs one extra model call per unaddressed message.

This does not contradict the recommendation in
[REFERENCES](../REFERENCES.md) to defer automatic channel routing. That
recommendation targets implicit routing in group channels; M3 stays
deterministic-first, is opt-in per user-created conversation, keeps the Main
Assistant available only as a chosen participant, and leaves ADR-0022 channel
behavior unchanged.

### Default responder

The default responder is the fallback when the user does not address anyone.
It is a persisted participant (`conversation_settings.default_responder_id`).
At creation the desktop preselects `main` when the Main Assistant is among the
chosen participants, otherwise the first participant. The user can choose a
different participant in the form.

This replaces a name-based global lookup: the Main Assistant is the default
only when it is deliberately in the group. If the stored default responder is
missing or no longer a participant, the service uses the first participant and
posts a notice. A group without a loadable participant posts a notice and does
not call a model.

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
- Every other participant's settled reply is mirrored into a branch as a peer
  entry: `ContextEntryKind::Peer`, `ContextRole::User`, content
  `[AgentName] <reply>`. The group system prompt explains the convention so a
  model does not mistake a peer reply for the user or for its own words.
- `project_context` treats peer entries as history, never as the latest user
  message. This keeps the context ring and automatic compaction honest.
- System notices, routing notices, unknown-mention notices, and failure notices
  stay display-only and never enter a branch.
- The group system prompt is stable: agent prompt, platform context, the
  participant roster and default responder, and the peer-reply convention,
  followed by the active branch in sequence order. Provider cache breakpoints
  remain M5.

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
  as M2. Non-responders are not compacted.
- Group sends are not pre-blocked by the composer. The service checks each
  selected branch after routing, compacts it when automatic compaction is on,
  and skips a branch that is still full with a system notice naming the
  participant; the other responders continue. When every selected branch is
  full, the turn posts a notice and no model call runs. The context ring warns
  as participants approach the limit. Single-agent conversations keep M2's
  pre-send block. The desktop supplies per-agent usable-window and tool-schema
  token estimates through `ChannelTurnOptions`, because the service does not
  own the model metadata or the tool registry.
- Cache summaries: the conversation total sums every call in the channel,
  including router and compaction calls. Per-agent tabs use the latest run for
  that agent; the UI labels the conversation figure as the conversation total
  and per-agent figures as the latest run.

## Data model

M3 needs one additive migration, `0013_group_conversations.sql`:

- `channel_participants.position INTEGER NOT NULL DEFAULT 0` with an index, so
  participant order is stable and independent of `joined_at` ties; existing
  rows backfill deterministically.
- `conversation_settings.default_responder_id TEXT NULL`, referencing a
  participant; it is nullable for old rows and single conversations.
- `conversation_routes`: id, channel_id, message_id (unique), source
  (`mention`, `router`, `default`), selected agent ids JSON, unknown mentions
  JSON, confidence, reason, router_call_id, task ids JSON, created_at. It is
  the persisted routing audit and the reload-safe source for the Answered-by
  line. It cascades when the channel or message is deleted.
- No columns are added to `channels`, `tasks`, `assignments`, `context_entries`,
  or `llm_calls`. `ContextEntryKind::Peer` is a domain enum value stored in the
  existing `kind` text column; old rows and packages deserialize unchanged.
- `ConversationSettingsUpdate` gains `router_enabled` and
  `default_responder_id`. Mode is derived from the participant count and is not
  user-editable.
- Participant ordering is selection order. The participant limit is eight;
  single conversations keep exactly one participant. A group cannot be
  converted to a single conversation or the reverse in M3.
- An imported row whose stored mode and participant count disagree is
  normalized on read: one participant is served as single, zero participants
  posts a notice, and a single conversation ignores `router_enabled`.
- The route row is the turn record. `task_ids` links the created participant
  tasks; `run_group` is not reused and no `turn_id` is added to tasks. Turn
  grouping in the background panel is deferred.

## Client and desktop

- `Client::create_conversation` takes a `NewConversation` value: title, ordered
  agent ids, provider id, model, `router_enabled`, and
  `default_responder_id`. The single-agent form is the one-agent case.
- New client/Tauri operations: conversation summaries with participants,
  conversation context per participant, per-agent compact, clear
  (per-agent and all), route listing, and settings updates for the router and
  default responder.
- `post_message` keeps its M2 pre-send capacity check for single conversations.
  Group conversations skip the pre-check; the service performs the post-routing
  check described above. The background turn reports the routed agents and
  route source in `cowbot-channel-reply`.
- New bus events: `ConversationRouted` (route row) and
  `ConversationSettingsUpdated` (settings). They flow on the existing
  `cowbot-event` channel; no new Tauri event channel.
- Sidebar:
  - the New conversation form gains a group mode with a searchable participant
    list, an ordered participant summary, a default-responder select, and an
    Automatic routing toggle shown for two or more agents;
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
  - the composer's working note names the responders when available;
  - the empty state explains mentions, routing, and the default responder.

## Work breakdown

### M3.1 `cowbot-core`: domain

- `RoutingSource` (`mention`, `router`, `default`), `ConversationRoute`, and
  the route report value.
- `ConversationParticipantContext` and the extended `ConversationContext`.
- `ConversationSettingsUpdate::router_enabled` and
  `default_responder_id`; `ChannelParticipant.position`.
- `ContextEntryKind::Peer` and projection handling that treats peer entries as
  history.
- Participant-scoped mention resolution helper, participant-count helpers, and
  the eight-participant limit.
- Pure tests for mention scoping, route serde, peer projection, and context
  aggregation.

### M3.2 `cowbot-store`: schema and repositories

- Migration `0013_group_conversations.sql` as described above, with a
  deterministic position backfill.
- Participant position read/write and ordered listing.
- `conversation_routes` insert, list by channel, and delete with the channel or
  message.
- Settings read/write for `default_responder_id`.
- Per-agent latest-run usage query.
- Tests for migration, multi-participant creation, ordered participants, route
  round trip, settings round trip, and per-agent usage.

### M3.3 `cowbot-service`: group turns without the router

- Group creation with an ordered participant list, a fixed provider/model, and
  a default responder.
- Mention routing scoped to participants and unknown-mention reporting.
- Default-responder fallback.
- One user message fanning out to several scoped conversation turns.
- Per-participant branch append, assembly, peer mirroring, and settled-reply
  posting in responder order.
- Route-row persistence and `ConversationRouted`.
- Per-agent context, compaction, clear, and post-routing capacity rules.
- Tests for a mention turn, `@team`, unknown mentions, fallback, multiple
  responders, partial failure, mirroring, and unchanged single/direct/channel
  behavior.

### M3.4 `cowbot-service` router

- Router request, prompt, JSON parsing, validation, confidence floor, timeout,
  and fallback.
- `LlmCallKind::Router` usage recording and route-row linkage.
- A fake-provider test path that returns valid, invalid, empty, low-confidence,
  timed-out, and failing responses.
- Wiring behind `router_enabled`; M3.3 behavior is the fallback.

### M3.5 `cowbot-client` and Tauri

- `NewConversation` creation, conversation summaries, participant-aware context
  views, per-agent and clear-all context operations, route listing, and
  settings updates.
- Per-agent usable-window and tool-token estimates passed to the service for
  the post-routing capacity check.
- Extended reply event payload with responder ids and route source.
- Client and Tauri tests.

### M3.6 Desktop UI

- Group creation form with participant picker, ordering, default-responder
  select, and routing toggle.
- Participant header chips, per-agent context tabs, routing toggle, and
  default-responder label.
- Answered-by line, routing popover, working state, empty state, and group
  avatars.
- `pnpm run build` and the `src-tauri` fmt/test/clippy gates.

### M3.7 Package, CLI, and documentation

- `.cowpkg` round trip for participant positions, default responder, and
  optional route records; old packages import with defaults.
- CLI verification for `cowbot message post --wait` on a group conversation.
- ADR-0032 for group conversations and routing; PRD, ARCHITECTURE, ROADMAP,
  USER_GUIDE, PACKAGE_FORMAT, CLAUDE.md, and the docs index.
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

- Core: mention scoping, `@team` participants, route-source serde, peer
  projection, participant context aggregation, participant-count mode.
- Store: migration 0013, ordered participants, multi-participant creation,
  route round trip, default-responder settings, per-agent latest-run usage.
- Service: group creation, mention routing, unknown mentions, default
  fallback, router enabled/disabled, router selection and failures, multiple
  responders, deterministic order, partial failure, peer mirroring, per-agent
  compaction, clear, capacity skipping, and unchanged surfaces.
- Client: group operations and a group turn through `reply_to_mentions`.
- Package: group participants, positions, default responder, and routes
  round-trip; old packages import.
- Desktop: build gates plus a manual flow for creation, mention, routing,
  fallback, and per-agent context tabs.

## Acceptance criteria

- [ ] A user can create a group conversation with two to eight agents, a
      default responder, and one fixed provider/model.
- [ ] `@Agent` addresses that participant; `@team` addresses every participant;
      non-participant mentions are rejected with a clear notice.
- [ ] With routing off, an unaddressed message goes to the default responder.
- [ ] With routing on, the router selects one to three participants and its
      choice is visible; invalid, low-confidence, timed-out, or failed routing
      falls back to the default responder.
- [ ] Several participants answer one message, failures are isolated, and
      replies are posted in deterministic responder order.
- [ ] Every participant has an independent append-only branch containing the
      shared group timeline, with peer replies marked as peer entries and tool
      calls only in the owner's branch.
- [ ] The context popover shows one tab per participant and compaction and
      clear act on the selected branch.
- [ ] A full selected branch is skipped with a notice naming the participant
      while the others answer.
- [ ] Membership is fixed after creation and the UI explains how to create a
      new group with a different team.
- [ ] Single-agent conversations, direct conversations, and group channels
      keep their behavior.
- [ ] Workspace packages round-trip group conversations; older packages import.
- [ ] Workspace and desktop gates pass.

## Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Router misroutes or adds cost | Opt-in toggle default off, one call per unaddressed message, three-responder cap, mentions always win, confidence floor, visible route record, default fallback |
| Router output is malformed or names unknown agents | Strict JSON extraction, case-insensitive validation against participants, discard invalid names, confidence floor, fallback to the default responder |
| Group context grows quickly | Mirror only final replies, not tool loops; per-agent compaction and capacity checks; bounded router input |
| Multiple responders create confusing order | Concurrent turns, deterministic posting in responder order, explicit sender attribution, Answered-by line |
| One failing participant hides the others | Per-agent failure notices; successful tasks post independently; the report lists what answered |
| Approval-card flood under Ask or Plan | One policy for all responders; multiple cards are allowed; plan tasks are approved from Tasks |
| Default responder is missing or deleted | Fall back to the first participant with a notice; no model call when no participant is loadable |
| ADR-0030's no-migration claim is wrong | Migration 0013 is additive; ADR-0032 explicitly amends the consequence and records why position and route audit need storage |
| Route rows outlive or disappear with history | Route rows cascade with the channel and message; the plan documents the audit lifetime |
| Period between route persistence and reply failure | The route row records source and selected agents; per-task results and notices record what actually answered |

## Open at implementation

These are implementation details, not open product decisions:

- Exact router prompt wording, history window, input-token budget, and timeout
  handling.
- The precise `ConversationContext` shape and how much of the M2 top-level
  fields it keeps.
- Whether replies post as each task settles or after the turn settles in
  responder order; M3 targets deterministic responder order, with incremental
  posting a possible refinement if the scheduler exposes a settle hook.
- The participant chip layout at narrow window widths.
- Whether the Answered-by popover shows the router reason verbatim or a
  shortened form.
- Exact store method names and whether route insertion and task enqueue share a
  transaction boundary.

## Deferred decisions

- Participant membership editing, reordering, muting, and branch backfill.
- Per-participant provider or model selection.
- A separate router model/provider, retries, and router evaluation.
- Turn-level cancellation and turn grouping in the background panel.
- Agent-to-agent delegation, rounds, debate, or reply-to-agent triggering.
- Consolidated group plans under Plan mode.
- Shared notes and long-term memory (M4).
- Anthropic and Responses cache breakpoints (M5).
- User participants and remote sessions (Phase 2).
