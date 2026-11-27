# ADR 0065: Desktop notifications while the window is out of sight

- Status: Accepted
- Date: 2026-11-27
- Discussion: PR for `feature/desktop-notifications`
- Related: [ADR-0060](0060-im-approvals.md), [ADR-0064](0064-desktop-background-residency.md)

## Context

ADR-0064 made the shell resident: closing the window hides it to a tray icon
while agent turns and crew runs keep executing. That removed the last thing that
told a user their work had settled, because there was none — the window itself
was the notification.

Two states are actively harmful to miss:

- **An approval is waiting.** `ApprovalService::request` publishes
  `ApprovalRequested` and blocks the run until a human decides, with a
  five-minute default timeout. A user who closed the window does not know there
  is a card to answer, and comes back to an expired approval and a stalled run.
- **A run settled.** A crew that finished, or failed, leaves nothing on screen.

`docs/LIMITATIONS.md` and `CLAUDE.md` both recorded "no system notifications
while hidden" as a known gap. This closes it.

## Decision

Show a system notification for two event kinds, and only while the window is
out of sight.

### 1. What notifies

| Event | Notification |
| --- | --- |
| `EventPayload::ApprovalRequested` | "CowBot needs your approval" / `{tool} · {reason}` |
| `EventPayload::TaskCompleted` | "Task finished" / the task's title |
| `EventPayload::TaskFailed` | "Task failed" / `{title}: {error}` |

`TaskCompleted` and `TaskFailed` are published per assignment
(`AssignmentService::complete` / `fail`), so a crew task settles more than once.
The subscriber keeps a `HashSet<TaskId>` and announces each task once.

Channel replies are deliberately **not** notified. A user posting in several
channels receives a reply for work they started moments ago in a conversation
they can already see; the notification would report an action to the person who
just took it. It also does not scale — one message that mentions six agents
produces one settle event with six reply counts.

### 2. When it fires

A notification is dropped unless **both** hold:

- the device has notifications switched on (`notifications_enabled`), and
- the main window is hidden to the tray, or visible but minimised.

Both are read at the moment of the event, so the Settings switch takes effect
without a restart and the shell keeps no stale copy of it.

An unreadable window state, and a missing `AppState`, both count as *shown*:
so the app stays quiet. This is the deliberate direction of failure. A
notification the user is already looking at is noise, and one that arrives
because the state could not be read is worse than one never sent.

### 3. Copy is English

The interface locale lives in the frontend (`catalog.ts`), and the shell has
none of it. The tray menu solved this by having the window push its labels
(ADR-0064), but that path does not fit here: a notification can fire at 3 a.m.
with no window alive to report a language, which is exactly the case the
feature exists for.

So the Rust side keeps English copy, and the Settings switch's own labels go
through the normal thirteen catalogues. Recorded as a limitation.

### 4. A click returns to the window

`Notification::on_action` handles `tap` by calling `shell::reveal`, the same
function the tray icon, the single-instance callback, and the macOS Dock icon
use. One reveal path means one set of bugs.

Because Windows only reports a click while the toast is still on screen — not
once it has moved to the Action Center — every notification's title and body
carry the whole story. Clicking is a convenience, not the source of the
information.

## Alternatives considered

- **Notify on channel replies too.** Rejected: it reports the user's own
  action back to them and multiplies with the number of channels in use.
- **Notify always, hidden or not.** Rejected: a toast about the run you are
  watching is pure noise, and the rule is trivial to get right.
- **Push localised copy from the frontend, like the tray labels.** Rejected for
  the reason in §3 — the frontend is the thing that is not running.
- **Use the JS plugin (`@tauri-apps/plugin-notification`) from the frontend.**
  Rejected: the decision to notify belongs where the state is (Rust owns the
  running sets and the events), and going through the frontend would mean
  shipping the event to a webview that may be throttled or suspended, then
  asking it back for a decision.
- **Add action buttons (Approve / Reject) to the toast.** A real improvement,
  and out of scope: deciding an approval from a notification needs the approval
  id, a re-entrancy story for an expired card, and a platform-by-platform
  action surface. The toast opens the app instead.
- **Aggregate notifications across tasks.** Not now — the dedupe per task is
  what makes a crew run one toast instead of four; coalescing different tasks
  would hide which one finished.

## Consequences

- **The default is on.** A resident app that finishes work silently is the
  problem this fixes; a user who dislikes toasts turns them off in Settings →
  General, beside the identical switch for close-to-tray.
- **Notifications never appear in server mode for someone else's work.** Only
  events on this client's stream are notified, which in server mode means the
  workspace this device opened.
- **A notification can outlive the event.** A toast shown for a run that a
  second device already cancelled stays on screen until dismissed; nothing
  retracts it.
- **`notifications_enabled` is device-local** and never enters a `.cowpkg`.
- **New dependency**: `tauri-plugin-notification` 2.x. It adds a plugin to the
  Tauri builder and its own crate tree to `Cargo.lock`; no ACL entry is needed
  because nothing is invoked from the frontend.
- **Follow-up work**: action buttons on the approval toast, and coalescing
  notifications per workspace if several tasks are typical.
