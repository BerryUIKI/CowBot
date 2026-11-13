# Security policy

CowBot is an early-development local runtime with a CLI and a desktop shell.
This page states the assets it protects, where the trust boundaries are, what is
known to be missing, and what an operator must do before trusting it with real
work. It replaces the shorter status note of earlier releases without changing
the reporting instructions.

**Applies to:** CowBot 0.1.9 and the `dev` branch at the time of writing.

## Assets

| Asset | Where it lives | Who can reach it |
| --- | --- | --- |
| Workspace data — channels, conversations, context branches, notes, memory | The SQLite database in the app data directory | Your OS user; anything running as you |
| Task and tool-call history, audit events, prompt snapshots | The same database | Your OS user; anything running as you |
| Provider credentials | Environment variables (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, ...) and the desktop's `config.json` | Your OS user; the desktop backend |
| Workspace packages (`.cowpkg`) | Wherever you export them | Anyone who can read the file |
| Model traffic | In flight to the configured endpoint | The endpoint operator, and anyone on the path to it |
| The device itself | Files, processes, network | The subprocess sandbox runs as you |

The database is the whole product state. It is not encrypted at rest, it is not
authenticated, and it is copied verbatim by `.cowpkg` export minus the
device-local and credential data described below.

## Trust boundaries

```
+----------------------- your OS user ------------------------+
|                                                             |
|  Desktop UI  <--IPC-->  Tauri backend  <---> SQLite         |
|                              |                              |
|                              v                              |
|                    cowbot-service (approvals, runtime)      |
|                              |                              |
|                    +---------+---------+                    |
|                    |                   |                    |
|                    v                   v                    |
|            subprocess sandbox    model provider HTTP        |
|            (your permissions)    (leaves the machine)       |
+-------------------------------------------------------------+
```

1. **The UI is untrusted input to the backend.** Model output is rendered as
   Markdown with sanitized inline HTML; `html` code blocks render inside a
   sandboxed iframe. The frontend never touches SQLite or `cowbot-service`.
2. **The sandbox is not an isolation boundary.** A tool call runs a subprocess
   as your user. Approval policy decides *whether* a call runs; it does not
   confine what the call can do once it runs.
3. **The model endpoint is outside your trust boundary.** Everything in the
   prompt — your messages, recalled memory, notes, file contents, tool output —
   leaves the machine when you use a hosted provider. A local runtime such as
   Ollama keeps it local, but CowBot cannot verify that a URL is local.
4. **Credentials never cross back to the UI.** The backend owns API keys; a
   command that returns a provider reports only whether a key is present.
5. **Prompt snapshots are device-local.** They are sealed, redacted, and never
   written into a workspace package.

## Authentication and session model

**Local mode has no authentication.** The desktop auto-creates one local user
and one workspace on first open, and the CLI does the same. There are no
sessions to steal because there is no login, and no service listening on a
socket to reach. Do not expose a local workspace to another person or another
machine and assume it is protected.

**Server mode has started (Phase 2, ADR-0043).** `cowbot-server` serves
Argon2id password accounts and opaque bearer sessions over HTTP. The properties
implemented so far:

- A password is hashed with Argon2id and must be at least ten characters; an
  unknown email and a wrong password return the same response after the same
  work, so accounts cannot be enumerated.
- A session token is 256 random bits, and only its SHA-256 hash is stored; a
  database read cannot mint a live session. Sessions expire after two weeks by
  default, and an expired one is deleted when it is read.
- Changing a password drops every session for that user.
- Failed sign-ins are counted per `X-Forwarded-For` source and refused after ten
  in fifteen minutes.

**Workspaces and roles (ADR-0044).** Workspace, membership, and invitation
routes enforce roles:

- Every workspace route resolves the caller's membership first, and a non-member
  gets `404` rather than `403`, so workspace existence cannot be probed.
- An Admin manages Members but cannot promote a peer or grant Owner; only an
  Owner grants Owner. A Member can read the workspace and its roster but cannot
  invite, rename, or change members.
- The last Owner cannot be demoted or removed, counted in the store so two
  concurrent demotions cannot both succeed.
- An invitation is a single-use 256-bit token, stored only as a hash, valid
  seven days, deleted on acceptance; its token is never returned by the listing.
- Membership changes are audited with the acting user and `source: human`.

**The collaboration surface (ADR-0045).** Channels and messages, tasks,
assignments, runs, tool calls, approvals, permission rules, the workspace
policy, agents, audit, and API tokens are served. Every route resolves the
channel or task's workspace and the caller's role *before* reading anything,
and the sender of a message is always the authenticated user — a `sender` field
in the body is ignored.

- **API tokens** are bound to one user and one workspace, 256 random bits
  stored only as a hash, revoked by stamping `revoked_at` so an audit still
  names them. Only an Owner or Admin mints one, and a token acts as its owner,
  so every request re-checks that owner's current role.
- **The live stream** (`GET /workspaces/{id}/stream`) authenticates with the
  same bearer token passed as a query parameter (a browser WebSocket cannot set
  an `Authorization` header) and filters the process-wide bus to the
  workspace's channels and tasks. A payload that cannot be placed is dropped;
  in a shared deployment, omitting it is safer than leaking it.

**Deployment (ADR-0046).** `deploy/compose.yaml` runs the server unprivileged
behind Caddy, which terminates TLS; the server binds loopback inside the compose
network and is never published. That placement is also what makes
`X-Forwarded-For` trustworthy for the sign-in limiter — a deployment that
publishes the port directly lets a client forge the header and dodge the limit.
Configuration is environment-only, and a value that does not parse stops the
server instead of silently falling back.

What server mode does **not** have yet: server-hosted memory, prompt snapshots,
context previews, and templates; TLS built into the binary (a proxy terminates
it); and any defence against two users editing the same agent or rule at once.

## Known gaps

| Gap | Impact | Plan |
| --- | --- | --- |
| Subprocess sandbox is not OS isolation | A tool call can read, write, and execute anything you can | Isolated backends (Docker, WASM) — Phase 3 |
| File tools join against a base path without traversal containment | A crafted path can reach outside the workspace directory | Symlink- and traversal-safe path resolution |
| No bound on sandbox memory, network, or descendant processes | A runaway or hostile command can exhaust the machine | Resource limits alongside the isolated backends |
| Database is unencrypted | Anyone who reads the file reads the whole workspace | Encrypted-at-rest option; relies on OS disk encryption today |
| Credentials sit in `config.json` and the environment | Any process running as you can read them | OS keychain integration |
| No authentication in local mode | Local only; no safe way to share a workspace | Phase 2 server; accounts and sessions exist (ADR-0043), workspace routes and membership enforcement are 2.2 |
| Server rate limiting is per process | A restart clears the window; several processes would not share it | One server per database is the deployment model; a shared store is a later change |
| Server has no TLS | Credentials cross the wire in clear text without a proxy | Terminate TLS at a reverse proxy; the server does not provide it |
| Prompt and tool output may appear in logs | Sensitive task data can reach console or debug output | Structured redacted logging |
| No prompt-cache or cost control per agent | A live model can spend without a per-agent budget | Per-assignment budgets already exist; per-agent spend caps planned |

The [limitations page](LIMITATIONS.md) covers the functional gaps; this table
covers the ones with a security consequence.

## Operational checklist

### Running locally

- [ ] Keep the database and any `.cowpkg` exports on an encrypted disk.
- [ ] Set API keys in the environment, not in shell history or a shared file.
- [ ] Prefer `ask` or `accept-edits` approval for a live model; `trusted` runs
      every tool call.
- [ ] Review the workspace permission rules and session decisions before a long
      unattended run.
- [ ] Run tasks that execute code in a disposable directory with no valuable
      files or credentials.
- [ ] Back up the database before opening it with a new build; migrations are
      forward-only.
- [ ] Turn off prompt snapshot capture, or set a short retention, if the content
      it seals is more sensitive than the usage data you need.

### Before any self-hosted deployment

- [ ] Treat every preceding item as still required.
- [ ] Use `deploy/compose.yaml` (or put the server behind a TLS proxy yourself)
      and never publish the server's port directly.
- [ ] Do not rely on the server for memory, prompt snapshots, templates, or
      context previews yet; those stay local.
- [ ] Terminate TLS in front of the server; CowBot does not provide it.
- [ ] Change the owner password the server prints on first start, and keep the
      generated one out of shell history.
- [ ] Decide whether model traffic may leave the network; a hosted provider
      sends prompts off-site.
- [ ] Give the process a dedicated OS user with no access to unrelated files.

## Reporting a vulnerability

Do not post exploit details, credentials, or private data in a public issue. Use
GitHub's private vulnerability reporting **if enabled** on the repository's
[Security page](https://github.com/BerryUIKI/CowBot/security). If it is
unavailable, open a public issue asking maintainers to establish a private
reporting channel, without including vulnerability details. No dedicated
security email is currently published.

Once a private channel is available, include the affected revision, platform,
minimal reproduction, expected and observed behavior, impact, and any suggested
mitigation. Use synthetic data and redact secrets. Maintainers should
acknowledge the report, coordinate a fix and disclosure with the reporter, and
avoid publishing details before a mitigation is ready. Response timing depends
on maintainer availability.

## Supported versions

Reports against the current release and the `dev` branch are welcome. Fixes land
on `dev` and ship in the next release; older releases may require an upgrade
rather than a backport. There is no long-term support branch and no support
window yet.
