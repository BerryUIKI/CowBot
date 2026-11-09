# CowBot Functional and System Architecture (Draft)

> **Status**: Draft v0.1 (to be confirmed section by section)
> **Date**: 2026-11-03
> **Related**: `docs/PRD.md`, `docs/ADR/0001-runtime-and-deployment.md`

---

## 1. Architecture Principles

1. **One core, multiple runtime forms**: desktop and server share the same Rust domain model and business logic.
2. **Local-first, multi-user-ready**: the platform works without a server by default, yet accounts, workspaces, and permissions exist from v1.
3. **API-first**: the desktop app, CLI, and future integrations all access capabilities through `cowbot-client` or HTTP + WebSocket.
4. **Event-driven**: messages, task state, tool calls, and approvals are broadcast over the event bus for real-time UI and auditing.
5. **Configuration over hard-coding**: roles, tools, templates, and approval policies are configurable; software engineering is not baked in.
6. **Secure by default**: risky operations require approval; the sandbox restricts working directories, timeouts, and output size.
7. **Incremental delivery**: every phase can be released, tested, and rolled back independently.

---

## 2. Runtime and Deployment

```
+---------------------------- Desktop (Tauri) -----------------------------+
|  Web frontend (channels / tasks / agents / approvals / settings)        |
|        | Tauri IPC / HTTP                                               |
|  cowbot-client --+-- Local mode: in-process cowbot-* core + SQLite      |
|               +-- Team mode: HTTP/WebSocket -> cowbot-server            |
+-------------------------------------------------------------------------+

+---------------------------- Self-hosted server --------------------------+
|  cowbot-server (HTTP + WebSocket API, auth, workspaces, permissions)    |
|        |                                                                |
|  cowbot-agent / cowbot-tools / cowbot-llm / cowbot-bus / cowbot-store   |
|        |                                                                |
|  SQLite (default) or PostgreSQL (P1)                                    |
+-------------------------------------------------------------------------+

CLI (cowbot) -- cowbot-client -- local mode or remote server
```

- The desktop app runs in local mode by default and needs no server; switching to team mode only changes the `cowbot-client` implementation and connection settings.
- The server supports multiple users from v1; local mode auto-creates a single-user workspace, but the data model matches the server.

### 2.1 Implementation Order

1. **Local/offline first**: desktop app + core + SQLite + templates + approvals + tool sandbox.
2. **Self-hosted team mode**: `cowbot-server`, multi-user accounts, invitations, API tokens, remote client.
3. **Extensions**: Docker sandbox, IM/Webhooks, long-term memory, PostgreSQL, template marketplace.

---

## 3. Functional Domains

### 3.1 Identity and Workspace

- Entities: `User`, `Workspace`, `Membership`, `Role` (Owner / Admin / Member).
- Local mode: first launch auto-creates a local Owner and a default workspace.
- Team mode: sign-up/sign-in, workspace invitations, member role management, API tokens.
- v1 authentication: local accounts (Argon2id), opaque server session tokens, API tokens; OIDC/SSO is P1.

### 3.2 Collaboration

- Entities: `Channel` and `Message` (threads via `ChannelMessage::thread_id`). `Channel::agent_id` marks a direct conversation owned by one agent; `null` is a group channel (ADR-0024). `Mention` is parsed from message text rather than stored (ADR-0022); `TaskCard` and `ApprovalCard` remain unmodelled.
- Humans and agents are both channel members; messages are persisted and shown on a timeline.
- A message that mentions an agent (`@Name`, or `@team`/`@all`/`@everyone`/`@here`) becomes one task per mentioned agent, run through the assignment scheduler with the channel's approval policy; the answer is written back to the channel as a message from that agent. Unknown mentions, failures, and empty answers produce a system notice (ADR-0022).
- An agent's direct conversation is created on demand and reconciled at desktop startup: every message there is addressed to that agent, so mentions are not required and never fan out. Group channels keep the mention rules above (ADR-0024).
- Channel names are unique per workspace, case-insensitively, and are validated in the service so existing duplicates remain editable. Channels and conversations carry a `pinned` flag; group channels can be renamed or deleted, while direct conversations can only have their history cleared (ADR-0026). `ChannelUpdated` and `ChannelDeleted` events keep clients in sync.
- Channels are the primary collaboration surface but do not restrict usage: users can create channels and threads freely.
- `cowbot-service` provides the local bootstrap (idempotent owner and default workspace), channel/message services that persist writes before publishing `ChannelCreated`, `ChannelUpdated`, `ChannelDeleted`, and `MessagePosted` events, channel name validation and management (rename, pin, delete, clear history), idempotent provisioning of agent direct conversations, and the channel agent service that turns mentions or direct messages into assignment-backed turns.

### 3.3 Agents and Templates

- `AgentProfile`: name, role, goal, backstory, model, tool set, iteration limit, delegation flag.
- Built-in template library: role templates and team templates (e.g., development, research, writing teams) selectable at creation time.
- Templates can be customized, imported, and exported; they are a starting point, not a fixed scenario.
- Template format is JSON (`schema_version` / `kind` / `agents` / `tasks`); built-in templates are compiled into the binary, user templates are stored in SQLite, and exports use `.cowtemplate`.
- Templates only reference tool names and model configuration; they never embed executable code. v1 has no online template marketplace.
- `cowbot-service::TemplateService` loads the compiled-in templates, validates documents (schema version, references, cycles), persists user templates, and imports/exports `.cowtemplate` files. Template format details live in `docs/TEMPLATE_FORMAT.md`.

### 3.4 Tasks and Orchestration

- Phase 1.4 introduces an assignment runtime (ADR-0006): each unit of agent work is an assignment record with `root_id` / `parent_id` / `depth`, origin, requester, assignee, mode, reply, status, execution/restart counters, and approval level.
- Assignment lifecycle: `queued -> running -> waiting -> resume -> needs_input -> completed / failed / cancelled`. Every transition writes an append-only event in the same transaction.
- Task status is a projection of the root assignment lifecycle: `pending -> in_progress -> completed / failed / cancelled`. It is updated in the same service calls as the assignment transition.
- v1 scope (confirmed): single-agent tasks plus a sequential crew on top of assignments; a scheduler that serializes each root and runs independent roots concurrently; per-root request/execution/wake budgets; stall guard; restart reconciliation.
- Implementation status (Phase 1.4): the lifecycle, event log, stall guard, restart reconciliation, and budget checks are wired into `cowbot-service`. `AssignmentService` enforces the request budget in `enqueue_child`, the execution budget in `start`, and the wake budget in `wake_parent`. `cowbot-service::AssignmentScheduler` (ADR-0019) is the single dispatch path: it consumes `queued` roots and `resume` parents, runs independent roots concurrently up to a configurable limit, serializes the assignments of one root, injects completed dependency and child outputs, and fails dependents whose prerequisite did not complete. `CrewRunner` registers the crew's agents, enqueues the tasks, and runs the scheduler until idle. Every attempt records one `Run` with `Completed` or `Failed` status and one `ToolCallRecord` per executed tool call, including denied calls.
- Later: hierarchical orchestration (manager agent delegation) and DAG workflows. A process-wide resident scheduler arrives with the server phase; the local scheduler lives for one crew run.
- Tasks support prerequisite dependencies (`context_tasks`); prior deliverables are injected into downstream context.
- Phase 1.4 stores the effective approval level on the assignment; inheritance (`max(own, inherited)`) and enforcement are implemented in Phase 1.5.

### 3.5 Tools and Sandbox

- `Tool` trait and `ToolRegistry`: name, description, parameter schema, async execution.
- Built-in tools: code execution, file read, file write; later: network access, retrieval, browser, and more.
- `SandboxBackend`: local subprocess in v1; Docker in P1; WASM evaluated in P2.
- Resource limits: timeout, output truncation, working-directory restriction, environment-variable filtering.
- Static risk classifier (`StaticRiskClassifier`): grades every tool call as `ReadOnly`, `Low`, `Medium`, `High`, or `Critical` using deterministic rules; unknown tools default to `High`.
- Approval gate: `ToolGate` is consulted before every tool execution. `cowbot-service::ApprovalService` applies the assignment's approval level, creates a pending approval when required, waits for a human decision, and records audit events. Only one decision is accepted per approval: `decide_approval` updates pending rows exclusively, so concurrent reviewers cannot overwrite each other. When the wait times out the approval is persisted as `expired` and the tool call is denied. Approvals left pending by a previous process are expired on startup by `ApprovalService::reconcile_on_startup`.

### 3.6 Model Gateway

- `LlmProvider` abstraction; v1 provides OpenAI Chat Completions, Anthropic Messages, and OpenAI Responses clients (cloud APIs, relays, DeepSeek, Ollama, LocalAI, Codex-style endpoints) plus a Mock provider.
- `LlmProvider::list_models` queries the endpoint's catalogue (`/models` for the OpenAI-style protocols, `/v1/models` for Anthropic); the desktop's vendor-first provider editor uses it to populate a provider's model list (ADR-0021).
- Model configuration is workspace- or agent-scoped, with a connection test.
- P1 adds streaming, retries/backoff, rate limiting, and token/cost accounting.

### 3.7 Memory

- v1 (confirmed): session context plus channel/task history as the short-term memory source.
- P1: project-level long-term memory and knowledge base (retrieval-augmented), with explicit storage and privacy boundaries.
- Agent memory must never leak across workspaces.

### 3.8 Persistence and Audit

- `CowStore` abstraction; v1 implements SQLite (WAL); PostgreSQL is deferred to the server phase (P1).
- Stored entities: users, workspaces, members, channels, messages, agents, templates, tasks, assignments, assignment events, runs, tool calls, approvals, and audit events.
- The crew runner records a `Run` per task attempt and a `ToolCallRecord` per tool call, including its arguments, result, and — for human-approved calls — the deciding actor in `tool_calls.approved_by`, so the desktop UI can read per-run tool history.
- Audit events record who approved or rejected what, and which tools an agent executed. Approval decisions are audited with tool name, call ID, risk level, matched rule, and arguments. Expired approvals are persisted with status `expired` so stale approval cards cannot reappear after a restart; the startup sweep covers rows left behind when the waiting process died.

### 3.9 Access Layer

- **Client abstraction**: `cowbot-client` exposes a transport-neutral `Client` trait. `LocalClient` calls the core in-process, bootstraps the local workspace, and runs startup reconciliation when it opens; the remote implementation arrives in Phase 2.3.
- **Desktop UI**: Tauri v2 shell (`desktop/src-tauri`) with a React + TypeScript frontend (`desktop/`). It covers first-run onboarding, local session bootstrap, a conversation-first shell (a searchable sidebar of agent contacts with last-message previews, group channels below them, and footer navigation to tasks, approvals, and settings), a bubble chat timeline with a typing indicator and a floating composer, tasks (list, creation, detail with assignment/run history, cancellation, and desktop-initiated execution), approvals (list and decide), and a sectioned settings page for model providers, agents, templates, workspace transfer, and version/update controls. Every agent gets a direct conversation, provisioned at startup and after adding agents, in which messages reach that agent without a mention (ADR-0024); group channels keep mention-driven replies with a per-channel approval policy (ADR-0022). Workspaces live one per local database; Settings → Workspace creates empty workspaces, exports the active workspace as `.cowpkg`, imports a package into a new database, and switches between registered databases by restarting the app (ADR-0017). Providers record their protocol — OpenAI Chat Completions, Anthropic Messages, or OpenAI Responses — and are stored in a local `config.json`; they are configured from a vendor-first catalogue, with the protocol as a form field, the endpoint's model list available through Fetch models, and several models per provider with one active model (ADR-0012, ADR-0016, ADR-0021). Providers can be added, edited, tested, and switched without restarting, and onboarding records its completion in the same file (ADR-0013). When enabled, the desktop checks GitHub Releases at launch and installs signed updates through the Tauri updater plugin; installing is refused while a crew runs (ADR-0018). All workspace data access goes through `cowbot-client` via Tauri commands, never directly through `cowbot-store` or `cowbot-service`; model provider configuration is desktop runtime state owned by the Tauri backend. Release installers are built by the release workflow (ADR-0014).
- **HTTP/WebSocket API**: `cowbot-server` serves desktop team mode, the CLI, and a future browser entry point.
- **CLI**: `cowbot` initializes and runs the local runtime through `cowbot-client`: `init` prepares the database and workspace; `run` executes the crew through the assignment scheduler with persisted history, `--channel`, `--concurrency`, and interactive approval prompts; `channel` and `message` list/create channels and list/post messages, and `message post --wait` lets the mentioned agents answer before the command exits. Settings resolve from `--config`, `COWBOT_CONFIG`, or `cowbot.toml`, with command-line and environment values taking precedence (ADR-0020). Template-driven crews are not wired into the CLI yet.
- **Webhooks / IM** (P1): Mattermost, Slack, Feishu, and others.

---

## 4. Core Flows

### 4.1 From Channel to Deliverable

1. A user posts a channel message that mentions one or more agents, or creates a task in the Tasks view.
2. The orchestrator creates an assignment (root/parent, requester, assignee) and the scheduler dispatches it: one assignment per root at a time, independent roots concurrently. A channel turn uses a scoped scheduler that claims only the roots it created (ADR-0023).
3. An agent calls the LLM and produces text or tool-call requests.
4. The approval policy decides: auto-approve or create an approval card.
5. Tools execute in the sandbox; results are written back to the task context.
6. The task completes; the channel turn posts the output back to the channel as a message from the agent, and deliverables, assignment events, and audit records are persisted.

### 4.2 Approval Policy (Multiple Modes)

Inspired by Claude Code's permission modes:

| Mode | Behavior |
| --- | --- |
| **Ask (default)** | ReadOnly auto-approves; Low and above require approval; Critical is denied |
| **SafeAuto** | ReadOnly/Low auto-approve; Medium and above require approval; Critical is denied |
| **PlanOnly** | Only ReadOnly is allowed; writes and executions are denied |
| **Trusted** | Everything except Critical auto-approves; Critical remains denied in v1 |

- Rule priority: workspace policy > agent policy > global default.
- Assignment inheritance: delegated work uses `inherit_approval_level(parent, requested)`; `PlanOnly` is a hard read-only boundary, otherwise the more permissive level wins (ADR-0006, implemented in Phase 1.5).
- Fine-grained rules: per tool, command prefix, path allowlist/denylist.
- Risk levels: `ReadOnly` / `Low` / `Medium` / `High` / `Critical`.
- Decision order: static rules first (tool, command pattern, path, network, destructive operations); an optional LLM classifier may handle unmatched operations and can only raise risk, never lower the static floor; undecidable operations default to `High`.
- Decision record: rule ID / classifier / human, together with the approver, written to the audit log.

### 4.3 Local Mode vs. Team Mode

- **Local mode**: single user, in-process core, SQLite; no network or server required.
- **Team mode**: multi-user, remote `cowbot-server`, shared workspaces; the desktop app only renders and interacts.
- Both modes share the domain model and UI; differences are encapsulated in `cowbot-client` and the auth layer.
- **Data exchange (v1)**: the two modes keep independent data and migrate via import/export (workspace packages, template packages, task archives); no bidirectional sync.

---

## 5. Logical Data Model (Draft)

| Entity | Key Fields |
| --- | --- |
| `User` | id, name, email, password_hash, created_at |
| `Workspace` | id, name, created_at |
| `Membership` | user_id, workspace_id, role |
| `Session` | id, user_id, token_hash, expires_at, created_at |
| `ApiToken` | id, user_id, workspace_id, token_hash, scopes, revoked_at |
| `Invite` | id, workspace_id, token_hash, role, expires_at |
| `Channel` | id, workspace_id, name, topic, created_at, agent_id (direct conversation owner), pinned |
| `Message` | id, channel_id, sender, content, thread_id, created_at |
| `AgentProfile` | id, workspace_id, name, role, goal, backstory, model, tools, limits |
| `Template` | id, kind (agent/team), name, payload |
| `Task` | id, workspace_id, channel_id, description, expected_output, assignee, status, context_tasks |
| `Assignment` | id, root_id, parent_id, depth, task_id, origin, requester, assignee, mode, reply, status, executions, restarts, approval_level, created_at, updated_at |
| `AssignmentEvent` | id, assignment_id, kind, detail, created_at |
| `Run` | id, task_id, agent_id, status, started_at, finished_at |
| `ToolCall` | id, run_id, tool_name, arguments, result, approved_by |
| `Approval` | id, task_id, tool_call_id, status (`pending` / `approved` / `rejected` / `expired`), decided_by, decided_at |
| `AuditEvent` | id, workspace_id, actor, action, target, created_at |

> Final field definitions will follow the `cowbot-store` migrations.

---

## 6. Crate Map and Plan

| Crate | Responsibility | Status |
| --- | --- | --- |
| `cowbot-core` | Domain model, errors, events, `Tool` trait | Existing |
| `cowbot-bus` | Event bus and subscriptions | Existing |
| `cowbot-tools` | Tool registry, sandbox backends, and static risk classification | Existing (subprocess) |
| `cowbot-llm` | Model providers and types | Existing (OpenAI, Anthropic, Responses + Mock; model listing) |
| `cowbot-agent` | Agent loop and crew orchestration | Existing (sequential) |
| `cowbot-cli` | `cowbot` CLI | Existing (local runtime, `cowbot.toml`, channels/messages) |
| `cowbot-store` | Persistence abstraction and SQLite/Postgres implementations | Existing (SQLite) |
| `cowbot-service` | Local bootstrap, channel/message services, assignment runtime and scheduler, approval service, crew runner, template management, and event wiring | Existing |
| `cowbot-server` | HTTP/WebSocket API, auth, multi-user | Planned |
| `cowbot-client` | Unified local/remote client abstraction | Existing (local mode) |
| `desktop/` | Tauri v2 shell + React/TypeScript frontend | Existing (channels, tasks, approvals, settings, model providers) |
| `cowbot-package` | `.cowpkg` / `.cowtemplate` import/export | Existing |
| `cowbot-desktop` | Tauri shell and frontend integration | Existing (`desktop/src-tauri`) |
| Frontend app | React + TypeScript (channels, tasks, approvals, agent management UI) | Existing (`desktop/`) |

---

## 7. Permissions and Approval Model

- **Workspace roles**: Owner (full control), Admin (members/agents/policy management), Member (channels and tasks).
- **Agent permissions**: allowed tool set, allowed models, delegation flag, maximum iterations.
- **Approval decisions**: record approver, time, mode, and rule basis in the audit log.
- **Sensitive operations**: file writes/deletes, command execution, network access, cross-workspace access.

---

## 8. Non-Functional Requirements

- **Security**: deny risky operations by default; encrypt credentials at rest; least privilege; auditable actions.
- **Performance**: local-mode cold start < 2s; real-time channel messages; low-latency task events.
- **Reliability**: recoverable task crashes; transactional SQLite writes; horizontally scalable server (P1).
- **Extensibility**: tools, models, templates, and access methods all extend through interfaces.
- **Observability**: structured logs, task/run tracing, cost accounting (P1).
- **Internationalization**: UI strings localizable (at least English and Chinese).

---

## 9. Confirmed Decisions and Open Questions

### 9.1 Confirmed (2026-11-03)

1. **Orchestration scope**: v1 = single agent + sequential crew; hierarchical manager and DAG are P1.
2. **Memory scope**: v1 = session + channel/task history; project-level long-term memory is P1.
3. **Frontend stack**: React + TypeScript (Tauri WebView).
4. **Local/team modes**: independent data, import/export only, no bidirectional sync.
5. **Templates**: JSON, built-in templates compiled into the binary, user templates import/export as `.cowtemplate`; no online marketplace in v1.
6. **Risk determination**: static-rule-first (ReadOnly–Critical), optional LLM classifier can only raise risk; unknown operations default to High.
7. **Import/export**: `.cowpkg` (ZIP + manifest + JSON/JSONL); no secrets, password hashes, or tokens.
8. **Authentication**: local accounts + Argon2id + opaque session tokens + API tokens; OIDC/SSO is P1.
9. **PostgreSQL**: deferred to the server phase; local/offline functionality comes first.

### 9.2 Open Questions

1. Data-at-rest encryption design.
2. Online template marketplace and template signing (P2).
3. TLS termination and reverse-proxy deployment details.
4. Invitation and notification channels (email / IM).

---

## 10. Design References

External projects and the patterns CowBot adopts, defers, or rejects are recorded in `docs/REFERENCES.md` (CrewAI, Mattermost, and OpenMausBot). The most immediate candidate is a simplified assignment runtime for Phase 1.4, described there.
