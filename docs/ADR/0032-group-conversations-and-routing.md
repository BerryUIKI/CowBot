# ADR 0032: Group conversations and routing

- Status: Accepted
- Date: 2026-11-11
- Plan: [M3 — group conversations and routing](../plans/M3-group-conversations-and-routing.md)
- Related: [ADR-0019](0019-assignment-scheduler.md), [ADR-0022](0022-channel-mention-turns.md), [ADR-0024](0024-agent-direct-conversations.md), [ADR-0028](0028-main-assistant-and-workspace-actions.md), [ADR-0029](0029-token-usage-accounting.md), [ADR-0030](0030-conversation-sessions-and-context.md), [ADR-0031](0031-task-worktrees-and-background-cards.md)
- Supersedes: None. Amends ADR-0030's consequence that group mode would need no second migration.

## Context

ADR-0030 gave each user-created conversation one participant, a fixed
provider/model, and an append-only context branch per agent. Users now want a
small team in one conversation: address one agent, address several, or let the
conversation decide who should answer. Group channels already turn mentions into
answers (ADR-0022), but they have no persistent per-agent context, participants
never see each other's replies in their own history, and adding routing there
would change existing channel behavior. A user-created conversation has a
private timeline, a fixed model, and per-agent branches, so it is the right
container for a group.

The design must answer four questions without disturbing single conversations,
direct conversations, or group channels: how membership is fixed, how a message
selects responders, how each participant's context stays honest, and how a turn
is audited and shown.

## Decision

### 1. A group is a conversation with two to eight fixed participants

Groups reuse `ChannelKind::Conversation` with `mode = group` and an ordered
participant roster; no new channel kind is introduced. Membership is fixed at
creation and cannot be converted to or from a single conversation. Participant
order is selection order, persisted as `channel_participants.position`, and is
used for display, mention expansion, and the default responder. The participant
count is authoritative when reading: one participant is served as a single
conversation, two or more as a group, and zero participants posts a notice.

### 2. Explicit mentions win; the router is opt-in and the default responder is the fallback

Mentions in a group resolve only against its participants. `@team`, `@all`,
`@everyone`, and `@here` expand to every participant in order. A name outside
the roster is rejected with one system notice listing the participants; it never
adds an agent. Mixed messages answer valid mentions and report the rejected
names.

When nobody is mentioned, routing order is:

1. `router_enabled` is on: one completion on the conversation's fixed model
   selects zero to three participant names as JSON, with a 20-second timeout and
   no retries. Invalid names are discarded; valid names are deduplicated,
   keep the router's order, and are capped at three. Any failure falls back.
2. The stored `conversation_settings.default_responder` answers. At creation the
   desktop preselects the participant named `main` (the built-in Main
   Assistant) when present, otherwise the first participant; users may choose
   another. A stored responder that is no longer a participant resolves to
   `main`, then to the first participant, with a notice.
3. With no stored responder, the message stays silent and only a route notice
   and turn row record it. The service never invents an agent for an existing
   conversation.

Routing is opt-in because it costs one extra model call per unaddressed
message. It applies to user-created conversations only. The recommendation in
[REFERENCES](../REFERENCES.md) §3.7 to defer automatic routing in group channels
stands; channel behavior is unchanged.

### 3. One user message fans out to independent roots and replies post as they settle

Every responder gets its own task in the conversation, enqueued under one scoped
scheduler whose concurrency is sized to the selection (capped at eight). Tasks
run independently and a failure never silences the others. Replies are posted
through an observed drain (`run_until_idle_observed`) as each task settles, so
the transcript shows honest settle order rather than a pretend synchronous
order. One `conversation_turns` row records the message, route source, selected
agents, unknown mentions, router reason and call id, and created task ids;
`ConversationRouted` announces it. Turn rows are device-local and cascade with
their message or channel.

### 4. Context stays per participant; peers are authored history

The user message is appended to every participant's branch before assembly,
including non-responders, so every participant stays current for later turns.
A responder's assistant and tool messages are recorded in its own branch by the
existing conversation recorder. Every other participant's settled reply is
mirrored into a branch as a `ContextRole::User` entry with
`author_agent_id = Some(speaker)` and content `[AgentName] <reply>`. Projection
treats an authored entry as history, never as the latest user message. The group
system prompt carries the roster, the default responder, and the peer-reply
convention, and is stable across turns.

Each participant has its own context meter, compaction, and clear action. The
conversation total includes router and compaction calls; a participant's latest
run excludes router calls. Auto-compaction runs per selected branch before its
turn when a caller-supplied usable window crosses the threshold. Group sends are
not pre-blocked: after routing, a branch that is still full is skipped with a
notice while the other responders continue, and a turn whose branches are all
full posts a notice without a model call. Single conversations keep M2's
pre-send block.

### 5. Storage is one additive migration; packages carry the new fields

Migration `0013_group_conversations.sql` adds `channel_participants.position`
(backfilled deterministically in creation order), `conversation_settings
.default_responder`, `context_entries.author_agent_id`, and the
`conversation_turns` table. This replaces ADR-0030's expectation that group mode
would fit the existing schema: position, a persisted default, peer attribution,
and the routing audit each need storage.

`.cowpkg` packages export participant positions, the default responder, and
authored context entries. Packages written before M3 have no positions; import
re-sequences a channel's all-zero participants in file order, which is the
exporting machine's creation order. Turn rows stay device-local and never enter
a package; route chips may be absent after import without corrupting the
conversation.

### 6. Client, desktop, and CLI surfaces

`cowbot-client` creates groups from a `NewConversation` value, lists
conversation summaries, participants, and turns, and exposes per-participant
context operations. Tauri mirrors them and passes per-participant usable windows
and tool-schema tokens for the post-routing check. The desktop adds an ordered
participant picker, participant chips, per-participant context tabs, an
Answered-by line with routing details, and a keyboard-accessible mention
autocomplete. `cowbot message post --wait` prints the route and per-agent
outcome; no group-management commands are added to the CLI.

## Alternatives considered

- **Introduce a new channel kind for groups.** Rejected: it would duplicate
  conversation settings, participants, and context branches for no gain, and
  would split client and desktop handling.
- **Put groups in group channels.** Rejected: ADR-0022's channel turns are
  mention-only by design, channels have no per-agent context, and routing there
  would change existing behavior the REFERENCES recommendation deliberately
  defers.
- **Keep one shared context branch and render the transcript.** Rejected: it
  loses per-agent attribution, tool history, and independent compaction, which
  is the point of per-participant branches.
- **Run the router on every unaddressed message.** Rejected: it adds a model call
  and a surprise decision to every message; routing is opt-in per conversation.
- **Require router confidence or a second model.** Deferred: strict roster
  validation plus the default-responder fallback already prevents a bad
  selection from derailing a turn.
- **Block group sends when the composer estimates a full context.** Rejected:
  the composer cannot know which participants will be selected, and other
  participants may have room. The service checks and skips per branch after
  routing.
- **Persist route chips as timeline messages.** Rejected: notices would clutter
  every group transcript. The turn row plus the reply event carry the same facts
  and the desktop renders them as an Answered-by line.

## Consequences

- One additive migration; older databases backfill positions in creation order.
- A group turn can create up to eight tasks, each with its own run and LLM calls
  attributed to the conversation channel.
- Replies appear in settle order and are mirrored into every other branch,
  increasing each branch's context until compaction summarizes them.
- Router decisions cost one extra call when enabled; their tokens are billed to
  the conversation total and excluded from participant runs and cache figures.
- Turn rows grow by one per group user message and cascade with the message or
  channel; they are not exported.
- Group sends are not pre-blocked; a full branch is skipped with a notice,
  which is visible but less immediate than M2's composer block.
- Single conversations, direct conversations, and group channels keep their M2
  behavior; the migration and client changes are additive.
