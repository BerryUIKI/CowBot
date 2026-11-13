# CowBot server

CowBot runs two ways from one codebase: a **local** desktop app (one person, one
machine, SQLite in the app data folder) and a **self-hosted server** (a small
team sharing a workspace over HTTPS). This page covers the server.

> **Status.** The server is Phase 2 and under active development. It has
> accounts, workspaces, roles, channels, messages, tasks, approvals, audit, API
> tokens, and a live event stream. It does **not** yet host memory, prompt
> snapshots, context previews, or templates, and the desktop cannot switch to a
> server from its settings — a client speaks to it directly for now. Treat it as
> a small-team deployment on a network you control, not a public service.

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

A value that does not parse stops the server with a message rather than silently
falling back — a mistyped listen address should not move the server somewhere
unexpected.

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
container image declares it as its health check; point your own probe at it.

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
