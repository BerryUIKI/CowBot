# ADR 0028: Main Assistant and Desktop Workspace Actions

- **Status**: Accepted
- **Date**: 2026-11-09
- **Related**: `docs/ADR/0024-agent-direct-conversations.md`, `docs/ADR/0026-channel-and-conversation-management.md`, `docs/ADR/0027-rich-agent-output.md`, `docs/ADR/0003-approval-policy.md`, `docs/USER_GUIDE.md`

## Context

CowBot's product direction is the one-person company: the user works with a primary assistant that decomposes goals, coordinates specialist agents, and keeps the workspace organized. In 0.1.5 the desktop had a generic "Assistant" agent with file and code tools only. Asked to create a channel, it suggested Slack, Discord, Teams, or Feishu because nothing in its prompt said it was running inside CowBot, and it had no way to change the workspace even if it had understood the request.

The runtime already has the pieces needed to let an agent act without bypassing policy: a `Tool` trait and registry, an assignment-backed tool gate with static risk levels and approvals, a client boundary that supports channel and task creation, and template instantiation. The missing layer was desktop workspace actions and a product identity for the primary agent.

## Decision

### 1. The built-in assistant becomes the Main Assistant

`general-assistant.cowtemplate.json` becomes **Main Assistant**:

- Agent `main`, role `Primary Assistant`, an OPC-oriented goal and backstory (turn goals into work, delegate to specialists, report next steps).
- Tools: `read_file`, `write_file`, `execute_code`, `list_agents`, `list_templates`, `create_channel`, `create_task`, `instantiate_team`.
- It is listed first so onboarding and the Settings template default select it.

Existing workspaces are not migrated automatically. The agent editor lets the user rename the old Assistant to `main`, set its role, and enable the workspace tools; adding the Main Assistant template is the alternative for a fresh primary agent.

### 2. Agent profiles are editable, and the conversation follows

`cowbot-service::AgentService::update_agent` validates a non-empty, workspace-unique (case-insensitive) name, trims the role, de-duplicates the tool list, persists the profile through `upsert_agent`, reconciles the agent's direct conversation name/topic through `ChannelService::ensure_agent_channel`, and publishes `AgentUpdated` plus the resulting `ChannelUpdated`.

The desktop exposes the same editor from Settings → Team and from an agent conversation's context menu. Names are unique because mentions and task assignment resolve agents by name; old message text is not rewritten.

### 3. Workspace actions are desktop-side tools

`desktop/src-tauri/src/workspace_tools.rs` implements five `cowbot_core::tool::Tool` values backed by `Arc<LocalClient>`:

| Tool | Risk | Effect |
| --- | --- | --- |
| `list_agents` | ReadOnly | Returns agent ids, names, roles, and tools |
| `list_templates` | ReadOnly | Returns template ids, names, kinds, and agent names |
| `create_channel` | Medium | Creates a group channel with a unique name |
| `create_task` | Medium | Queues a pending task for a named agent, optionally linked to a channel |
| `instantiate_team` | High | Adds the agents from a template and provisions their conversations |

`AppState::build_tools` registers them alongside the sandbox and file tools. Only agents whose profile lists a tool receive its definition, so the Main Assistant gets them while a specialist keeps its narrower set. The agents still run through the same assignment scheduler, approval gate, and tool-call recording as any other task.

The tools live in the desktop crate because the client depends on the tools crate; adding them to `cowbot-tools` would require a cycle or a new workspace-action abstraction. They are therefore desktop-only for now; the CLI's channel turns do not expose them.

### 4. Risk and approvals stay explicit

`StaticRiskClassifier` gains static rules for the new tools: reads are ReadOnly, channel/task creation is Medium, and adding agents is High. Under **Ask** and **Safe auto**, Medium and High calls wait for approval; **Trusted** auto-approves everything except Critical, and **Plan only** denies them. The approval card now shows the live tool name, its JSON arguments, and the risk level/reason, cached from `ToolCallRequested` and `ApprovalRequested` events; approvals that predate the app session fall back to the tool-call id.

### 5. Channel turns carry CowBot and OPC context

`ChannelAgentService::platform_context` adds a shared environment block to every channel/direct turn:

- The agent runs inside CowBot, a local desktop workspace for a one-person company; the user is the operator.
- The current agent roster (up to twenty names and roles).
- Which workspace actions exist and that deleting channels or agents is a UI action.
- Markdown for normal answers; a fenced `html` document for interactive artifacts, sandboxed without network or a chat bridge (ADR-0027).
- Reply in the language of the latest message; do not invent external products; if an action has no tool, explain the matching CowBot UI action.

### 6. Delegation is queued, not executed

`create_task` creates the task and its root assignment but does not run the assignee. Running a single assigned agent outside a channel turn, and hierarchical orchestration, remain future work. Main tells the user the task is queued in **Tasks**.

## Alternatives considered

- **Auto-migrate every existing Assistant to main**: convenient but silently rewrites user data, and a workspace may intentionally keep several assistants. The explicit editor is safer.
- **Let the agent call services directly from the prompt**: bypasses the tool gate, approvals, and run history. Rejected.
- **Put workspace tools in `cowbot-tools`**: cleaner crate layering in theory, but the crate would have to depend on `cowbot-client` (a cycle) or grow a workspace-action trait that only one implementation uses today. Desktop-side tools keep the change contained; a shared abstraction can come with remote mode.
- **Auto-run delegated tasks**: would turn every `create_task` into a nested agent run with its own approvals, budgets, and failure modes. That is the hierarchical orchestration milestone, not Phase 1 polish.
- **Give every agent every tool**: simpler registry handling, but removes the per-agent tool selection that makes specialist agents predictable and approvals meaningful.

## Consequences

- The primary assistant can create channels, start teams, and queue work, with the same audit trail as any other agent action.
- Users see what they approve: tool name, arguments, risk, and reason for live requests.
- Workspace tools are not available in the CLI or remote mode yet.
- Existing agents need an explicit edit to gain the new tools; their conversations follow renames.
- Agent renames do not rewrite historical message text, and task assignment uses the current name.
- Delegated work is queued, not executed; the user still runs tasks from **Tasks**.
- Approval details for pending approvals created before the app started are limited to the tool-call id.
- Coverage: `crates/cowbot-service/tests/agents.rs` (rename sync, uniqueness, blank names), `crates/cowbot-client/src/local.rs` (client update), `crates/cowbot-tools/src/risk.rs` (static levels), `desktop/src-tauri/src/workspace_tools.rs` (create channel/task, team instantiation, unknown assignee, listings), and the channel-agent tests for the context block.
