# CowBot Product Requirements Document (PRD)

> **Status**: Draft v0.1
> **Date**: 2026-11-03
> **Scope**: This document defines the product positioning, target users, core scenarios, and MVP scope. Functional and system architecture live in `docs/ARCHITECTURE.md`; decisions are recorded in `docs/ADR/`.

---

## 1. Background and Problem

Today's AI agent products fall into three broad categories, each with a clear gap:

1. **Single-agent coding tools** (e.g., terminal AI assistants): great for individual productivity, but there is only one agent, so role division and collaborative workflows never emerge.
2. **Multi-agent orchestration frameworks** (e.g., CrewAI, LangGraph): powerful, but code-centric, with no human-facing collaboration surface, approvals, or audit trail.
3. **Team collaboration tools** (e.g., Mattermost, Slack): channels, threads, and human collaboration, but agents are not first-class members.

At the same time, more individual developers and small teams want to:

- run an AI team on their own machine or private server, with data staying inside their network;
- choose models freely (cloud APIs or local models);
- have multiple agents collaborate like colleagues rather than calling them as one-shot functions;
- start with software development, but later extend to research, writing, data analysis, and other general work.

CowBot fills exactly this gap: **a self-hosted, channel-native multi-agent team platform.**

---

## 2. Product Positioning

### 2.1 One-Sentence Positioning

> CowBot is a self-hosted, channel-native multi-agent team platform that lets individuals and small teams direct their AI colleagues the way they would lead a team — starting with software development, but not limited to it.

### 2.2 Target Users

| Priority | User | Typical Need |
| --- | --- | --- |
| **Primary** | Individual developers, tech leads, independent creators | Build a customizable AI development team on their own machine or private server, with control over data and models |
| **Secondary** | Small technical teams (3–20 people) | Share agents, tasks, and channels, with an auditable human-AI collaboration process |
| **Expansion** | General knowledge workers | Reuse the same platform for research, writing, data analysis, and other non-coding work through custom roles, tools, and channels |

**Key constraint**: the platform must not hard-code software engineering. Roles, tools, workflows, and channels must all be configurable; software engineering is only the first set of built-in templates and best practices.

**Multi-user by design**: v1 is designed for multiple users from the first release. Even when the desktop app runs in local single-user mode, accounts, workspace membership, and permissions must exist from the start to avoid a later rewrite.

### 2.3 Core Scenarios

1. **An individual builds an AI development team**
   One person configures planner, coder, and reviewer roles, and lets them split requirement breakdown, coding, testing, and review work inside a channel.

2. **Human-AI collaboration in channels**
   Humans and multiple agents share context in the same channel or thread, coordinating through @mentions, task cards, and approval requests.

3. **General work expansion (no fixed scenarios)**
   The platform provides only generic primitives: workspaces, channels, agents, tasks, and tools. Users compose their own scenarios through custom roles, tools, and templates; no fixed workflow is built in.

### 2.4 Value Proposition

- **Collaborate like colleagues, not like tools**: channels, threads, tasks, and approvals are first-class.
- **Self-hosted, data-sovereign**: deploy on a workstation or private network; data stays on your infrastructure.
- **Model-neutral**: works with OpenAI Chat Completions, Anthropic Messages, and OpenAI Responses endpoints, including cloud APIs and local models, chosen per task.
- **Verifiable execution**: agent code runs for real in a sandbox, and results are inspectable.
- **Open and extensible**: agent roles, tools, workflows, and access methods are all extensible.

### 2.5 Differentiation: Channel-Native Human-AI Collaboration

CowBot's core differentiation is not "a stronger single agent", but **putting multiple agents and humans in the same collaboration space**:

- Channels and threads are the primary interface; humans and agents are both channel members. Every agent also has a direct conversation where messages reach it without a mention; group channels keep mention-based addressing (ADR-0024). Users can also create single-agent conversation sessions with an independent, visible model context and manual or automatic compaction (ADR-0030).
- Agents are addressed via @mention in a group channel and answer in that channel; in an agent's direct conversation every message is addressed to that agent. Proactive task claiming by role is not implemented in 0.1.x.
- Tasks, reasoning, tool calls, approvals, and deliverables all live in the channel timeline, traceable and auditable.
- Humans can step in at any time to correct, approve, or take over.
- Collaboration context is shared within the channel instead of being scattered across separate sessions.

### 2.6 Non-Goals (Explicitly Out of Scope for v1)

- Public multi-tenant SaaS.
- A no-code general workflow builder.
- A closed tool that only serves software engineering.
- A replacement for a full IM or office suite.
- Supporting every model and tool ecosystem at once.

### 2.7 Deployment and Runtime Forms

CowBot follows a "single core, two forms" model:

| Form | Description | Best For |
| --- | --- | --- |
| Desktop app (primary entry) | Tauri shell with an embedded core and local SQLite; offline single-machine use by default, with an option to connect to a remote team server | Individual developers, daily use |
| Self-hosted server | Headless service with HTTP/WebSocket APIs; Docker and single-binary deployment | Small teams, enterprise intranets, multi-user collaboration |

- The desktop app runs in **local mode** by default (no server needed) and can connect to a server in **team mode** for multi-user collaboration.
- The server supports multiple users, workspace membership, and role-based permissions from v1.
- Desktop and server share the same Rust core libraries, domain model, and API contract.

---

## 3. User Personas

### 3.1 Primary Persona: Independent Tech Lead

- 5+ years of development experience; comfortable with the command line, Docker, and API keys.
- Owns a server or a high-performance workstation.
- Wants AI to take over repetitive engineering work, with controllable processes and verifiable results.
- Cares about privacy and cost; prefers self-hosting and local models.

### 3.2 Secondary Persona: Small Technical Team

- 3–20 people using a private deployment or intranet service.
- Needs shared agent configurations, task visibility, and collaboration records.
- Cares about permissions, auditability, and knowledge retention.

### 3.3 Expansion Persona: General Knowledge Worker

- Does not write code, but wants to configure agents for research, writing, and data organization.
- Needs low-friction role/tool configuration and clear channel interactions.

---

## 4. Core User Stories

1. As an individual developer, I want to deploy CowBot on a private server in one step and create my first workspace in the desktop console.
2. As a tech lead, I want to create planner / coder / reviewer agents and configure their models and tools.
3. As a user, I want to submit a requirement to the agent team in a channel and see it broken down, assigned, and executed.
4. As a user, I want agents to ask for my approval before risky operations such as deleting files or running unknown commands.
5. As a user, I want agents to run code and tests in a sandbox and attach the results to the task deliverable.
6. As a user, I want to use the same platform for non-coding work, such as having an agent research a topic and produce a report.
7. As a team administrator, I want to review task history, tool calls, and approval records within a channel.

---

## 5. MVP Scope

### 5.1 P0 (First Usable Release)

| Area | Content |
| --- | --- |
| Runtime forms | Desktop app (Tauri, local mode) + self-hosted server (Docker/single binary, team mode) |
| Multi-user and permissions | Sign-up/sign-in, workspace membership, admin/member roles, API tokens; desktop and server share one account model |
| Agent template library | Built-in JSON role/team templates compiled into the app; custom templates and `.cowtemplate` import/export |
| Collaboration space | Workspaces, channels, threads, message timeline; @Agent assignment; task cards and approval cards |
| Agent management | Create/edit agents (role, goal, backstory, model, tools, iteration limit) |
| Tasks and orchestration | Task creation, assignment runtime (lifecycle, event log, budgets, restart reconciliation), sequential crew execution, deliverables, run and tool-call history, and status transitions |
| Tools and sandbox | Code execution, file read/write; timeouts, output truncation, working-directory limits |
| Approval policy | Five modes (Auto / Manual / Accept edits / Plan / Bypass permissions); static risk levels (ReadOnly–Critical); optional LLM classifier that can only raise risk; tool/command/path allowlists |
| Model configuration | OpenAI Chat Completions, Anthropic Messages, and OpenAI Responses endpoints (cloud APIs and local models), protocol selection, connection test |
| Persistence | SQLite (WAL) for workspaces, channels, messages, tasks, agent configuration, and approval records |
| CLI | `cowbot` retained for initialization, diagnostics, headless runs, and debugging |

### 5.2 P1 (Enhancements)

- Hierarchical orchestration (manager agent delegation) and DAG workflows.
- OIDC/SSO and enterprise directory integration; finer-grained permissions.
- PostgreSQL storage backend.
- Optional browser access.
- Docker sandbox backend.
- Webhooks and Mattermost / Slack / Feishu integrations.
- Long-term agent memory and knowledge base.
- Streaming responses, retries, rate limiting, and cost/billing views (per-call token usage is recorded, ADR-0029).
- Audit log and operation replay.

### 5.3 P2 (Ecosystem)

- Plugin mechanism and MCP compatibility.
- Agent/workflow template marketplace.
- Lightweight WASM sandbox.
- Multi-workspace and organization-level management.
- Mobile or additional desktop clients.

---

## 6. Success Metrics

- **Onboarding efficiency**: under 30 minutes from deployment to the first agent team completing a task.
- **Task effectiveness**: task success rate, human intervention rate, rework rate.
- **Collaboration activity**: channel messages, tasks created, approval response time.
- **Stability**: self-hosted deployment success rate, service availability.
- **Community health**: GitHub stars, issue response time, contributor count.

---

## 7. Confirmed Decisions and Open Questions

### 7.1 Confirmed (2026-11-03)

1. v1 is multi-user by design from the first release.
2. Desktop console form factor: desktop shell (Tauri).
3. Deployment: desktop app (optionally serverless) plus self-hosted server (Docker/single binary).
4. General work has no fixed scenarios; only configurable primitives and templates are provided, preserving freedom.
5. Approvals offer five modes (Manual by default, auto, accept edits, plan, bypass) with fine-grained rules.
6. Built-in agent and team template library.
7. v1 orchestration scope: single agent + sequential crew; hierarchical manager and DAG are P1.
8. v1 memory scope: session plus channel/task history; project-level long-term memory is P1.
9. Desktop frontend stack: React + TypeScript.
10. Local and team modes keep separate data; migration is import/export only, with no bidirectional sync.
11. Templates are JSON, with built-in templates compiled into the app; user templates import/export as `.cowtemplate`; no online marketplace in v1.
12. Risk determination is static-rule-first (ReadOnly / Low / Medium / High / Critical); an optional LLM classifier may only raise risk; unknown operations default to High.
13. Import/export uses `.cowpkg` (ZIP + manifest.json + JSON/JSONL) and never includes secrets, password hashes, or tokens.
14. Authentication: local accounts + Argon2id + opaque server session tokens + API tokens; OIDC/SSO is P1.
15. PostgreSQL is deferred to the server phase; local/offline functionality comes first.

### 7.2 Planned Direction (2026-11-10)

1. Work and Code are project profiles, not global application modes; a project chooses its profile.
2. Workspaces remain the data, tenant, and scheduling boundary. Projects are the unit of work inside a workspace; Home is the unassigned view.
3. Agents remain workspace-wide with one direct conversation each; projects track team membership and own group channels.
4. Audience adaptation uses starter kits, capability packs, and security policies, not separate applications. A workspace policy is the enforced security floor; projects may only tighten it.
5. Finance is the first vertical target, aimed at small private funds, family offices, and independent analysts. It is a research/analysis workspace, not investment advice or trading.
6. Medical research and medical writing are a later vertical; clinical diagnosis, treatment recommendations, and clinical decision support are out of scope.
7. Branded editions are deferred. There is no runtime edition switching; separate installers, if ever needed, are built from the same codebase.
8. The project model is planned for internal M6 and is not implemented. See the [M6 plan](plans/M6-projects-and-product-structure.md).

### 7.3 Open Questions

1. Data-at-rest encryption design.
2. Online template marketplace and template signing (P2).
3. TLS termination and reverse-proxy deployment details.
4. Invitation and notification channels (email / IM).
