# ADR 0064: Desktop background residency

- Status: Accepted
- Date: 2026-11-27
- Discussion: PR for `feature/tray-residency`
- Related: [ADR-0018](0018-desktop-auto-updates.md), [ADR-0019](0019-assignment-scheduler.md), [ADR-0022](0022-channel-mention-turns.md), [ADR-0051](0051-database-lifecycle-safeguards.md), [ADR-0053](0053-desktop-server-mode.md), [ADR-0060](0060-im-approvals.md)

## Context

Agent turns and crew runs execute inside the desktop process. `post_message`
spawns `LocalClient::reply_to_mentions` on the Tauri runtime, and `run_task`
spawns the crew the same way; both finish by writing to SQLite. The window was
the process: closing it destroyed the last webview, Tauri exited, and any turn
still in flight ended without a word. A user who wanted CowBot to keep working
had to leave the window on the taskbar.

That is the wrong shape for what CowBot is. A group channel or an IM-style
workspace is something you send a message to and walk away from; a crew run is
minutes of work. Neither should depend on a window being visible, and the
close button is not a natural "stop everything" gesture.

Residency also interacts with two existing decisions that constrain the design:

- **`config.json` is the desktop's only settings store** (ADR-0016, ADR-0053),
  and the startup screen already reads it before `AppState` exists.
- **The window is the only place execution state is shown.** Approvals are cards
  in the right inspector (ADR-0060); a background turn needs a human there.

## Decision

The desktop stays resident. Closing the main window hides it; quitting is a
separate, explicit action.

### 1. The window's close button hides

`WindowEvent::CloseRequested` on the `main` window is intercepted and
prevented, and the window is hidden. The process, the SQLite connections, and
every spawned turn keep running.

A `close_to_tray` boolean in `config.json` controls this, and it defaults to
**on** — including for configuration files written before the tray existed.
Turning it off restores the pre-0.1.18 behaviour: close means quit.

The one exception is a window that never finished opening. If `AppState` was
never installed — the database failed to open, or the startup screen is waiting
for an answer about an upgrade — closing quits instead of hiding. Hiding would
put a failure the user still has to answer somewhere they cannot see it.

### 2. A tray icon is the way back

`TrayIconBuilder` with the app's own window icon: left click reveals the
window, right click opens a two-item menu (**Open CowBot**, **Quit CowBot**).
The same reveal path serves the single-instance callback (a second launch is
often the only thing a user knows to try) and, on macOS, `RunEvent::Reopen`
from the Dock icon.

The menu labels follow the interface language, but the shell has no access to
the locale. The window pushes them through `set_tray_labels` whenever the
language changes; English is what the tray shows before the window has
reported a language, which covers a startup that never reaches the frontend.
Only the item text is updated — on Linux a tray menu cannot be replaced once it
is set.

**No live status in the menu.** A "2 tasks running" line was considered and
rejected: the count would come from the frontend, and a hidden window's timers
are throttled by the webview (and can be suspended outright on some platforms),
so the number would drift into being wrong. A stale count is worse than none.

### 3. Quitting is explicit and refuses to lose work

- **Tray → Quit CowBot** checks the Rust-side running sets — the same
  `AppState.running` and `AppState.replies` that gate workspace switching
  (ADR-0017). If either is non-empty it does not quit: it reveals the window
  and emits `cowbot-tray-quit-blocked`, and the window explains why. This answer
  is exact even while the window is hidden, because it does not come from the
  frontend.
- **The account menu** gains **Quit CowBot**. It warns with a confirmation when
  a run or a reply is in flight, then quits anyway. This is the deliberate exit,
  and the one that works when the tray refused.

The window's × never quits while `close_to_tray` is on, so there is no gesture
that both closes the window and ends the process by accident.

### 4. Coming back refreshes the summary

Events are delivered whether or not the window is visible, so the timeline is
already current. On focus, the window re-reads pending approvals and the
background overview anyway — a delivery can be dropped, and a window that was
hidden for an hour should not show a stale approval count. This is a targeted
refresh, not the full startup reload: the latter resets the selected task.

## Alternatives considered

- **Minimise to the taskbar instead of hiding.** That is what already
  happened; it does not survive a user closing windows to clear their desktop,
  and it gives no way back once the window is closed.
- **Keep the window alive but destroy the webview when hidden.** Would release
  memory, but re-creating the webview on reveal loses the timeline's scroll
  position and in-flight stream state, and the shell would need to replay
  events it never buffered.
- **Default `close_to_tray` to off and let users opt in.** Safer against
  surprise, and rejected: the behaviour it preserves — silently killing running
  work — is the bug. A user who closes the window and later finds CowBot still
  in the tray learns something true about the app; a user whose crew run
  vanished learns nothing.
- **Quit from the tray unconditionally, with a confirmation dialog.** The
  dialog is a native one the tray can raise while the window is hidden, which is
  the worst moment to ask. Refusing and explaining in the window keeps the
  decision where the state is shown.
- **System notifications for a blocked quit or a finished run.** Out of scope
  here; it needs a notification permission surface on three platforms and a
  policy for when not to notify. Tracked as follow-up.
- **Start at login.** Also out of scope. Residency and autostart are separate
  decisions, and autostart has its own questions (default off, first launch
  hidden, updater interaction).

## Consequences

- **Behaviour change on upgrade.** Closing the window no longer quits. This must
  be in the release notes in plain words, and Settings → General carries the
  switch that restores the old behaviour.
- **A resident process.** CowBot now holds a window-less process, an SQLite
  connection, and possibly an open network stream until the user quits. On
  Windows and Linux the tray icon makes it visible; on GNOME without an
  AppIndicator extension the icon may not appear at all, and the window is then
  reachable only through the account menu's Quit — i.e. not at all once hidden.
  This is accepted as a platform limitation rather than worked around.
- **Linux ignores `show_menu_on_left_click(false)`.** Left and right click both
  open the menu there; the first item still reveals the window.
- **A refused quit reveals a window the user may not be looking at.** The
  notice appears in the app; a user who walked away sees only a window. That is
  the same gap system notifications would close.
- **`close_to_tray` is device-local**, beside the locale and the bot tag, and
  never enters a `.cowpkg` (ADR-0017).
- **The tray does not exist before setup runs**, so a configuration that cannot
  be read leaves the app with no tray; `close_to_tray` then reads as its default
  and the window hides. The alternative — refusing to hide when the file is
  unreadable — would make a corrupt config unquittable.
- **Server mode is unaffected.** In server mode a turn runs on the server
  (ADR-0055) or not at all, so residency buys the window's continued presence
  rather than preserved execution. Nothing about it changes.
- **Follow-up work**: system notifications for approvals and finished runs while
  hidden; a launch-at-login switch; and, if the tray proves undiscoverable, a
  first-run hint pointing at it.
