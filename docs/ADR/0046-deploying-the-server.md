# ADR 0046: Deploying the server

- Status: Accepted
- Date: 2026-11-14
- Discussion: Phase 2 milestone 2.4 in [the roadmap](../ROADMAP.md).
- Supersedes: None. Builds on [ADR-0043](0043-self-hosted-server-auth.md) through [ADR-0045](0045-collaboration-api-and-remote-client.md).

## Context

Milestones 2.1–2.3 built a server that works when started by a developer: the
configuration is compiled in, the binary is not packaged, and TLS is somebody
else's problem. A team that wants to run it had no image, no way to set the
database path or the listen address without editing code, and no guidance on
backup or upgrades. `SECURITY.md` said "do not deploy beyond localhost", which
was honest but left no path off localhost.

## Decision

### 1. Configuration is environment-only, and a bad value stops the server

`ServerConfig::from_env` reads eight variables and keeps the current defaults
for anything unset, so a developer's `cargo run` behaves exactly as before. A
value that does not parse — a mistyped address, a non-numeric session length —
prints the variable name and exits `2`. Silently falling back would move the
server somewhere the operator did not intend, which is worse than not starting.

There is no configuration file. The settings are few, they are deployment-time
constants, and an environment variable is the one thing every container runtime
and process manager agrees on.

### 2. The image runs unprivileged with one volume

`deploy/Dockerfile` is a two-stage build: a Rust builder compiles
`cowbot-server` in release mode with `--locked`, and a slim Debian runtime
carries only the binary, the SQLite runtime library, and `curl` for the health
check. The process runs as a dedicated `cowbot` user, listens on
`0.0.0.0:8787` inside the container, and keeps its database on `/data`.

The image declares `HEALTHCHECK` against `/health`, so Compose and any
orchestrator can tell a started container from a working one.

### 3. TLS is the proxy's job, and the server stays on loopback behind it

The server does not terminate TLS. It never has, and adding it would mean
certificate renewal, cipher policy, and a second configuration surface for a
component that has none today. `deploy/compose.yaml` runs Caddy in front of it;
Caddy obtains and renews a certificate for the deployment's domain and forwards
every request, including the WebSocket upgrade, to the server on the compose
network.

**This is what lets the server trust `X-Forwarded-For`.** The rate limiter uses
that header for per-source sign-in limits, and a client that can set it freely
can dodge the limit. The container binds `127.0.0.1:8787` and is not published,
so only the proxy reaches it — the header is trustworthy because the only
sender is the proxy. A deployment that publishes the port directly would put
the rate limiter back to one shared bucket and should not.

### 4. One file is the whole backup

The database is one SQLite file. The operations guide says to stop the server
before copying it — a copy taken mid-write is inconsistent — and that restore is
the reverse. Migrations are embedded and forward-only, so restoring an older
database onto a newer binary upgrades it, and the guide says to keep the
backup's version beside it for the reverse case.

### 5. The health route is the probe

`GET /health` is already the one unauthenticated route. It becomes the
container's `HEALTHCHECK` and the operator's probe, so there is one definition
of "up".

## Alternatives considered

**A TOML configuration file.** Matches the CLI's `cowbot.toml` and gives a
readable file, but a container would put it in the image or a second volume, and
the settings are constants set at deploy time. Environment variables are one
surface rather than two.

**Terminate TLS in the server with `rustls`.** A single binary with HTTPS is
appealing, but it makes certificate renewal the server's job and gives an
operator a second place to misconfigure. A reverse proxy is the deployment's
job, and Compose makes it one service.

**Publish `8787` and put the proxy in front anyway.** Simpler networking, but it
exposes the server to anyone on the network and makes `X-Forwarded-For`
spoofable, which quietly disables the sign-in limiter.

**Kubernetes manifests.** Out of scope for milestone 2.4, which is one small
team. Compose runs the same image a manifest would.

## Consequences

- `docs/SERVER.md` is the single place a deployment reads: configuration table,
  running from source, Compose, health, backup, restore, upgrade, token use,
  the stream, and troubleshooting.
- The `cowbot` user cannot write anywhere but `/data`, so an operator who mounts
  a different path must also give that user ownership.
- The Compose file requires `COWBOT_OWNER_EMAIL` to be set; it fails at start
  rather than creating an account at a placeholder address that cannot receive
  a password change.
- The server still has no upstream TLS termination when run directly on a host.
  `SERVER.md` says so, and `SECURITY.md` still lists TLS as an operator
  responsibility.
- PostgreSQL, horizontal scaling, and multi-process rate limiting remain out of
  scope. One server over one database file is the deployment model.
