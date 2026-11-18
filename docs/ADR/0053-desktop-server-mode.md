# 0053: The desktop's server mode: saved sessions, reconnect, and mode gating

- Status: Accepted
- Date: 2026-11-18
- Discussion: Phase 2 milestone 2.3 in [the roadmap](../ROADMAP.md). Builds on [ADR-0045](0045-collaboration-api-and-remote-client.md) and [ADR-0047](0047-hosting-the-workspace-data-surfaces.md).
- Supersedes: None

## Context

ADR-0045 gave `cowbot-client` a `RemoteClient` over HTTP and a WebSocket, and
the desktop's `AppState.client` is a `Arc<dyn Client>`, so the shell was ready
to be pointed at a server. It was never pointed anywhere but a local SQLite
file: `finish` always built a `LocalClient`, `config.json` had no field for a
server, Settings had no section for one, and the socket gave up on its first
connect failure. A team could deploy the server and sign in with `curl`, but a
desktop user could not.

Everything else in 2.3 was already served: the collaboration surface, the
workspace data surfaces, template and package transfer, and the two read-only
context previews. What remained was the client-side half of the milestone.

## Decision

### 1. Which backend is a property of the device

`config.json` gains `backend` (`local` or `server`) and a `server` block holding
the origin, the account email, the bound workspace id and name, and the session
token. A missing `backend` means local, so every configuration written before
this decision keeps its offline behaviour, and `active_server` requires all four
coordinates: a damaged file starts the local workspace rather than starting
nothing.

The token lives beside model API keys in the same owner-only file and never
crosses the Tauri command boundary back to the frontend. `get_server_settings`
reports the mode, origin, account, and workspace; it does not report the token.

Why the device and not the workspace: a workspace row cannot say which database
*this* installation opens, and a user may hold a local workspace and a server
workspace at the same time. `active_workspace_database` already works this way
for local databases.

### 2. The transport is chosen once, at startup

`finish` reads the saved backend and either opens SQLite or resumes the saved
session, and every server action in Settings restarts the app. Swapping the
client under running turns would mean deciding what happens to a reply that is
streaming, a task that is running, and an event subscription that is open —
a much larger question than milestone 2.3 asks. The restart also makes the
state after a switch unambiguous: one client, one subscription, no leftovers.
`ensure_no_running_tasks` refuses a switch while a turn is in flight.

### 3. A restart resumes the session instead of asking again

`RemoteClient::resume(base, token, workspace)` calls `/api/v1/me` with the saved
bearer token and rebuilds the client from the answer, so restarting CowBot in
server mode does not need the password. A token the server has forgotten is a
`CowError::Config` naming the expired session, which the settings view shows as
"sign in again".

### 4. The live stream reconnects and says when it did

`subscribe` now retries with exponential backoff (2 seconds up to 60) instead of
ending at the first failure, and a socket that drops reconnects to the same
workspace. The first successful connect after a drop publishes
`EventPayload::StreamResumed`; the window reloads channels, agents, tasks,
approvals, the open timeline, and the background overview, because events
published while the socket was down were never delivered.

A token the server rejects (`401` from `/api/v1/me`) stops the loop with a
`SystemNotice` telling the user to sign in again, so a revoked session does not
look like a server that is down. A `5xx` or a timeout keeps retrying.

### 5. Sign-in probes before it saves

`probe_server` signs in and lists the account's workspaces without writing
anything, so the form can offer a workspace picker. `connect_server` signs in
again with the chosen workspace, writes the connection, and restarts. A refused
password therefore leaves the open workspace untouched.

`switch_server_workspace` resumes the saved token against a different workspace
rather than asking for the password again. `use_local_backend` returns to the
local workspace and keeps the saved session; `disconnect_server` forgets it.

### 6. Operations that need this machine hide themselves

ADR-0045 made `RemoteClient` answer the local-only `Client` members with an
error. The desktop now uses the same boundary it already had for that:
`local_client: Option<Arc<LocalClient>>` is `None` in server mode.

`get_app_info` carries the mode to the window, and the controls that cannot work
remotely are hidden rather than failing on click: running a crew from task
detail, the project worktree selector, registering a repository, local
workspace creation and switching, workspace package export and import,
compaction and auto-compaction, and the profile editor.

Two commands needed more than hiding, because the window calls them
unconditionally:

- `ensure_agent_channels` provisions direct conversations in local mode and
  lists the server's channels in server mode.
- `list_task_overview` needs a run's assignments and its newest tool call. The
  local runtime answers both from one query per run group; server mode combines
  the assignments route across the group's tasks and walks their runs for the
  newest tool call. `get_task_worktree` answers "none" instead of failing the
  whole task panel.

### 7. Agent turns still run on the member's machine

A turn executes in the desktop's process with its model provider, so a server
workspace member without that provider still cannot make an agent answer. That
is milestone 2.3's stated limit, not a bug in this decision: the client is
transport-neutral, and moving execution to the server is a separate milestone.

## Alternatives considered

**A per-workspace server field inside the database.** The database is the
workspace; it cannot record which backend opens it. It would also duplicate what
the rest of the device-local configuration already does.

**Swap the transport in place without restarting.** Cheaper for the user by a
second, and it forces answers for in-flight turns, open subscriptions, and
cached lists that would each need their own rule. Restarting gives one clear
state and matches how local workspace switching already behaves.

**Ask for the password on every launch.** Simple and it would have avoided
storing a token, but the server's sessions last two weeks by design (ADR-0043),
and a desktop that forgets them on exit throws that away.

**Treat a failed socket as fatal.** The first version did exactly that, and the
result was a screen that looked live while it was silently stale. A reconnect
plus an explicit `StreamResumed` is the honest version, because it tells the
window what it missed instead of pretending nothing happened.

**Keep the local-only buttons and let them fail.** A button that raises "needs
the local runtime" is worse than an absent one: it invites a click to learn
something Settings → Server already says.

## Consequences

- The desktop has a real second transport. Settings → Server connects, tests,
  switches workspaces, and returns to local mode, in all thirteen interface
  languages.
- `AppInfo` gains `backend`. It is a compatibly additive field, and the frontend
  defaults to local until the first load answers.
- `EventPayload` gains `StreamResumed`. The server's stream filter admits no
  payload for it, because only a client's own socket publishes it.
- Restarting after a server switch is a visible pause. That is the cost of
  keeping the state unambiguous.
- The remaining server-mode gaps are execution (agent turns, crews, and
  compaction still need the local runtime), profile editing, and live-turn
  cancellation. They are listed in [LIMITATIONS](../LIMITATIONS.md).
- Follow-up work: moving turn execution to the server, a server-side profile
  edit, and streaming a live turn over a remote transport.
