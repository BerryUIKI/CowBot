# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository Overview

CowBot is an open-source, local-first multi-agent collaboration platform in Rust: role-based agent crews, channel/conversation collaboration, an approval-gated tool sandbox, an event-driven bus, a `cowbot` CLI, a Tauri v2 desktop app, and a self-hosted team server. One codebase runs in two forms — **local** (one person, SQLite in the app-data folder) and **self-hosted server** (a small team sharing a workspace over HTTPS).

## Git Workflow (Trunk-Based Development)

- **NEVER push directly to `main`.** It is branch-protected: PR required, the `Workspace (fmt, clippy, test)` and `Desktop (frontend and Tauri backend)` checks required (branches up to date), force-push/deletion blocked.
- `main` — the trunk and the only long-lived branch; always releasable. A release is a `v<version>` tag cut from a reviewed commit.
- `feature/<name>` — short-lived, cut from `main`, merged back via `gh pr create --base main`, deleted after merge.
- **No `dev`, `release/*`, or `hotfix/*` branches** — `dev` was retired at 0.1.17 (ADR-0063).
- One coherent change per PR, small atomic commits, rebase/merge `main` in daily, fix a red trunk before starting anything new.
- **All repository artifacts — code, comments, docs, commit messages, branch names — must be English.** Conversation with the user may be Chinese.

## Development Commands

CI (`.github/workflows/ci.yml`) runs every command below with `--locked`; match it locally.

### Workspace (Rust)

```bash
cargo build --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo doc --workspace --no-deps --locked
```

Focused iterations:

```bash
cargo test -p cowbot-service
cargo test -p cowbot-store --test migrations
cargo test -p cowbot-bus -- test_bus_publish_and_receive --exact
```

Desktop gates are **not** covered by the workspace commands — `desktop/src-tauri` is a standalone Cargo project outside the root workspace:

```bash
cd desktop
pnpm install --frozen-lockfile
pnpm run build                                                    # tsc + vite
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

### Generated desktop types (ADR-0040)

Tauri payload types are generated from `cowbot-core`, never hand-written. After changing a core type the desktop renders:

```bash
cd desktop
pnpm run generate:types      # cargo test -p cowbot-core --features bindings --lib + index generation
pnpm run check:types         # fails if desktop/src/generated is stale
```

Commit the regenerated `desktop/src/generated`. Opting a type in is one attribute: `#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]`. `TS_RS_EXPORT_DIR` and `TS_RS_LARGE_INT = "number"` are pinned in `.cargo/config.toml`.

### Running things

```bash
cargo run -p cowbot-cli -- --help
cargo run -p cowbot-cli -- init --db cowbot.db
cargo run -p cowbot-cli -- tools
cargo run -p cowbot-cli -- run --task "Plan a calculator" --dry-run --db cowbot.db
cargo run -p cowbot-cli -- channel create --name general --topic "General"
cargo run -p cowbot-cli -- message post --channel <id> --content "Starting the run" --wait
cargo run -p cowbot-cli -- export --output my-workspace.cowpkg --db cowbot.db

cargo build --release -p cowbot-server && ./target/release/cowbot-server

cd desktop && pnpm run tauri dev                                  # native window + Vite
cd desktop && pnpm run tauri build -- --no-sign                   # local bundles, no updater signature
```

The mock provider needs no credentials, so `--dry-run` works offline but still writes run history to SQLite. Desktop prerequisites are in `desktop/README.md`; a browser-only Vite preview has no native backend.

## Configuration & Runtime State

- **CLI database** resolution order: `--db`, then `COWBOT_DB`, then `db` in `cowbot.toml`, then `cowbot.db` in the working directory. `--config`/`COWBOT_CONFIG` point at another `cowbot.toml`; flags and env override the file. **API keys are never read from `cowbot.toml`** — they stay in environment variables (ADR-0020).
- **Desktop** keeps its state in `app_data_dir()/config.json` (model providers with plain-text API keys, onboarding flag, locale, database folder, workspace registry, device-local project registry) and databases at `app_data_dir()/cowbot.db` plus `app_data_dir()/workspaces/<uuid>.db`. A debug build uses `app_data_dir()/dev` and `COWBOT_DATA_DIR` overrides the directory, so `tauri dev` never migrates a release workspace. Environment variables do not configure the desktop.
- **Server** reads deployment env vars (see `docs/SERVER.md`): `COWBOT_SERVER_DATABASE`, `_ADDRESS`, `_OWNER_*`, `_WORKSPACE`, `_WORKSPACE_DIR`, session/login limits, and the all-or-nothing `COWBOT_SERVER_LLM_*` group. A value that does not parse exits rather than falling back. On an empty database it creates the owner and prints the generated password once.

### Migrations

`cowbot-store` embeds `crates/cowbot-store/migrations/*.sql` at compile time with `sqlx::migrate!`; sqlx records each file's SHA-384 and refuses to run when an applied file changed. Therefore:

- **Never edit an applied migration** — add the next numbered file (currently `0021`; next is `0022`).
- Migrations are forward-only; `.gitattributes` pins them to `text eol=lf` because CRLF checkouts produce different checksums (ADR-0025). Generated TypeScript is pinned the same way.
- `Store::migrate` runs a narrow compatibility check that repairs the LF/CRLF checksum variant only after verifying the live schema matches, and refuses a database from a newer build while naming both schema versions (ADR-0025, ADR-0051).

## Architecture

### Layering

Strictly layered — a crate may depend only on the layers below it:

```
cowbot-core        domain types, events, turn contract, token counters   (depends on nothing internal)
  ├─ cowbot-bus    broadcast EventBus + bounded history
  ├─ cowbot-tools  ToolRegistry, sandbox backend, static risk classifier
  ├─ cowbot-llm    provider protocols, streaming, tokenizers, model discovery
  ├─ cowbot-store  SQLite + sqlx migrations and repositories
  └─ cowbot-agent  runtime loop, TurnObserver, Crew
       └─ cowbot-service   application services (turns, routing, memory, approvals, scheduler, preview, snapshots)
            └─ cowbot-client  Client trait; LocalClient + RemoteClient (local depends on server only as a dev-dependency)
                 ├─ cowbot-cli     the `cowbot` binary
                 └─ cowbot-server  axum HTTP + WebSocket API, auth, workspace roles, integrations
cowbot-integrations  shared IM adapter layer (inbound ingest, outbound mirror, approvals, attachments)
  └─ cowbot-mattermost / cowbot-telegram / cowbot-slack / cowbot-feishu   platform adapters
desktop/src-tauri    standalone Cargo project; speaks only to cowbot-client
```

Changing a boundary means changing an ADR first — read `docs/ARCHITECTURE.md` and the relevant `docs/ADR/` record before restructuring.

### Crate responsibilities

- **`cowbot-core`** — `AgentProfile`, `Task`, `Channel`/`ChannelMessage` (a channel with `agent_id` is that agent's direct conversation; `kind` separates group channels, direct conversations, and user-created conversations), conversation sessions/settings/participants, append-only `ContextEntry` branches with peer authorship, compaction helpers, routing sources and modes, notes and memory entries with four scopes plus deterministic ranking/selection and credential redaction, task run groups and sources, worktree lifecycle types, `@mention` parsing, the `Tool` trait, the `Event`/`EventPayload` bus model, token-usage types with `TokenCounter` and a character-estimate fallback, read-only `PromptPlan` values, sealed `LlmPromptSnapshot` documents, the `/btw` `LlmCallKind::SideQuestion` (ADR-0039), the static slash-command catalogue (ADR-0038), and the turn contract — `TurnId`, `TurnEvent` (started, text/reasoning deltas, tool start/finish, approval required, done, error), `TurnControl`, `CancelSignal` — shared by chat, group, channel, and task turns (ADR-0041).
- **`cowbot-bus`** — Tokio-broadcast `EventBus` with subscriber handles and bounded in-memory history.
- **`cowbot-tools`** — `ToolRegistry`, `CodeExecutionTool`/`ReadFileTool`/`WriteFileTool`, the pluggable `SandboxBackend` implemented by `SubprocessSandbox`, and `StaticRiskClassifier`.
- **`cowbot-llm`** — `LlmProvider` with OpenAI Chat Completions, Anthropic Messages, OpenAI Responses, and `MockLlmProvider`. `chat_stream` emits normalized text/reasoning/tool-call/usage chunks with cooperative cancellation over a shared SSE decoder; a default adapter keeps non-streaming providers working. Model-to-tokenizer map counts `o200k_base`/`cl100k_base` exactly and falls back to the character estimator elsewhere. `list_models` returns `DiscoveredModel` (id plus any advertised context window / output limit, `None` when the endpoint is silent).
- **`cowbot-store`** — SQLite repositories for identity, collaboration, agents/templates, tasks/assignments/runs/tool calls/LLM calls/approvals, worktrees, conversation settings and context, notes and memory (FTS5 trigram index maintained by triggers, LIKE fallback, recall stats), and prompt snapshots. Notable invariants: each run persists the executing agent profile snapshot; `upsert_agent` never reassigns `workspace_id` (a mismatched agent id writes nothing and returns `false`); snapshot rows cascade with their LLM call and are hash-verified on read; the last workspace Owner is protected by a store-level owner count (ADR-0044).
- **`cowbot-agent`** — the loop (LLM call → tool calls → observations) with optional `ToolGate`, `LlmUsageRecorder`, and `ConversationRecorder`, `execute_messages` for caller-assembled requests, streaming via `TurnObserver`, a partial draft that survives stops and failures, sequential `Crew`, and `StandardRoles` (Planner, Coder, Reviewer).
- **`cowbot-service`** — the application layer: bootstrap, channel/message operations (persist first, then publish), channel and conversation management, channel mention turns, conversation sessions with independent branches, group routing (mention/router/rotation/default) with peer mirroring and post-routing capacity skips, memory recall and injection incl. the scoped `remember` tool, the assignment runtime and scheduler (wake/re-entry, cross-root concurrency, budgets, stall guard, restart reconciliation), the worktree service, the approval service (risk policy, allow/ask/deny rules, session decisions, Bypass Critical tri-state), the crew runner, and the shared `TurnRunner` every turn goes through (opens the turn, publishes lifecycle events, stores the reply **before** the terminal event, keeps a stopped turn's partial text; `cancel_turn` is idempotent). `PreviewService` builds read-only `PromptPlan`s and never writes; `PromptSnapshotService` seals/verifies/sweeps snapshots.
- **`cowbot-client`** — the transport-neutral `Client` trait plus `LocalClient` (opens SQLite, bootstraps the workspace, reconciles startup state) and `RemoteClient` (HTTP + WebSocket, same trait, ADR-0045/0053). Members that need the local machine — worktrees, in-process crew execution, live-turn cancellation, process-local session decisions — report server mode plainly. The membership surface is on the trait: list members, change a role, remove someone, list/mint/revoke an invitation, and accept one. A local workspace answers `list_members` with its one real member and refuses the rest, because it has no server to invite into. `RemoteClient::redeem` is the pair's other door: it takes an invitation and creates the account behind it (ADR-0066), and it and `sign_in` read the same response into a client. `ClientSession` carries `role` and `grantable_roles`. Both constructors read `/health` and refuse a mismatched API version by naming both numbers; plain reads retry connection failures, timeouts, `5xx`, and `429` up to three times, writes are sent once (ADR-0054). Never let store or service types leak across this boundary.
- **`cowbot-package`** — `.cowpkg` ZIP with a manifest and per-file SHA-256; export redacts credentials, import always creates a new workspace and re-keys. Integrations are deliberately excluded — a bot token and dedupe history are deployment state (ADR-0056).
- **`cowbot-integrations`** — the shared adapter layer (ADR-0056). `Integration` carries only `run` + `deliver`; a `Registry` holds one factory per kind. The money-losing rules live here and are tested against a fake adapter: claim the platform event id before posting (retries are no-ops, failed posts release the claim), drop any message from a bot (two bots mentioning each other loop forever), resolve authors to `bridge:{platform}:{integration}:{external}` principals that have no password hash, and rewrite a mention of the bound bot as `@AgentName`. `outbound` mirrors only `Sender::Agent` and chunks on paragraph/line/sentence boundaries, by characters or (through `ChunkLimit::Units`) by UTF-16 units. `supervisor` runs inbound and outbound on separate tasks. `approvals` posts a card and calls `ApprovalService::decide`, gated by `may_decide` — only the requesting account or a workspace admin, because every role can write and the card sits in a shared room (ADR-0060). `attachments` stores a file below the tool root (20 MB cap, named from the platform id, fetched only after the dedupe claim); models cannot be sent images in this build. Four additive tables (`0021`); configuration is reachable only through `Store::integration_config` and never returned by the API.
- **Platform adapters** — each one exists to absorb a platform's specific traps; keep those properties intact when editing:
  - **`cowbot-mattermost`** (WebSocket, ADR-0057): token is sent in-band as the first frame and readiness is a `status` event, not the handshake; `wss://` is refused because the socket URL is derived from `base_url`; only a mention of the bot starts a turn; post size and identity are discovered at startup.
  - **`cowbot-telegram`** (Bot API, ADR-0058): entity `offset`/`length` are **UTF-16 code units** — `slice_utf16` returns `None` inside a surrogate pair rather than panicking. `409`/`401` are terminal and name the fix; `429` waits exactly `retry_after`. Markdown → HTML escapes first, falls back to plain text. Reports `ChunkLimit::Units(4_096)`.
  - **`cowbot-slack`** (Socket Mode, ADR-0059): `apps.connections.open` once **per dial** (single-use, 30 s); acknowledge every envelope by `envelope_id` **before** handling (3 s window). A message with a `subtype` is not a message — otherwise every edit stores a second turn. Dedupe key is `channel:ts`, since `ts` is unique only within a channel. `api.rs` refuses `url_private` hosts that are not `slack.com` before attaching the token.
  - **`cowbot-feishu`** (long connection, ADR-0061): hand-written protobuf codec in `frame.rs`; answer every data frame with `200` even on handler failure (a redelivery is worse). Dedupe on the **message id**, not `header.event_id`; `content` is JSON inside JSON; `@_user_N` names live only in `mentions[]`, with `@_all` handled first. Every business failure is HTTP 200 with a non-zero `code`.
  - Every adapter dials out by default, so deployments need no public URL; `accepts_webhook`/`webhook` add the other transport via `crates/cowbot-server/src/webhooks.rs` (`POST /api/v1/integrations/{id}/webhook`, raw body verified before parsing, ADR-0062). To add a platform: implement `Integration` in a new crate, register it in `crate::adapters::registry()` in `cowbot-server`, and cover the dedupe/addressing rules in `cowbot-integrations/tests/pipeline.rs` style tests.
- **`cowbot-cli`** — the `cowbot` binary (`init`, `run`, `tools`, `export`, `import`, `channel`, `message` incl. `--wait`, `memory`); `run` supports `--db`, `--dry-run`, `--channel`, `--concurrency`, `--approval`. Settings resolution and key handling follow ADR-0020.
- **`cowbot-server`** — axum HTTP + WebSocket (ADR-0043). Auth: Argon2id, ≥10-char passwords, sign-in spends equal work on unknown emails and returns the same error, 256-bit bearer tokens stored as SHA-256 with two-week expiry, password change drops every session, failed sign-ins limited per `X-Forwarded-For` (10 per 15 min). `POST /api/v1/invites/redeem` is the one other public write route: the invitation is the credential, so it creates the account and issues the session, and it counts failures against the same per-source bucket as sign-in because it hashes a password too (ADR-0066). `AuthenticatedUser` is a `FromRequestParts` extractor. Every error is one `{code, message}` JSON body; storage errors are logged in detail and reported generically. Workspace routes resolve the caller's role first (`require_member` returns `404` rather than `403` for non-members, so existence cannot be probed) and `Role` in `cowbot-core` carries the policy (`can_manage`, `can_grant`, `can_invite`, `can_administer`, `can_write`), with `grantable` sent to clients as `grantable_roles`. `GET /api/v1/workspaces/{id}/stream` filters the process-wide bus to the workspace and sends `{"type":"Resync"}` on lag. When `COWBOT_SERVER_LLM_*` is set, `TurnExecutor` answers channel turns itself, holding one `ChannelAgentService` per workspace (it owns the open-turn registry and compaction branch locks); its tool set is deliberately narrow — `execute_code`, `read_file`, `write_file` rooted at `COWBOT_SERVER_WORKSPACE_DIR` — and it derives context windows from `COWBOT_SERVER_LLM_CONTEXT_WINDOW`/`_MAX_OUTPUT_TOKENS` (ADR-0055). Operations guide: `docs/SERVER.md`; deployment: `deploy/` (multi-stage Dockerfile, Compose with Caddy terminating TLS).

### Cross-cutting rules

- **The frontend never touches SQLite or `cowbot-service`.** All desktop access goes through Tauri commands wrapping `Client`; API keys never cross back to the frontend.
- **The server never learns adapters by name** — it asks the registry.
- **Tests must be deterministic**: no live APIs, no secrets. Fakes live beside the shared layer (e.g. the fake adapter in `cowbot-integrations`).
- **A new capability lands with an ADR** (`docs/ADR/0067-...` is next, 0001–0066 exist) and updates `docs/ARCHITECTURE.md`, `docs/ROADMAP.md`, and `docs/LIMITATIONS.md` where affected.

## Desktop App

`desktop/` is React 19 + TypeScript + Vite on pnpm; `desktop/src-tauri/` is the standalone Tauri v2 backend. Both hooks live in the shell: it chooses the backend at startup — `LocalClient` on the active SQLite database, or `RemoteClient` resumed from a saved server session when the device is in server mode (`BackendMode::Server`, configured from **Settings → Server**: `connect_server`, `switch_server_workspace`, `disconnect_server`) — and gates startup on a schema-version comparison with an in-app Upgrade / Back up and upgrade / Quit screen (ADR-0051, ADR-0053). Worktrees are reconciled locally before the backend is chosen.

The UI is conversation-first: a searchable sidebar merging agent direct chats and group channels with pinned-first ordering and live background task cards; a bubble timeline with streaming, reasoning, tool chips, a stop button, and a floating composer; a right-side inspector (Task, Context, Approvals, Background) that keeps detail beside the chat; context meters with a colored stacked bar and a compaction popover; group routing controls and per-participant context tabs; slash commands and `/btw` side questions; Markdown plus sandboxed HTML artifacts; Settings for model providers (vendor-first catalogue, three protocols, model discovery, multiple models per provider), team/templates, memory, approvals and permission rules, audit and recorded prompts, projects, workspace transfer, server, and About/updates. The interface ships 13 locales (en, zh-CN, zh-TW, fr, de, hi, id, it, ja, ko, pt-BR, es-ES, es-419) through typed catalogues; `ui_locale` is device-local. The shell is resident by default — the window's × hides to a tray icon, and `close_to_tray` in `config.json` (Settings → General) restores close-means-quit.

`desktop/src-tauri` keeps providers, onboarding, update, locale, database folder, workspace registry, and project registry in `config.json`; exposes Tauri commands for the whole `Client` surface (session, channels, conversations, context previews, prompt snapshots, memory, projects, worktrees, models, onboarding, app, workspace, server, members and invitations); forwards client events on `cowbot-event` and group replies on `cowbot-channel-reply`. `src/shell.rs` keeps it resident: the main window's close is intercepted and hides it to a tray icon, and the tray's Quit refuses while `AppState.running` or `AppState.replies` is non-empty (ADR-0064). `src/notify.rs` subscribes the same client and shows a system notification for a settled task run or a waiting approval, but only while the window is hidden or minimised and the device switch is on; its copy is English because the shell has no locale (ADR-0065). Settings → Server holds membership twice over, because the two audiences are different: a disconnected device offers **Join with an invitation**, which calls `redeem_invite` and creates the account, while a connected one offers **Join another workspace** and the member and invitation panels (`ServerMembers.tsx`, driven by `ClientSession.grantable_roles`). Release builds come from `.github/workflows/release.yml` on `v*` tags (unsigned NSIS/MSI, universal DMG, deb/AppImage) with `TAURI_SIGNING_PRIVATE_KEY`-signed updater artifacts and a `latest.json` manifest (ADR-0018); tags must match `desktop/src-tauri/tauri.conf.json`.

## Website

`site/` is the GitHub Pages landing page (plain HTML/CSS/JS, no build). `gh-pages` is generated output: edit `site/` and republish with `bash site/publish.sh` (`--dry-run` writes to `tmp/gh-pages/` without pushing).

## Current Status

Phase 1 (local MVP) is complete and Phase 2 milestones 2.1–2.5 are implemented. The server now hosts the collaboration API plus conversation, memory, prompt-snapshot, template, and package surfaces, and the desktop can connect to a server from Settings → Server, where it also manages members and invitations and can join through an invitation with no account yet. Keep the docs in sync as each milestone lands.

**Known gaps** (do not assume these exist):

- Multi-user concurrency defence when two people edit the same agent or rule; server-side rate limiting for shared deployments; a per-workspace token budget and a per-workspace model provider table (the server reads one provider from the environment).
- Streaming a live server-run turn over a remote transport (it appears when it settles) and cancelling one through the API.
- Membership has no live event: the member list is a snapshot on open plus a Refresh button. Nothing sends invitation mail, so a code travels however the inviter chose to send it. The app manages the roster of the workspace it has open but cannot create a workspace (ADR-0066).
- Hierarchical orchestration / DAG workflows; the remaining ADR-0006 deferred items (hierarchical manager/DAG process types, the full OpenMausBot rule set).
- Project-scoped knowledge bases and embeddings; isolated sandbox backends (Docker/WASM); memory-aware routing; expiry/retention automation; per-agent group extraction.
- Consolidated group-plan approval and turn grouping in the background panel; merging a task worktree branch back or opening a PR from the app.
- Prompt-cache optimisation for Anthropic and Responses (M5); linking audit events and tool calls to the LLM call and snapshot that produced them.
- Desktop notifications are English-only and cover just two events (a settled task run, a waiting approval); a launch-at-login switch and a tray status badge do not exist, because a hidden webview's timers are throttled (ADR-0064, ADR-0065).

## Documentation Map

- `docs/PRD.md`, `docs/ARCHITECTURE.md`, `docs/ROADMAP.md`, `docs/USER_GUIDE.md`, `docs/DEVELOPMENT.md`, `docs/LIMITATIONS.md`, `docs/PROVIDER_CAPABILITIES.md`, `docs/SERVER.md`, `docs/COLLABORATION.md`, `SECURITY.md`
- `docs/ADR/` — 0001–0066, indexed in `docs/ADR/README.md`; `TEMPLATE.md` for new records. **Read the relevant ADR before changing an established boundary.**
- `docs/plans/` — M1 usage accounting, M2 conversations and context, M3 group conversations and routing, M4 agent memory and shared notes, M6 projects and product structure, plus the IM integration and server-turn plans
- `docs/TEMPLATE_FORMAT.md`, `docs/PACKAGE_FORMAT.md`, `docs/REFERENCES.md`, `docs/releases/`
