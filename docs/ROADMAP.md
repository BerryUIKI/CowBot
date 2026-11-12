# CowBot Roadmap

> **Status**: Draft v0.1
> **Date**: 2026-11-03
> **Related**: `docs/PRD.md`, `docs/ARCHITECTURE.md`, `docs/ADR/`

This roadmap sequences the work from the current foundation to a self-hosted, channel-native multi-agent platform. It follows the agreed order: **local/offline first, self-hosted team mode second, ecosystem extensions third.**

---

## 1. Principles

1. **Local/offline first**: the desktop app must be fully usable without a server before multi-user server work begins.
2. **Multi-user-ready from day one**: the data model and APIs include users, workspaces, memberships, and roles even in local mode.
3. **Small, shippable increments**: each milestone produces a testable, demonstrable outcome.
4. **Documentation travels with code**: every milestone updates the relevant docs and records decisions as ADRs.
5. **Quality gates are non-negotiable**: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and workspace tests must pass.
6. **English-only artifacts**: code, comments, docs, commit messages, and branch names are English; conversation may be Chinese.

---

## 2. Phase Overview

| Phase | Theme | Primary Outcome |
| --- | --- | --- |
| **Phase 0** | Foundation | Workspace, core crates, product/architecture docs, initial ADRs |
| **Phase 1** | Local/Offline MVP | Installable desktop app with channels, agents, tasks, approvals, sandbox, and SQLite |
| **Phase 2** | Self-Hosted Team Mode | Multi-user server, shared workspaces, remote desktop client, Docker deployment |
| **Phase 3** | Extensibility and Ecosystem | Docker sandbox, knowledge bases, integrations, plugins/MCP, marketplace, enterprise features |

No calendar dates are committed; a phase ships when its exit criteria are met. Phase 0 and Phase 1 are complete; Phase 2 has not started.

---

## 3. Phase 0 — Foundation (Completed)

**Goal**: establish the repository, crate boundaries, and shared product/architecture understanding.

Delivered:

- Cargo workspace with `cowbot-core`, `cowbot-bus`, `cowbot-tools`, `cowbot-llm`, `cowbot-agent`, and `cowbot-cli`.
- Sequential agent crew with a tool-calling loop and a working subprocess code sandbox.
- OpenAI-compatible and mock LLM providers.
- `cowbot` CLI with `run` and `tools` commands.
- PRD, functional/system architecture, and ADRs 0001–0005.
- Passing tests, clippy, and formatting checks.

---

## 4. Phase 1 — Local/Offline MVP (Desktop)

**Goal**: a single user can install CowBot Desktop, create a workspace, assemble an agent team from templates, run a sequential task in a channel, approve risky tool calls, inspect deliverables, and export/import the workspace — all without a server.

### 4.1 Milestones

| ID | Milestone | Key Deliverables | Depends On |
| --- | --- | --- | --- |
| 1.1 | Persistence foundation (`cowbot-store`) | SQLite schema and migrations; repositories for users, workspaces, channels, messages, agents, templates, tasks, runs, tool calls, approvals, audit; WAL and transactional writes; store tests | Phase 0 |
| 1.2 | Local identity and workspace | Local owner bootstrap; default workspace; channel/thread/message persistence; event bus wired to storage | 1.1 |
| 1.3 | Agent templates and configuration | Template JSON schema; built-in templates compiled in; template CRUD and validation; `.cowtemplate` import/export | 1.1 |
| 1.4 | Task orchestration | Assignment runtime (root/parent/depth tree, lifecycle, event log, budgets, stall guard, restart reconciliation); sequential crew on assignments; context chaining; cancellation and failure handling | 1.2, 1.3 |
| 1.5 | Tools, sandbox, and approvals | Static risk classifier; Ask/SafeAuto/PlanOnly/Trusted modes; approval cards and audit records; code/file tools with limits | 1.2, 1.4 |
| 1.6 | Desktop shell and UI | Tauri shell; React + TypeScript UI for channels, tasks, agents, approvals, templates, and settings; local `cowbot-client` | 1.2–1.5 |
| 1.7 | Packaging and data exchange | Windows/macOS/Linux builds; `.cowpkg` workspace export/import; first-run onboarding; user documentation | 1.6 |

> **Progress**: milestones 1.1–1.5 are implemented — `cowbot-store` provides embedded SQLite migrations and repositories, `cowbot-service` provides idempotent local bootstrap, channel/message persistence wired to the event bus, the assignment runtime with event-sourced lifecycle/restart reconciliation, the assignment scheduler with wake/re-entry and cross-root concurrency (ADR-0019), the static risk classifier with approval gates and audit records, and validated built-in templates with `.cowtemplate` import/export.
>
> **Deferred beyond Phase 1** (ADR-0006): hierarchical manager and DAG process types, `consult`/`notify` edge cases, outward-action reconciliation, and the provider capability matrix. The background wake/re-entry scheduler and cross-root concurrency were delivered in ADR-0019; `AssignmentService` continues to enforce request, execution, and wake budgets. Remote mode is Phase 2.3.
>
> **1.6 progress**: the Tauri v2 shell and React frontend live in `desktop/`. The desktop now covers local session bootstrap, channel and message operations, live events, task creation/list/detail with assignment and run history, pending approval decisions, desktop-initiated crew execution of a submitted task, and a sectioned settings page in the sidebar footer for agents, templates, and model providers. Providers record their protocol — OpenAI Chat Completions, Anthropic Messages, or OpenAI Responses — in a local `config.json`; they are configured through a vendor-first catalogue, the protocol is a dropdown in the form, the app can fetch the endpoint's model list, and one provider can hold several models (ADR-0012, ADR-0016, ADR-0021). Providers can be added, edited, tested, and switched without restarting. The shell is conversation-first: the sidebar lists agent conversations with last-message previews, group channels sit below them, and each agent has a direct conversation that needs no mention (ADR-0024). Channels and conversations can be pinned, and group channels renamed or deleted, from a right-click menu; names are unique per workspace (ADR-0026). Messages and task deliverables render Markdown and sanitized inline HTML, and fenced `html` blocks open as sandboxed HTML artifacts (ADR-0027). The built-in Main Assistant (`main`) ships with desktop workspace tools for creating channels, instantiating teams, and queuing tasks, and agents can be edited (name, role, tools) from Settings or a conversation (ADR-0028).
>
> **1.7 progress**: `cowbot-package` implements the `.cowpkg` format from ADR-0004 — a verified ZIP container with a manifest, per-file SHA-256 checksums, and import that always creates a fresh workspace. `cowbot export` and `cowbot import` expose it from the CLI. The desktop now has first-run onboarding: a three-step guide covers the local workspace, model providers, and starter agents, can be skipped, and completes offline (ADR-0013). A release workflow builds unsigned Windows (NSIS/MSI), macOS (universal DMG), and Linux (deb/AppImage) installers on `v*` tags and manual runs, and creates draft GitHub Releases for review (ADR-0014). Tag builds sign updater artifacts and publish a `latest.json` manifest; the desktop checks it at launch and installs updates from Settings → About (ADR-0018). The desktop can create a new workspace, export the active workspace as a `.cowpkg` package, import a package into a new workspace database, and switch between local workspaces (ADR-0017). The [user guide](USER_GUIDE.md) covers installation, model setup, workspaces, tasks, approvals, updates, and troubleshooting.
>
> **Channel mentions (0.1.4)**: a channel message that mentions an agent by name (or `@team`) becomes a task in that channel, runs through the assignment scheduler with the channel's approval policy, and posts the answer back as a message from that agent; unknown mentions and failures produce a system notice. A scoped scheduler keeps background turns from claiming each other's assignments (ADR-0022, ADR-0023). `cowbot message post --wait` drives the same path from the CLI.
>
> **Usage accounting and conversations (M1–M2)**: every completion records normalized token usage per call, including DeepSeek cache hit/miss fields, and the desktop stores optional per-model context-window metadata (ADR-0029). User-created single-agent conversations sit above agent contacts in the sidebar and keep an independent append-only context branch per participant; the model request is a real system/user/assistant/tool message list rather than a rendered transcript. A header context ring and popover report percent used/left against the usable window, per-category tokens, last-turn and conversation cache hit rate, and offer manual compaction plus optional automatic compaction (off by default at 80%). At 100% sending is blocked with compact, clear-context, and new-conversation actions. Display history is never deleted by context operations, and workspace packages round-trip conversation data (ADR-0030).
>
> **Group conversations and routing (M3)**: user-created conversations hold two to eight ordered participants and route explicit mentions against that roster. Unaddressed messages go to an opt-in model router or a persisted default responder. Each participant keeps an independent context branch with its own meter tab, peers' replies appear as authored history, and replies post as independent tasks settle. Turn rows persist the routing audit, workspace packages round-trip the new fields, and group channels keep their mention-only behavior (ADR-0032).
>
> **Agent memory and shared notes (M4)**: a workspace note, a note per conversation, and memory entries scoped to the workspace, an agent, a conversation, or one agent in one conversation. Recall is deterministic and budgeted over an FTS5 trigram index with a LIKE fallback for short queries; pinned entries come first, the memory block is labeled reference data, and recall failures never block a turn. Capture is explicit through Settings → Memory, a per-conversation note editor, a "Remember this message" action, and an approval-gated `remember` tool; opt-in extraction stores at most three reviewed `pending` candidates per settled conversation turn and records one `memory_extract` call. Task executions recall for their assignee and task channel. Packages round-trip notes, entries, and settings with redaction on write and export. The context popover now shows a colored stacked bar with a labeled legend, the remaining tokens until auto-compaction, and the memory estimate (ADR-0033).
>
> **Worktrees and background cards (0.1.7)**: Settings → Projects registers local git repositories on this machine. A new task can opt into a per-task worktree on `cowbot/<slug>` under `<repo>/.cowbot/worktrees`; the run's file, code, and command tools are rooted at the checkout, and task detail shows the branch, diff, and discard actions. Crew tasks share the submitted task through `Task.run_group`, and assignment/worktree events feed compact sidebar cards plus a Background panel with live status, progress, and finished runs. Worktree records and project paths stay local and are excluded from `.cowpkg`; merging a branch back or opening a pull request remains out of scope (ADR-0031).
>
> **Composer slash commands (0.1.9)**: typing `/` in the composer opens a command menu driven by a catalogue and parser in `cowbot-core`, so the CLI and future remote clients share the same names, aliases, quoting, and arity rules. Fifteen commands cover help, chat management, context compaction and clearing, notes and memory, task creation, model and approval switching, cancellation, and Settings navigation. Commands are desktop actions and never become messages or context entries; context commands require a user-created conversation, destructive commands keep their confirmations, and an unknown first token shows a hint that Escape overrides so path-like input still sends (ADR-0038). `/btw <question>` asks an agent something without adding it to the conversation: the turn runs with the same tools and approval gate, but posts no message, appends no context entry, and records no routing turn, and its answer appears in a dismissible panel above the composer (ADR-0039).
>
> **Phase 1 status**: complete. The exit criteria below are met. Reproducible path: install a release → complete onboarding → add agents from a template → create a channel and a task in it → run the crew and decide an approval → inspect run and tool-call history → export the workspace, import it as a new local workspace, and switch between them. The CLI reads `cowbot.toml`, lists and creates channels and messages, and can attach a crew's tasks to a channel (ADR-0020). 0.1.x remains the current release line until Phase 2 work starts.

### 4.2 Phase 1 Exit Criteria

- A new user can install the app and complete first-run onboarding offline.
- The user can create a workspace and channel, and add agents from built-in templates.
- A task can be submitted in a channel and executed by a sequential crew with visible progress.
- Risky operations trigger approvals according to the selected mode, and decisions appear in the audit log.
- Code execution and file operations respect sandbox limits (timeout, output truncation, working directory).
- Task deliverables and run history persist across restarts.
- Assignments expose an append-only event log, enforce per-root budgets, and reconcile interrupted runs on startup.
- A workspace can be exported to `.cowpkg` and re-imported into a fresh install.
- All quality gates pass and the main flow is covered by integration tests.

### 4.3 Explicitly Out of Scope for Phase 1

- Multi-user server, invitations, and remote sessions.
- OIDC/SSO, PostgreSQL, Docker sandbox, IM/Webhook integrations.
- Project-scoped long-term knowledge bases, streaming responses, cost accounting.
- Template marketplace and plugin/MCP ecosystem.

---

## 5. Phase 2 — Self-Hosted Team Mode

**Goal**: a small team can deploy CowBot Server, invite members, share workspaces and channels, and collaborate with agents under role-based permissions and audit.

### 5.1 Milestones

| ID | Milestone | Key Deliverables | Depends On |
| --- | --- | --- | --- |
| 2.1 | Server and API foundation (`cowbot-server`) | HTTP + WebSocket API; Argon2id accounts; opaque sessions; API tokens; error model; login rate limiting | Phase 1 |
| 2.2 | Multi-user workspaces | Workspace creation; invitations; Owner/Admin/Member roles; membership enforcement; audit integration | 2.1 |
| 2.3 | Remote client mode | `cowbot-client` remote implementation; desktop sign-in and server connection; reconnection; local/team mode switch | 2.1, 2.2 |
| 2.4 | Deployment and operations | Single binary and Docker image; Compose example; TLS via reverse proxy; health checks; backup/restore; configuration reference | 2.1 |
| 2.5 | Migration and compatibility | `.cowpkg` import into the server; server-side schema migrations; version compatibility policy | 2.2, 2.4 |

### 5.2 Phase 2 Exit Criteria

- A team can deploy the server with Docker or a single binary and connect desktop clients over HTTPS.
- Multiple users can sign in, join a workspace, and collaborate in shared channels.
- Role checks prevent unauthorized workspace, agent, and policy changes.
- Approvals and tool executions record the acting user and appear in the audit log.
- Server restart preserves workspaces, messages, tasks, and approvals.
- Backup and restore procedures are documented and tested.
- Workspace export from local mode imports cleanly into the server.

### 5.3 Explicitly Out of Scope for Phase 2

- PostgreSQL, OIDC/SSO, browser access, horizontal scaling.
- Docker sandbox, long-term memory, marketplace, and plugin ecosystem.

---

## 6. Phase 3 — Extensibility and Ecosystem

**Goal**: make CowBot adaptable to more environments and workflows without compromising the secure, local-first core.

### 6.1 Workstreams

| ID | Workstream | Key Deliverables |
| --- | --- | --- |
| 3.1 | Sandbox and execution | Docker sandbox backend; resource quotas; network policy; additional built-in tools |
| 3.2 | Knowledge and retention | Project-scoped knowledge bases; retrieval-augmented context; memory retention and privacy controls; embeddings and hybrid retrieval |
| 3.3 | Integrations | Webhooks; Mattermost, Slack, and Feishu adapters; optional browser access |
| 3.4 | Ecosystem | Plugin mechanism; MCP compatibility; template marketplace with signing; SDK for custom tools |
| 3.5 | Enterprise readiness | OIDC/SSO; PostgreSQL backend; data-at-rest encryption; audit export; multi-workspace/organization management |
| 3.6 | Scale and polish | Streaming responses; retries/backoff; rate limiting; token and cost accounting; performance and UX improvements |

Each workstream can ship independently; prioritization will be decided with users after Phase 2.

### 6.2 Deferred Product Directions (Not Started)

**Product structure and vertical kits (planned, not started).** Work and Code are project profiles rather than global modes; domain presets use starter kits, capability packs, and security policies. Finance is the first vertical, targeting small private funds and independent analysts; medical research is a later candidate. See the [M6 plan](plans/M6-projects-and-product-structure.md) for the decision record and scope.

**Project model (planned for internal M6; not started).** Projects become the unit of work inside a workspace, with channels and tasks optionally scoped to one; Home is the unscoped view. The model is settled at the decision level; implementation is not scheduled. See the [M6 plan](plans/M6-projects-and-product-structure.md) for the locked decisions and work breakdown.

Locked decisions:

- A workspace remains the data, tenant, and scheduling boundary; projects are context boundaries, not permission boundaries.
- Home is `project_id = NULL` for channels and tasks. No implicit "General" project is created, and existing rows are not backfilled.
- Agents stay workspace-wide with one direct conversation each; `project_agents` records which agents a project uses, and projects own group channels only.
- Channel names are unique per project; Home is a separate namespace.
- Archiving makes a project read-only without cancelling running work; hard deletion is limited to projects with no tasks or messages.
- Mentions and task assignment reject agents outside the project team; removing a member keeps history and blocks new assignments.
- Code projects bind one repository from the device-local registry, which is excluded from `.cowpkg` export.
- The existing device-local git registry keeps its behavior but is renamed **Repositories**; **Project** means the work unit in this plan.
- Workspace policy sets the enforced security floor; project settings may only tighten it.

**Branded editions (deferred, not started).** Future distribution may build separate installers from the same codebase — for example CowBot Finance or CowBot Medical — each with its own application identity, data directory, update channel, compliance defaults, and support terms. This is deferred until a paying vertical customer requires a distinct procurement identity, update channel, or compliance package. Switching editions inside one installation is not planned; domain adaptation stays at runtime through project-level presets, capability packs, and security policies.

---

## 7. Cross-Cutting Workstreams

- **Quality**: unit and integration tests per milestone; `fmt`, `clippy -D warnings`, and workspace tests in CI.
- **Security**: threat modeling for tool execution and authentication; dependency auditing; sandbox escape review.
- **Documentation**: PRD, architecture, ADRs, and roadmap updated with every milestone; docs remain English-only.
- **Observability**: structured logging and task/run tracing from Phase 1; metrics and cost accounting later.
- **Internationalization**: UI strings externalized for at least English and Chinese; repository artifacts remain English.
- **Git workflow**: GitFlow with `main` as the release branch and `dev` as the integration branch.

---

## 8. GitFlow Workflow

- `main`: stable, release-ready history only.
- `dev`: integration branch for completed features; all feature work merges here first.
- `feature/<name>`: branched from `dev`, merged back into `dev` with `--no-ff`.
- `release/<version>`: branched from `dev` when preparing a release, merged into both `main` and `dev`.
- `hotfix/<name>`: branched from `main` for urgent fixes, merged into both `main` and `dev`.
- Commits are small, atomic, and English-written; pull requests target `dev` unless they are release or hotfix PRs.

---

## 9. Dependencies and Sequencing

```
Phase 0 (done)
   |
   v
1.1 Store --> 1.2 Identity/Workspace --> 1.4 Orchestration --> 1.5 Tools/Approvals --> 1.6 Desktop UI --> 1.7 Packaging
                     |                        ^
                     +--> 1.3 Templates ------+
   |
   v
Phase 2: 2.1 Server/API --> 2.2 Multi-user --> 2.3 Remote client
                 |                |
                 +--> 2.4 Deployment
                 +--> 2.5 Migration
   |
   v
Phase 3 workstreams (parallel, prioritized with users)
```

Critical path: `cowbot-store` → identity/workspace → orchestration → approvals → desktop UI → packaging. Delaying persistence or approvals blocks most later work.

---

## 10. Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Scope creep in Phase 1 | Delays offline MVP | Strict exit criteria; explicitly deferred list; small atomic PRs |
| Sandbox escape or destructive commands | Security incident | Static risk rules first; approval gates; limited working directory; container backend in Phase 3 |
| Model/provider API changes | Broken agent runs | Provider abstraction; OpenAI-compatible baseline; mock provider for tests |
| Tauri or React ecosystem churn | Packaging and UI delays | Pin versions; isolate the UI behind `cowbot-client`; keep the core independent |
| Cross-platform packaging issues | Poor first-run experience | Early CI builds for Windows/macOS/Linux; smoke tests per release |
| Multi-user auth complexity | Phase 2 delays | Opaque sessions and simple RBAC first; OIDC deferred |
| Local/team data divergence | User confusion | Explicit import/export; no bidirectional sync in v1; documented migration path |
| Template supply-chain risk | Malicious templates | No embedded code; tool-reference validation; signing deferred but planned |

---

## 11. Definition of Done (Per Milestone)

A milestone is done when:

1. Acceptance criteria are written and met.
2. Unit and integration tests cover the new behavior.
3. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` pass.
4. Relevant docs and ADRs are updated in English.
5. A demo or reproducible verification path is documented.
6. Changes are merged into `dev` through small, atomic commits and reviewed.

---

## 12. Success Metrics by Phase

### Phase 1

- Install-to-first-task completion under 30 minutes.
- Task completion rate and approval response time.
- Crash-free desktop sessions.
- Successful `.cowpkg` export/import round trips.

### Phase 2

- Self-hosted deployment success rate.
- Number of active workspaces and members per deployment.
- Authentication and authorization failures.
- Audit completeness for approvals and tool calls.

### Phase 3

- Adoption of Docker sandbox and integrations.
- Knowledge retrieval usefulness and memory privacy incidents (target: zero).
- Plugin/MCP ecosystem growth.
- Cost and latency efficiency per task.
