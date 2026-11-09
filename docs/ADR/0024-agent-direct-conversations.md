# ADR 0024: Agent Direct Conversations

- **Status**: Accepted
- **Date**: 2026-11-09
- **Related**: `docs/ADR/0022-channel-mention-turns.md`, `docs/ADR/0023-channel-turn-scheduler-scope.md`, `docs/ADR/0009-desktop-ipc-and-state.md`, `docs/ARCHITECTURE.md` §3.2, `docs/USER_GUIDE.md`

## Context

ADR-0022 made `@mention` messages in a channel produce an agent answer. That gives every agent a reachable surface, but only through a shared room: to talk to one agent a user has to create a channel, type the agent's name, and remember that messages without a mention stay unanswered. The desktop shell was built around the same model — Channels, Tasks, Approvals, and Settings were peer views, and the sidebar listed channels rather than people.

Users expect the opposite shape from a desktop assistant: a contact list of agents, one conversation per agent, and a message composer that always reaches that agent. Grok Bot's desktop layout makes this explicit, and the reference docs already treat messaging-app UX as the target (`docs/REFERENCES.md` §3.7).

The domain had no way to express "this conversation belongs to this agent". `Channel` carried only a name, topic, and creation time, and nothing in the runtime distinguished a group room from a one-to-one thread.

## Decision

### 1. A direct conversation is a channel with an owner

`Channel` gains `agent_id: Option<AgentId>` (serde default `null`). A channel with an owner is that agent's direct conversation; a channel without one is a group room. Reusing `Channel` instead of adding a `DirectMessage` entity keeps one message store, one event stream, one service, and one desktop timeline, so mentions, assignments, approvals, and history work in a direct conversation without a parallel path.

The store applies this with migration `0003_agent_channels.sql`:

- `ALTER TABLE channels ADD COLUMN agent_id TEXT`
- a partial unique index on `(workspace_id, agent_id)` where `agent_id` is not null

The index is what makes "one conversation per agent" a database invariant rather than a convention.

### 2. Conversations are provisioned idempotently

`ChannelService::ensure_agent_channel` returns the agent's existing channel or creates one with `Channel::direct(agent, role)` and publishes `ChannelCreated`. `ensure_agent_channels` applies it to every workspace agent.

`LocalClient::ensure_agent_channels` runs the reconciliation and returns the channel list; the Tauri command of the same name exposes it. The desktop calls it at startup, after instantiating agents from a template, and after onboarding adds a team. Old workspaces therefore gain conversations for their existing agents without a migration script, and imported `.cowpkg` workspaces gain them on first launch.

### 3. A direct conversation has exactly one addressee

`ChannelAgentService::reply` branches on `channel.agent_id`:

- **Direct**: the channel's agent answers every message. Mentions are plain text; `@team` does not fan out to the workspace, and unknown mentions do not produce notices. A missing agent produces one system notice.
- **Group**: unchanged from ADR-0022 — `resolve_mentions`, unknown-mention notices, `@team` fan-out, and silence for messages without a mention.

The turn task's description says it is a direct reply rather than a channel mention, so the model is not told to act as a teammate in a room it cannot see. Everything else — the assignment scheduler, approval gate, run and tool-call recording, and the agent-attributed answer — is shared with group turns.

### 4. The desktop presents agents first

The shell becomes conversation-first:

- The sidebar always starts with the agent contact list, each row showing the agent's avatar, name, latest-message preview, and time, ordered by recent activity.
- Group channels sit below as a secondary section with their own search and creation flow.
- The main pane is a chat thread: user messages in right-aligned light bubbles, agent messages in left-aligned dark bubbles, system notices centered, a typing bubble while a turn runs, and a floating composer.
- Tasks, Approvals, and Settings move to the sidebar footer. The Tasks view keeps its list in the sidebar and its detail in the main pane.
- Search filters agents, channels, and tasks.

The visual language stays CowBot's (ink and brass, serif headings); the structure follows the reference, not its palette.

## Alternatives considered

- **A separate direct-message entity and table**: cleaner domain purity, but two message stores, two event types, and a second reply path would have to duplicate ADR-0022's scheduler integration and the desktop timeline. Rejected.
- **Frontend-only agent rows that route to a shared channel with an implicit `@mention`**: no schema change, but the stored message would carry text the user did not type, the channel would mix conversations, and there would be no durable link between an agent and its thread. Rejected.
- **Per-agent conversations created only on demand from the UI**: avoids touching workspaces that never use them, but leaves the contact list empty until a click and makes startup reconciliation order-dependent. Provisioning at startup is small and idempotent.
- **Hiding group channels entirely**: the reference has no channels, but CowBot's channel model is the collaboration primitive (ADR-0022, `docs/REFERENCES.md` §2) and removing it would strand existing workspaces. Group rooms stay, below the agents.

## Consequences

- Every agent has exactly one direct conversation; users can message an agent without learning mention syntax.
- The last-addressed rule is visible: direct conversations never fan out, group rooms remain explicit.
- `.cowpkg` packages carry `agent_id` in `channels.jsonl`; `#[serde(default)]` keeps packages written before this change importable.
- Existing workspaces are upgraded in place by reconciliation; no data migration is required.
- The sidebar's agent list is the first place a new agent appears, so template instantiation must trigger reconciliation to avoid an empty contact row.
- Deferred: per-conversation renaming or deletion of the direct channel, agent deletion, unread counts, attachments, streaming answers, and multi-user direct messages.
- Coverage: `crates/cowbot-core/src/channel.rs` serde tests, `crates/cowbot-store/tests/persistence.rs` (round-trip and unique index), and `crates/cowbot-service/tests/channel_agents.rs` (direct answer without a mention, mentions ignored in direct conversations, `ensure_agent_channel` idempotency).
