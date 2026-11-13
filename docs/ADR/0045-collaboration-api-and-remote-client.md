# ADR 0045: The collaboration API and `RemoteClient`

- Status: Accepted
- Date: 2026-11-14
- Discussion: Phase 2 milestones 2.2 (remaining) and 2.3 in [the roadmap](../ROADMAP.md).
- Supersedes: None. Builds on [ADR-0043](0043-self-hosted-server-auth.md) (accounts) and [ADR-0044](0044-workspace-membership-and-roles.md) (roles).

## Context

ADR-0044 shipped workspaces, roles, and invitations. What a team actually does
— post in a channel, create a task, decide an approval, read the audit log —
had no HTTP route, and the desktop had no way to reach a server at all. The
`Client` trait (ADR-0007) was written for this: it is object-safe, and until now
it had exactly one implementation and no trait object anywhere.

## Decision

### 1. Routes cover the collaboration surface, not the whole trait

The server serves what a shared workspace needs: channels and messages, tasks
and their assignments, runs, tool calls, approvals, permission rules, the
workspace policy, agents, audit, and API tokens. Every route resolves the
channel or task first, then the workspace that owns it, then the caller's role
— never the workspace in the path, which a caller could forge.

| Area | Routes | Role |
| --- | --- | --- |
| Channels | `GET/POST /workspaces/{w}/channels`, `GET/PATCH/DELETE /channels/{c}` | read: any member; create/rename/pin: write; delete/clear: administer |
| Messages | `GET/POST /channels/{c}/messages`, `POST .../clear` | read/post: write |
| Tasks | `GET/POST /workspaces/{w}/tasks`, `GET /tasks/{t}`, `.../cancel`, `.../archive`, `.../approve-plan` | write |
| History | `/tasks/{t}/assignments`, `/tasks/{t}/runs`, `/runs/{r}`, `/runs/{r}/tool-calls`, `/assignments/{a}/events` | any member |
| Approvals | `GET /workspaces/{w}/approvals`, `POST /approvals/{a}/decide` | write |
| Rules and policy | `/workspaces/{w}/permission-rules`, `/workspaces/{w}/policy` | administer |
| Agents | `GET/PATCH /workspaces/{w}/agents` | read: any member; edit: administer |
| Audit | `GET /workspaces/{w}/audit` | administer |
| Tokens | `GET/POST /workspaces/{w}/tokens`, `DELETE .../{id}` | administer |
| Stream | `GET /workspaces/{w}/stream` (WebSocket) | any member |

The sender of a message is always the authenticated user. A `sender` field in
the request body is ignored, so a caller cannot post as someone else.

A malformed body is parsed *after* the role check on routes whose body carries
an enum, so an unprivileged caller learns nothing about which values are valid.

### 2. Everything that needs the local machine says so

The trait has members that cannot mean anything over HTTP: preparing a git
worktree on disk, running a crew in-process, compacting with a local tokenizer,
the per-process session decisions. `RemoteClient` implements them as an error
that names server mode, rather than a silent no-op or a plausible-looking lie.
The desktop hides the affected controls behind `local_client: Option<…>`, and a
command that needs the runtime answers with one sentence instead of failing in a
confusing place.

Read-only surfaces that simply have no server route yet (memory, prompt
snapshots, context previews, templates) return empty results rather than errors,
so a screen renders empty instead of breaking. Those are the next milestone's
work, not a correctness claim.

### 3. The stream is filtered per workspace

The bus is process-wide. Most payloads name a channel or a task but not a
workspace, so the socket loads the workspace's channel and task ids at
subscribe time and drops anything it cannot place. New channels are the one gap
— the client already has the create response — and a channel created by another
user appears on the next reconnect. A payload that names neither a channel nor a
task is dropped; in a multi-tenant deployment, omitting an unplaceable event is
safer than leaking it.

A subscriber that falls behind the bus receives `{"type":"Resync"}`, which tells
the client to reload rather than silently missing events — the same recovery the
local desktop uses.

The socket authenticates with the same bearer token as a request, passed as a
`token` query parameter because a browser WebSocket handshake cannot set an
`Authorization` header.

### 4. API tokens act as their owner

A token is bound to one user and one workspace, has one scope (`workspace`), is
256 random bits stored only as a hash, and is revoked by stamping `revoked_at`
rather than deleting the row, so an audit can still say which token acted.
Only an Owner or Admin mints one; the token then acts *as its owner*, so every
request re-checks the owner's current role — demoting an admin immediately
narrows what their tokens can do.

Revoking is the owner's own action, or an Owner's for someone else's token.

### 5. `RemoteClient` is the desktop's second transport

`RemoteClient::sign_in(base, email, password, workspace)` signs in, picks a
workspace, and implements `Client` over `reqwest` and `tokio-tungstenite`. The
API error body's `code` maps back to `CowError`, so a caller handles the same
variants in both modes.

The desktop's `AppState.client` is now `Arc<dyn Client>`, with
`local_client: Option<Arc<LocalClient>>` beside it for the operations that need
the machine. The change is small because only a handful of commands bound the
concrete type.

## Alternatives considered

**Serve the entire trait, including worktrees and crew execution.** A server
cannot prepare a worktree in a folder the user has not registered on that
machine, and running a crew needs the model provider configuration the desktop
holds. Pretending would move the failure somewhere confusing.

**One WebSocket per channel.** More precise filtering, but the desktop already
subscribes once and switches chats in the UI; per-channel sockets would multiply
connections for no benefit.

**A token as the only credential.** Sessions are better for a person at a
keyboard (revocable, short-lived, tied to a password change) and tokens are
better for a script. Both share the `Bearer` header; the server tries a session
first and falls back to a token.

**A separate DTO crate.** The client already re-exports the core types, and the
wire shapes are those types. A parallel DTO layer would double every change.

## Consequences

- The server crate now depends on `cowbot-service` for the write paths that
  publish on the bus (channel posts, task creation, approvals). The dependency
  is the same one `cowbot-client` already has.
- `Client` gained `request_task_worktree`, the first member added since the
  trait was written. It exists so a workspace tool can ask for a worktree
  without knowing whether it holds a local or remote client; both answer
  honestly.
- Every new route must open with `require_member` (or route through a helper
  that does), and the tests exercise the negative case for the routes that
  exist. A route that forgets it is visible in review as the one missing line.
- The stream treats new channels as eventually visible. A channel created by
  another user during a session appears on reconnect; the create response covers
  the creator. Closing this needs a channel-scoped event the bus does not
  publish yet.
- Memory, prompt snapshots, context previews, and templates remain local until
  the next milestone; `RemoteClient` returns empty reads for them rather than
  errors.
