# CowBot server

CowBot runs two ways from one codebase: a **local** desktop app (one person, one
machine, SQLite in the app data folder) and a **self-hosted server** (a small
team sharing a workspace over HTTPS). This page covers the server.

> **Status.** The server is Phase 2 and under active development. It hosts
> accounts, workspaces, roles, channels, messages, tasks, approvals, audit, API
> tokens, a live event stream, memory, conversations and their context,
> previews, recorded prompts, templates, and package transfer. The desktop
> cannot switch to a server from its settings yet — a client speaks to it
> directly for now — and agent turns still run in the desktop's process with its
> model provider, so the server stores and serves state rather than executing
> work. Treat it as a small-team deployment on a network you control, not a
> public service.

## What it needs

- One machine, or a container runtime.
- One writable directory for the SQLite database.
- A domain name and a TLS-terminating proxy if it is reachable from outside the
  host. The server does not terminate TLS itself.

## Configuration

Every setting has an environment variable and a default that keeps a
single-machine trial working.

| Variable | Default | Meaning |
| --- | --- | --- |
| `COWBOT_SERVER_DATABASE` | `cowbot-server.db` | SQLite file the server owns |
| `COWBOT_SERVER_ADDRESS` | `127.0.0.1:8787` | Listen address |
| `COWBOT_SERVER_OWNER_NAME` | `Owner` | Display name of the first account |
| `COWBOT_SERVER_OWNER_EMAIL` | `owner@cowbot.local` | Sign-in email of the first account |
| `COWBOT_SERVER_WORKSPACE` | `Team Workspace` | Workspace created on an empty database |
| `COWBOT_SERVER_SESSION_HOURS` | `336` (two weeks) | Session lifetime |
| `COWBOT_SERVER_LOGIN_ATTEMPTS` | `10` | Failed sign-ins per source before refusal |
| `COWBOT_SERVER_LOGIN_WINDOW_SECS` | `900` | Rate-limit window |
| `COWBOT_SERVER_WORKSPACE_DIR` | `cowbot-workspace` | Directory the file tools are rooted at and the sandbox runs in |
| `COWBOT_SERVER_LLM_PROTOCOL` | unset | `openai-chat`, `anthropic`, `openai-responses`, or `mock` |
| `COWBOT_SERVER_LLM_BASE_URL` | unset | Provider endpoint; required for every protocol but `mock` |
| `COWBOT_SERVER_LLM_API_KEY` | unset | Provider key; never written or returned by the API |
| `COWBOT_SERVER_LLM_MODEL` | unset (or `mock`) | Model every channel turn is answered with |
| `COWBOT_SERVER_LLM_CONTEXT_WINDOW` | unset | The model's context window, so a full conversation is skipped rather than sent |
| `COWBOT_SERVER_LLM_MAX_OUTPUT_TOKENS` | unset | The model's output limit, subtracted from the window |

A value that does not parse stops the server with a message rather than silently
falling back — a mistyped listen address should not move the server somewhere
unexpected.

### Answering channel turns

The four `COWBOT_SERVER_LLM_*` variables are all-or-nothing. Set none and the
server stores a posted message and runs no turn, which is what a server has
always done: the reply is the job of whichever desktop holds the model
provider. Set all of them and the server answers a message that mentions an
agent itself, with no desktop in the loop — see
[ADR-0055](ADR/0055-server-side-turn-execution.md).

```bash
COWBOT_SERVER_LLM_PROTOCOL=openai-chat \
COWBOT_SERVER_LLM_BASE_URL=https://api.openai.com/v1 \
COWBOT_SERVER_LLM_API_KEY=sk-... \
COWBOT_SERVER_LLM_MODEL=gpt-4o \
cowbot-server
```

Setting only some of them is a startup error naming the missing one, rather than
a server that starts and then fails every turn.

`COWBOT_SERVER_LLM_PROTOCOL=mock` answers with an echo provider and needs
neither a key nor an endpoint. It exists so a deployment can be stood up and
tested end to end without spending on a model, and so the acceptance tests run
in CI.

**The server's agent is deliberately less capable than a desktop's.** A server
turn can run code and read or write files inside `COWBOT_SERVER_WORKSPACE_DIR`,
and nothing else. The desktop's workspace tools — creating a channel,
instantiating a team, queueing a task, reaching a registered git project —
assume a local project folder the server does not have, so they are not
registered. Approvals still apply: a risky tool call in a server turn creates
the same pending approval the API lists at
`GET /api/v1/workspaces/{id}/approvals`.

Two limits are worth knowing before deploying this: the server cannot stream a
turn into a remote client's timeline while it runs (the answer appears when the
turn settles), and a turn cannot be cancelled through the API.

### IM integrations

``cowbot-integrations` is the shared layer every IM adapter sits on: channel
bindings, an inbound pipeline that dedupes a platform's retried delivery and
attributes the message to the person who wrote it, and an outbound mirror that
posts an agent's reply back into the IM
([ADR-0056](ADR-0056-im-integrations.md)).

An Owner or Admin configures one from the desktop's **Settings → Integrations**,
or directly:

```
POST   /api/v1/workspaces/{ws}/integrations                    create
GET    /api/v1/workspaces/{ws}/integrations                    list (any member)
DELETE /api/v1/workspaces/{ws}/integrations/{id}               retire
GET    /api/v1/workspaces/{ws}/integrations/{id}/bindings      list
POST   /api/v1/workspaces/{ws}/integrations/{id}/bindings      bind
DELETE /api/v1/workspaces/{ws}/integrations/{id}/bindings/{external}
```

The configuration — endpoint, bot token, signing secret — is sent once on create
and **never returned**. It is stored as JSON in the database with no encryption at
rest, so a reader of that file sees a bot token.

**Mattermost is the one platform this build can run**
([ADR-0057](ADR-0057-mattermost-adapter.md)). Its configuration is:

```json
{ "base_url": "https://chat.example.com", "bot_token": "…" }
```

The bot's own user id and the server's post size limit are read from the server,
so those are the only two values to supply. Telegram, Slack, and Feishu report
`supported: false` — a row can be created for them and a conversation bound, but
nothing runs it.

An integration starts with the server, and only when a model provider is
configured: a bot that stores a question and cannot answer it is worse than one
that is not running. Retire one through the API and restart to stop it.

**The bot must be a member of the channels it is bound to.** Mattermost delivers
posts only for channels the bot has joined, and a binding that misses one is
silent rather than broken. Two things check it: a bind responds with a `health` of
`reachable`, `unreachable`, or `unknown` and the reason, and startup posts a notice
into the CowBot channel of any binding that cannot receive. `unknown` means the
check could not run — it is never treated as success. A bot removed from a channel
after startup is not noticed until the next restart.

Setting `COWBOT_SERVER_LLM_CONTEXT_WINDOW` is worth the effort. A conversation
whose branch no longer fits is then skipped with a notice instead of being sent
and failing at the provider — and compaction, when the conversation has
auto-compaction on, runs inside the turn with the same provider. Without it the
server has no idea how large the model's window is, so it does not attempt
either.

## Running from source

```bash
cargo build --release -p cowbot-server
./target/release/cowbot-server
```

On an empty database the server creates the owner account and one workspace and
prints the generated password **once**:

```
Created owner account owner@cowbot.local
  password: 4ac739fc9c9982c619346ac2fa4e48328c6b
  Change it from POST /api/v1/auth/password after signing in.
```

Save it, sign in, and change it. On an existing database the server starts
unchanged and prints nothing.

## Running with Docker Compose

`deploy/` has a two-service Compose file: the server, and Caddy in front of it
terminating TLS.

```bash
cd deploy
cp .env.example .env          # set COWBOT_OWNER_EMAIL and the domain
# edit Caddyfile: replace team.example.com with your name
docker compose up -d
docker compose logs server | grep "password:"
```

The server binds to loopback inside the compose network, so only the proxy can
reach it. That is what lets the server trust `X-Forwarded-For` for sign-in rate
limiting and keeps the raw port off the internet.

## Operations

### Health

`GET /health` returns `{"status":"ok","api_version":1}` without a token. The
container image declares it as its health check; point your own probe at it. The
desktop reads it before signing in or resuming a session and refuses a server
that speaks another API version, naming both. A protocol bump therefore means
deploying the server and updating the desktops together; the message tells the
user which side is older.

### Backup

The database is one file. Stop the server (or use SQLite's online backup) and
copy `cowbot-server.db`:

```bash
docker compose stop server
docker run --rm -v cowbot-data:/data -v "$PWD":/backup alpine \
  tar czf /backup/cowbot-$(date +%F).tar.gz -C /data .
docker compose start server
```

A copy taken while the server is running is likely to be inconsistent; stop it
first.

### Restore

Stop the server, replace the file, start it again. Migrations run at startup, so
restoring an older database than the binary expects is safe — it upgrades — but
restoring a *newer* one is not; keep the backup's version beside it.

### Upgrading

Pull the new image and restart. Schema migrations are embedded in the binary and
run on open. Back up first; migrations are forward-only.

### Rotating the owner password

Sign in and `POST /api/v1/auth/password` with `current_password` and
`new_password`. That drops every other session for the account.

## API tokens

A script authenticates with a token instead of a session.

```bash
# Mint one (Owner or Admin). The secret is printed once.
curl -sX POST https://team.example.com/api/v1/workspaces/$WS/tokens \
  -H "authorization: Bearer $SESSION" -H 'content-type: application/json' \
  -d '{"scopes":["workspace"]}'
```

A token acts as its owner: it carries that person's current role, so demoting
them narrows their tokens immediately. Revoke with
`DELETE /api/v1/workspaces/{ws}/tokens/{id}`.

## The event stream

`GET /api/v1/workspaces/{ws}/stream?token=…` upgrades to a WebSocket and sends
each workspace event as JSON. A client that falls behind receives
`{"type":"Resync"}` and should reload the affected view.

## Connecting the desktop

Enter the server's HTTPS origin in **Settings → Server** in the desktop app,
sign in with an account in this deployment, and choose a workspace. CowBot
stores the session on that device, so a restart resumes it without asking for
the password, and the event stream reconnects on its own after a network drop.
A session the server revokes — a password change ends every session, and
`POST /api/v1/auth/sign-out` ends one — shows up as a request to sign in again.
The section also offers switching between the account's workspaces, returning to
the device's local workspace, and disconnecting. See
[ADR-0053](ADR/0053-desktop-server-mode.md).

Agent turns still execute in the desktop's process with that machine's model
provider; the server hosts the data and the collaboration surface, not the
scheduler.

## Workspace data: memory, conversations, templates, packages

Milestone 2.5 serves the rest of the workspace. Every route resolves the
workspace first, then the caller's role in it; a row from another workspace
reads as `404`.

| Area | Routes | Role |
| --- | --- | --- |
| Notes | `GET/PUT /workspaces/{ws}/memory/notes`, `PUT/DELETE .../notes/{channel}` | read: member; write: Member |
| Memory entries | `GET/POST /workspaces/{ws}/memory/entries`, `PATCH/DELETE .../entries/{id}`, `POST .../entries/{id}/{approve,dismiss,archive}` | read: member; write: Member |
| Memory settings | `GET/PATCH /workspaces/{ws}/memory/settings` | administer |
| Recall preview | `GET /workspaces/{ws}/memory/recall` | member |
| Conversations | `GET/POST /workspaces/{ws}/conversations`, `GET/PATCH .../{ch}`, `GET .../{ch}/context`, `DELETE .../{ch}/context[/{agent}]`, `GET .../{ch}/participants`, `GET .../{ch}/turns`, `GET .../{ch}/preview` | read: member; write: Member |
| Previews | `GET /runs/{run}/preview?model=…&tools=…` | member |
| Recorded prompts | `GET /workspaces/{ws}/snapshots`, `GET /snapshots/settings`, `PATCH /snapshots/settings`, `POST /snapshots/purge`, `GET /snapshots/{llm_call_id}`, `GET /runs/{run}/snapshots` | administer |
| Templates | `GET/POST /workspaces/{ws}/templates`, `GET/DELETE .../{id}`, `GET .../{id}/export`, `POST .../{id}/instantiate` | read: member; write: administer |
| Integrations | `GET/POST /workspaces/{ws}/integrations`, `DELETE .../{id}`, `GET/POST .../{id}/bindings`, `DELETE .../{id}/bindings/{external}` | read: member; write: administer |
| Packages | `POST /packages/import` (body: the archive), `GET /workspaces/{ws}/packages/export` | import: any signed-in user; export: administer |

Two details an operator should know:

- **Templates are workspace-scoped** since migration `0019`. A template written
  by an older build belongs to the first workspace in the database.
- **Import creates a new workspace owned by the importing user.** The package
  never carries credentials, so the exporting owner is not recreated as an
  account. Import into a database that has not seen the source workspace: a
  package whose ids collide is refused rather than merged.

## Troubleshooting

- **Sign-in returns 429.** The source hit the failure limit; wait out the
  window, or raise `COWBOT_SERVER_LOGIN_ATTEMPTS` if it is a shared proxy IP.
- **Every request 401s.** The session expired (two weeks by default) or the
  password changed, which drops all sessions.
- **403 on an action a member could do.** Check the role: creating channels and
  tasks needs Member, deleting or clearing a channel needs Owner or Admin
  (ADR-0044, ADR-0045).
- **The server will not start after an edit.** A configuration value did not
  parse; the error names the variable.
