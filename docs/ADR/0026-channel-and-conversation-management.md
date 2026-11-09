# ADR 0026: Channel and Conversation Management

- **Status**: Accepted
- **Date**: 2026-11-09
- **Related**: `docs/ADR/0024-agent-direct-conversations.md`, `docs/ADR/0025-database-migration-checksum-compatibility.md`, `docs/ARCHITECTURE.md` §3.2, `docs/USER_GUIDE.md`

## Context

0.1.5 shipped channels and agent direct conversations, but neither could be managed from the desktop. A workspace could accumulate two channels with the same name — the reference workspace had two channels called `1` — and the sidebar rendered them identically. There was no way to rename, delete, or pin a channel, and no way to clear a direct conversation's history. Creating a channel accepted any name, including one already in use.

The underlying store already supported the operations that matter: `messages.channel_id` cascades on channel deletion, `tasks.channel_id` becomes `NULL`, and the sidebar already sorts conversations by recent activity. What was missing was a deliberate rule set and a UI.

## Decision

### 1. Channel names are unique per workspace, case-insensitively

`ChannelService::create_channel` and `rename_channel` trim the name, reject an empty name, and reject a name that another channel in the workspace already uses under `COLLATE NOCASE`. The check lives in the service, not in a database unique index, because existing workspaces can already contain duplicates: a migration that added a unique index would fail on those rows and prevent the app from opening.

The existing duplicates stay visible until the user renames or deletes them, which the new management UI makes possible.

### 2. Channels carry a `pinned` flag

Migration `0004_channel_management.sql` adds `channels.pinned INTEGER NOT NULL DEFAULT 0`. `Channel` gains `pinned` with a serde default, so `.cowpkg` packages written before this change import with `pinned = false` and older readers ignore the field.

Pinning is per channel and independent of the section: pinned agent conversations sort first among agents, pinned group channels sort first among channels, and each group then falls back to recent activity. The sidebar shows a pin mark.

### 3. Rename and pin publish channel updates; deletion publishes a deletion

`ChannelUpdated(Channel)` and `ChannelDeleted { channel_id }` join `ChannelCreated` in `EventPayload`. The desktop refreshes the sidebar, previews, and selection from those events and from the command results. `ChannelService::ensure_agent_channel` now reconciles an existing direct conversation with the agent's current name and role and publishes `ChannelUpdated` when it changes, so a later agent rename keeps its conversation label in sync.

### 4. Group channels can be deleted; agent conversations cannot

Deleting a group channel removes its messages through the existing cascade and sets tasks' `channel_id` to `NULL`, preserving task history. The service refuses to delete a channel with `agent_id` set: an agent's direct conversation is part of the roster, and reconciling it on the next launch would recreate it empty anyway. Direct conversations instead offer **Clear history**, which deletes messages but keeps the channel, agent, and pin state.

All destructive actions are confirmed in a modal, not a `window.confirm`.

### 5. The desktop exposes management through a context menu

Right-clicking a sidebar row, pressing the context-menu key, or using the row's overflow button opens a menu:

- Agent conversation: pin/unpin, clear history.
- Group channel: rename, pin/unpin, delete.

Rename uses a modal with name and topic fields; service errors (for example, a duplicate name) appear in the dialog. Agent editing and renaming are recorded separately in ADR-0027, together with the main assistant.

## Alternatives considered

- **A database unique index on `(workspace_id, name)`**: stronger invariant, but it cannot be added to workspaces that already contain duplicates without a data migration that deletes or renames user data. Service validation plus the new UI is reversible and keeps existing rows untouched.
- **Soft delete or archive instead of hard delete**: preserves history but needs a visibility model, archived listings, and restore flows that the product does not have yet. Hard delete with confirmation matches the current scope.
- **A per-channel settings page instead of a context menu**: more room for future options, but slower for the common actions and a larger surface. The context menu plus modals covers rename, pin, delete, and clear.
- **Deleting direct conversations too**: reconciling conversations at startup would recreate them empty, so deletion would look like a clear-history action with an extra step and a missing contact until the next reconcile. Rejected.

## Consequences

- Two channels can no longer share a name through the app; the CLI inherits the same validation because it goes through `ChannelService`.
- Existing duplicates can be cleaned up by renaming or deleting one of them.
- Pinned state survives restarts and workspace packages.
- Deleting a channel is permanent and removes its messages; tasks remain, unlinked from the channel.
- Direct conversations cannot be deleted, only cleared, and the agent's identity is never removed by channel management.
- Coverage: `crates/cowbot-store/tests/persistence.rs` (pinned round-trip, rename, name lookup, clear, cascade), `crates/cowbot-service/tests/channel_management.rs` (duplicate names, empty names, DM delete refusal, reconciliation, pinning, cascade), and `crates/cowbot-client/src/local.rs` (client round-trip).
