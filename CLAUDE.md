# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository Overview

CowBot is an open-source, high-performance multi-agent collaboration platform with code execution and automation capabilities, built in Rust. It utilizes an event-driven message bus for decoupled multi-agent communication and modular tool sandboxing.

## Project Status & Workflow

**Current phase: Phase 1 (local/offline MVP) is complete on `dev`; Phase 2 (self-hosted team mode) has not started.** Milestones 1.1–1.7 are implemented. Keep the documentation in sync as each new milestone lands.

- **Implemented baseline**: the workspace crates below, a working sequential agent crew, a local subprocess code sandbox, OpenAI Chat Completions, Anthropic Messages, and OpenAI Responses LLM clients plus a mock provider, SQLite persistence via `cowbot-store`, a transport-neutral `cowbot-client` with a local backend, the assignment scheduler with wake/re-entry, cross-root concurrency, and scoped background turns (ADR-0019, ADR-0023), channel mention turns that answer `@Agent` messages in the channel as ordinary assignments (ADR-0022), agent direct conversations that every agent gets without a mention (ADR-0024), the `cowbot` CLI (`init`, `run`, `tools`, `export`, `import`, `channel`, `message` including `--wait`, plus `cowbot.toml` configuration), `.cowpkg` workspace packages via `cowbot-package`, and a Tauri v2 desktop shell with first-run onboarding plus a conversation-first React interface — searchable agent contacts, group channels, bubble chat with a typing indicator and floating composer, tasks, approvals, and settings — that can start crew execution, manage switchable model providers across three API protocols through a vendor-first catalogue with endpoint model discovery (ADR-0021), and create, export, import, and switch local workspaces. A release workflow builds unsigned Windows, macOS, and Linux installers, and `docs/USER_GUIDE.md` documents the desktop workflow. The desktop checks GitHub Releases for signed updates at launch and installs them from Settings → About (ADR-0018). Tests, `clippy -D warnings`, and `cargo fmt --check` pass; CI runs them plus the desktop gates on every PR to `dev` or `main`.
- **Known gaps** (not yet built): Phase 2 (`cowbot-client` remote mode, server API, multi-user workspaces), hierarchical orchestration and DAG workflows, agent long-term memory, isolated sandbox backends (Docker/WASM), REST/WebSocket surface, streaming responses, and retries/rate limiting/token accounting. The remaining ADR-0006 deferred items are hierarchical manager/DAG process types and the full OpenMausBot rule set (consult/notify edge cases, outward-action reconciliation, provider capability matrix).
- **Documentation**: `docs/PRD.md`, `docs/ARCHITECTURE.md`, `docs/ROADMAP.md`, `docs/USER_GUIDE.md`, `docs/ADR/` (0001–0024), `docs/releases/` (archived release notes), `docs/TEMPLATE_FORMAT.md`, `docs/PACKAGE_FORMAT.md`, and `docs/REFERENCES.md` (external design references). Update them with every milestone.
- **Commit style**: small, atomic commits; frequent pushes and small PRs.
- **Language**: all repository artifacts — code, comments, docs, commit messages, and branch names — must be English; conversation with the user may be Chinese.
- **Git workflow**: GitFlow with `main` as the release branch and `dev` as the integration branch (see below).

## Git Workflow (GitFlow)

- **Strict rule**: NEVER push directly to `dev` or `main`. Both branches have GitHub branch protection enabled with pull request requirements and force-push/deletion blocked.
- `main` — release-ready history only; protected.
- `dev` — integration branch for completed features; protected. All feature pull requests must target `dev`.
- `feature/<name>` — branched from `dev`, pushed to origin, and merged into `dev` via Pull Request (`gh pr create --base dev`).
- `release/<version>` — branched from `dev` when preparing a release, merged into both `main` and `dev` via Pull Request.
- `hotfix/<name>` — branched from `main` for urgent fixes, merged into both `main` and `dev` via Pull Request.
- Commit messages are small, atomic, and English; always use PRs to merge into `dev`.

## Language

All repository artifacts — code, comments, documentation, commit messages, and branch names — must be written in English. Conversation with the user may be in Chinese; do not add mixed-language sections to docs.

## Workspace Architecture

The project is structured as a Cargo workspace under `crates/`:

- **`crates/cowbot-core`**: Core domain types and shared errors (`CowError`). Defines `AgentProfile`, `Task`, `Channel`/`ChannelMessage` (a channel with `agent_id` is that agent's direct conversation, ADR-0024), `@mention` parsing and resolution (ADR-0022), the `Tool` trait, and the `Event`/`EventPayload` bus message model.
- **`crates/cowbot-bus`**: Tokio broadcast-backed `EventBus` with subscriber handles and a bounded in-memory event history.
- **`crates/cowbot-tools`**: `ToolRegistry` plus the built-in tools (`CodeExecutionTool`, `ReadFileTool`, `WriteFileTool`), the pluggable `SandboxBackend` trait implemented by `SubprocessSandbox`, and the deterministic `StaticRiskClassifier`.
- **`crates/cowbot-llm`**: `LlmProvider` abstraction with an OpenAI Chat Completions client (works with OpenAI, DeepSeek, Ollama, etc.), an Anthropic Messages client (Claude and Claude Code-compatible relays), an OpenAI Responses client (Codex-style endpoints), and a `MockLlmProvider` for offline tests and dry runs. `LlmProvider::list_models` queries each protocol's model catalogue for the desktop's Fetch models action (ADR-0021).
- **`crates/cowbot-agent`**: Agent runtime loop (LLM call → tool calls → observations), an optional `ToolGate` consulted before each tool execution, sequential `Crew` orchestration, and preset roles in `StandardRoles` (Planner, Coder, Reviewer).
- **`crates/cowbot-store`**: SQLite persistence built on `sqlx`: embedded migrations, connection management, and repository methods for identity, collaboration, agents/templates, tasks/assignments/runs/tool calls/approvals, and audit events.
- **`crates/cowbot-client`**: Transport-neutral `Client` trait and the local-mode `LocalClient`, which opens SQLite, bootstraps the local workspace, runs startup reconciliation, and exposes session, channel/message, channel turns (`reply_to_mentions`), agent conversation provisioning (`ensure_agent_channels`), template, agent, task/assignment/approval, history (runs, tool calls, assignment events, audit), and event-stream operations without leaking store or service types.
- **`crates/cowbot-package`**: `.cowpkg` workspace packages (ADR-0004): ZIP container with a manifest, per-file SHA-256 checksums, export of a workspace, and verified import that always creates a new workspace.
- **`crates/cowbot-service`**: Application services for the local runtime: idempotent local owner/workspace bootstrap, channel/message operations that persist first and then publish events, idempotent agent direct-conversation provisioning (ADR-0024), channel turns that turn mentions or direct messages into assignment-backed answers (ADR-0022), the assignment runtime (tree lifecycle, event log, budgets, stall guard, restart reconciliation), the assignment scheduler with wake/re-entry and cross-root concurrency (ADR-0019), the approval service (risk policy, pending approvals, audit), a scheduler-backed crew runner, and template management with built-in templates, validation, CRUD, and `.cowtemplate` import/export.
- **`crates/cowbot-cli`**: The `cowbot` binary. `init` opens the local database through `cowbot-client` and prints the session; `run` executes the Planner/Coder/Reviewer crew through `LocalClient::run_crew_with_options` with persisted history, `--db`, `--dry-run`, `--channel`, `--concurrency`, and an interactive `--approval` policy; `channel` and `message` list/create channels and list/post messages (`message post --wait` lets mentioned agents answer); `tools` inspects the registry and smoke-tests the sandbox. Settings resolve from `--config`/`COWBOT_CONFIG`/`cowbot.toml` with CLI and environment values taking precedence, and API keys stay in environment variables (ADR-0020).

## Website

`site/` holds the GitHub Pages landing page (plain HTML/CSS/JS, no build step): a hero with an interactive approval demo, platform download links resolved from the GitHub Releases API, feature and architecture sections, and a quick start. It is published from the `gh-pages` branch, which holds an assembled copy of `site/` plus `assets/brand/` and `docs/assets/` so relative asset paths resolve. The `gh-pages` branch is generated output; edit `site/` and republish it with `bash site/publish.sh` (add `--dry-run` to write to `tmp/gh-pages/` without pushing). Pages serves that branch's root.

## Desktop App

The desktop app lives under `desktop/` and is not part of the Cargo workspace:

- **`desktop/`**: React 19 + TypeScript + Vite frontend, built with pnpm (ADR-0015). It has a three-step first-run onboarding overlay (welcome, model, starter team) and a conversation-first shell (ADR-0024): a searchable sidebar of agent contacts with avatars and last-message previews, group channels below them, footer navigation to Tasks, Approvals, and Settings, a bubble chat timeline with a typing indicator and a floating composer, and task, approval, and settings views (task creation and detail with assignments/events/runs/tool calls, cancellation, desktop-initiated execution; pending approvals with approve/reject; workspace identity and workspace switching/export/import, vendor-first model providers with protocol selection, model discovery, and multiple models per provider, agents, templates, and an About section with version and update controls). It refreshes from Tauri events.
- **`desktop/src-tauri/`**: Standalone Tauri v2 Cargo project. Opens `LocalClient` on the active workspace database (`app_data_dir()/cowbot.db` by default, or a registered database under `app_data_dir()/workspaces/`), stores model providers (with their known model lists), the onboarding flag, the update-check preference, and the workspace registry in `app_data_dir()/config.json`, exposes Tauri commands that wrap the `Client` trait (session, channels/messages, agent conversation provisioning, agents/templates, tasks, assignments, runs, tool calls, approvals) plus model provider commands (`get_model_settings`, `save_model_provider`, `delete_model_provider`, `activate_model_provider`, `test_model_provider`, `fetch_provider_models`), onboarding commands (`get_onboarding_state`, `complete_onboarding`), app commands (`get_app_info`, `set_auto_check_updates`, `restart_app`), and workspace commands (`list_workspaces`, `switch_workspace`, `export_workspace`, `import_workspace`), and forwards client events on the `cowbot-event` channel. The Tauri updater and opener plugins back in-app updates and release-note links; API keys never cross back to the frontend.
- **Architecture**: see `docs/ADR/0009-desktop-ipc-and-state.md`, `docs/ADR/0012-local-model-configuration.md`, `docs/ADR/0013-first-run-onboarding.md`, `docs/ADR/0014-desktop-release-builds.md`, `docs/ADR/0015-pnpm-for-desktop-dependencies.md`, `docs/ADR/0016-model-provider-protocols.md`, `docs/ADR/0017-desktop-workspace-transfer.md`, `docs/ADR/0018-desktop-auto-updates.md`, `docs/ADR/0021-vendor-first-model-providers.md`, and `docs/ADR/0024-agent-direct-conversations.md`. The frontend never touches SQLite or `cowbot-service`.
- **Release builds**: `.github/workflows/release.yml` builds unsigned NSIS/MSI, universal DMG, deb, and AppImage bundles on `v*` tags and manual dispatches; tag runs create draft GitHub Releases. Updater artifacts are signed with the `TAURI_SIGNING_PRIVATE_KEY` secret, and a final job publishes `latest.json` for the in-app updater (ADR-0018). Tags must match `desktop/src-tauri/tauri.conf.json`.
- **Gates**: root `cargo fmt/clippy/test --workspace` intentionally excludes `src-tauri`; run `pnpm run build`, `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml -- --check`, `cargo test --manifest-path desktop/src-tauri/Cargo.toml`, and `cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --all-targets -- -D warnings` when changing the shell.

## Common Development Commands

### Building
```bash
cargo build --workspace
cargo build --release --workspace
```

### Running the CLI
```bash
cargo run -p cowbot-cli -- --help
cargo run -p cowbot-cli -- tools
cargo run -p cowbot-cli -- init --db cowbot.db
cargo run -p cowbot-cli -- run --task "Analyze and write code" --dry-run
```

### Running the Desktop App
```bash
cd desktop
pnpm install
pnpm run tauri dev        # dev window + Vite server
pnpm run build            # type-check and bundle the frontend
pnpm run tauri build -- --no-sign   # local bundles without updater signatures
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

`pnpm run tauri build` without `--no-sign` needs the updater signing key; CI supplies it from repository secrets. See `desktop/README.md`.

### Testing
```bash
# Run all workspace tests
cargo test --workspace

# Run tests for a specific crate
cargo test -p cowbot-core
cargo test -p cowbot-bus
cargo test -p cowbot-tools

# Run a single specific test
cargo test -p cowbot-bus -- test_bus_publish_and_receive --exact
```

### Linting & Formatting
```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo fmt
```
