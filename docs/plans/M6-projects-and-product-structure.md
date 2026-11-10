# M6 — Projects and product structure

- Status: Planned (not started)
- Date: 2026-11-10
- Related: [ROADMAP 6.2](../ROADMAP.md#62-deferred-product-directions-not-started), [ADR-0024](../ADR/0024-agent-direct-conversations.md), [ADR-0026](../ADR/0026-channel-and-conversation-management.md), [ADR-0028](../ADR/0028-main-assistant-and-workspace-actions.md), [ADR-0030](../ADR/0030-conversation-sessions-and-context.md)
- Follow-ups: finance starter kits and security policy, Code profile tools, medical research kit

## Goal

Make projects the unit of work inside a workspace. A user keeps a general Home, creates focused projects with their own group channels, tasks, team, and (for Code projects) a local folder, while agents, approvals, audit, and scheduling remain workspace-wide.

This plan records decisions taken during product planning on 2026-11-10. Nothing here is implemented. Items under [Open at implementation](#open-at-implementation) are implementation details, not open product decisions.

## Product structure

CowBot adapts to different audiences through layers, not through separate applications or a global mode switch.

| Layer | Decides | Scope |
| --- | --- | --- |
| Profile | Interaction structure and execution environment | Per project |
| Starter kit | Default team, roles, tools, example tasks, deliverables | Per project |
| Pack | Capabilities, connectors, permissions | Per project |
| Policy | Security and compliance boundaries | Workspace floor; project may tighten |
| Edition | Brand, installer identity, update channel, commercial terms | Build time; deferred |

### Profiles

- **Work** is the default: channels, conversations, tasks, approvals, deliverables, and general knowledge work.
- **Code** adds a bound local folder, repository-aware tools, diff review, terminal, tests, and checkpoint/rollback. It is not an IDE: completion, debugging, and a full LSP stack are out of scope.
- A profile is chosen per project. There is no global Work/Code switch and no separate application mode.
- Code profile tools are a follow-up milestone; M6 only stores the folder binding and makes it available to the run's tool set.

### Starter kits

- Declarative presets: default team, roles, tools, example tasks, deliverable formats, and policy defaults.
- Onboarding asks what the user wants to accomplish, not which persona they are.
- A user can hold projects with different kits at the same time.
- The existing `.cowtemplate` remains an agent/team format. A project template composes agent/team templates with a pack manifest; the exact schema is open for the M6 kickoff.

### Packs

- A pack declares capabilities, tools, connectors, and the permissions it needs.
- Packs carry no arbitrary executable code in the first version; built-in packs ship with the app; signing and third-party packs are later.
- Finance is the first vertical pack family. Medical research is a later candidate.

### Policy

- The workspace policy is the enforced security floor.
- A project may only tighten the floor, never loosen it.
- Task-level approval settings may override within the floor.
- Policy changes are audited.
- Existing workspace approval policy remains the base; model allowlists, local-only mode, network access, folder access, retention, and export controls are added by the security workstream.

### Editions

- Branded editions are deferred; see ROADMAP 6.2.
- No runtime edition switching.
- One codebase; if a vertical customer later requires a distinct application identity, data directory, update channel, or compliance package, build a separate installer from the same codebase.
- The first release is one CowBot installer with finance starter kits built in.

## Project model (locked)

| Decision | Result |
| --- | --- |
| Hierarchy | Workspace → Project → Channel/Task |
| Workspace role | Data, tenant, and scheduling boundary |
| Project role | Context boundary, not a permission boundary |
| Home | `project_id = NULL` for channels and tasks; a view, not an entity |
| General project | Not created automatically; existing rows are not backfilled |
| Agents | Workspace-wide roster with one direct conversation each |
| Project team | `project_agents` membership; projects own group channels only |
| Direct and user conversations | Global; not project-scoped |
| Channel names | Unique per project; Home is a separate namespace |
| Profile | `work` or `code`, chosen when the project is created |
| Lifecycle | Archive is read-only for new work but lets running work finish; hard delete only for projects with no tasks or messages |
| Membership | Mentions and assignments reject non-members; removing a member keeps history and blocks new assignments |
| Code binding | One local folder per project, stored machine-locally and excluded from `.cowpkg` |
| Scheduling | Concurrency root remains the workspace |

### Behaviour rules

- A task created in a project channel inherits that project. An explicit project that does not match the channel is rejected.
- A task assigned outside the project team is rejected; the UI offers to add the agent first.
- A mention of an agent outside the project team is rejected with a system notice; it does not add the agent automatically.
- While a project is archived, new channels and tasks are rejected, but running tasks and pending approvals can complete.
- Direct conversations and user-created conversations stay global. Project collaboration uses project group channels; project-scoped one-to-one conversations are not in M6.
- Project create, rename, archive, restore, delete, and membership changes go through the service layer and write audit events.

## Data model shape (planned)

- `projects`: id, workspace id, name, description, kind (`work`/`code`), status (`active`/`archived`), pinned, timestamps.
- `project_agents`: project id, agent id, timestamp; unique pair.
- `channels.project_id` and `tasks.project_id`: nullable; `NULL` is Home.
- `project_bindings`: project id, local path, timestamp; machine-local and excluded from workspace packages. Phase 2 adds a host key.
- No backfill: existing channels and tasks remain in Home.
- Project lifecycle and membership events are added to the event bus.
- Channel and task payloads carry their project id so the desktop can filter without extra lookups.

The exact migration number, column names, and serialization defaults are open at the M6 kickoff. The relationships and nullability above are locked.

## Home and navigation

- **Home**: Main Assistant entry, inbox (pending approvals, running tasks, recent results), pinned/recent projects, unfiled group channels, agent direct conversations, and user conversations.
- **Project view**: project header with name, profile, and bound folder; project channels, team, and tasks. The sidebar switches scope and offers a return to Home.
- Settings stay workspace-level: agents, templates, models, workspace transfer, and updates. Project settings live in the project header menu.
- The first version uses one sidebar with scope switching. A project icon rail can come later if the number of projects justifies it.

## Main Assistant

The Main Assistant stays global in Home. M6 adds project awareness:

- `list_projects` (ReadOnly), `create_project` (Medium), `add_project_agent` (High).
- `create_channel` and `create_task` accept an optional `project_id`.
- The assistant can queue work into a project but does not run it; execution stays in Tasks, matching ADR-0028.

## Code projects

- A Code project binds one local folder through a file picker; the path is normalized and stored in `project_bindings`.
- Tools for a run are built from the project context. Home runs keep the current workspace directory.
- A missing or invalid binding disables code tools for that project and asks the user to rebind.
- Diff approval, terminal, git, tests, streaming output, and checkpoint/rollback belong to the Code profile follow-up, not M6.
- The first Code audience is still open: quant/finance users first or general engineering. This changes the follow-up scope but not the M6 model.

## Vertical direction (not M6 scope)

- **Finance** is the first vertical. Target small private funds, family offices, and independent analysts before banks and brokers.
- Finance uses the Work profile, finance starter kits, packs, and a strict policy; it is not a third profile and not a separate mode.
- Position finance as a research and analysis workspace. Do not provide personalized investment advice, do not connect trading or order execution, and require human review before external use.
- Required foundation: data-at-rest encryption, OS keychain for secrets, model egress policy and audit, project folder/network restrictions, and audit export. Local-first and model-neutral are the differentiators, but the current build cannot yet make enterprise security claims.
- First workflows: single-stock/earnings research, portfolio review, and due diligence. Inputs are local PDF/Excel/CSV and user-licensed sources; no market data is redistributed with the product.
- **Medical** is a later vertical. Scope is medical research, medical writing, medical affairs, and clinical operations, not clinical diagnosis, treatment recommendations, or clinical decision support. Medical requires a regulated policy (local models, no external network, full egress audit, de-identification, human review) and should follow finance.
- Target market geography (for example China versus overseas) is open and affects model, data-source, and compliance choices.

## Scope

### In scope

- Project domain, store, service, client, CLI, desktop Home/project UI, Main Assistant tools.
- Project-scoped channels and tasks; team membership; lifecycle and audit.
- Machine-local folder binding and per-run tool context for Code projects.
- Package round trip for projects and membership; bindings excluded.
- Tests and documentation.

### Out of scope

- Code profile tools (diff, terminal, git, tests, checkpoints, streaming).
- Starter kit and pack implementation; project templates.
- Security foundation work (encryption, keychain, egress audit, container sandbox).
- Finance and medical content, connectors, and compliance material.
- Branded editions and runtime edition switching.
- Project-level human permissions and team/server mode.

## Work breakdown

- M6.1 `cowbot-core`: project types, project id on channels/tasks, project events.
- M6.2 `cowbot-store`: tables, repositories, filters, package export/import, migration tests.
- M6.3 `cowbot-service`: lifecycle, membership, channel/task validation, audit.
- M6.4 `cowbot-client` and CLI: project operations and scoped queries.
- M6.5 Desktop: Home, project list, project view, scoped sidebar, project settings, move channel/task, Main Assistant tools.
- M6.6 Code binding: folder picker, binding table, per-run tool context; no code-profile tools.
- M6.7 Tests and docs: migration, round trip, membership rules, UI flows, PRD/architecture updates, ADR.

## Success criteria

- Existing workspaces open unchanged; all existing channels and tasks appear in Home.
- A user can create Work and Code projects, add agents, create project channels and tasks, and see them scoped correctly.
- Mentions and assignments outside the project team are rejected with a clear notice.
- Archive, restore, and delete follow the locked lifecycle rules.
- A Code project can bind a folder and pass it to tool execution; the binding survives restart and is absent after package import.
- `.cowpkg` round-trips projects and membership without local paths.
- No regression in Home, direct conversations, user conversations, tasks, approvals, or scheduling.

## Open at implementation

These are implementation details, not open product decisions:

- Migration number, exact column names, and serialization defaults.
- Sidebar layout and project header details.
- Project template and pack manifest schema.
- Policy field set and how existing workspace policy is extended.
- Code profile tool set and first Code audience.
- Target market geography and first finance design partners.
