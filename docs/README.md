# Documentation

Start with the root [README](../README.md) for implemented functionality and a runnable quick start.

| Audience | Guide |
| --- | --- |
| Users | [Desktop user guide](USER_GUIDE.md) |
| Developers | [Development and troubleshooting](DEVELOPMENT.md) |
| Contributors | [Contributing](../CONTRIBUTING.md) |
| Collaborators and reviewers | [Collaboration workflow](COLLABORATION.md) |
| Maintainers | [Release checklist](RELEASING.md) |
| Release history | [Release notes](releases/README.md) |
| Community | [Code of Conduct](../CODE_OF_CONDUCT.md) and [security reporting](../SECURITY.md) |
| Product contributors | [PRD](PRD.md) and [roadmap](ROADMAP.md) |
| Architecture contributors | [Target architecture](ARCHITECTURE.md) and [decision records](ADR/README.md) |
| Context management (implemented) | [M1 usage accounting plan](plans/M1-usage-accounting.md), [M2 conversations and context plan](plans/M2-conversations-and-context.md), [ADR-0029](ADR/0029-token-usage-accounting.md), and [ADR-0030](ADR/0030-conversation-sessions-and-context.md) |
| Group conversations and routing (implemented) | [M3 group conversations and routing plan](plans/M3-group-conversations-and-routing.md) and [ADR-0032](ADR/0032-group-conversations-and-routing.md) |
| Planned agent memory and shared notes | [M4 agent memory and shared notes plan](plans/M4-agent-memory-and-shared-notes.md) |
| Planned projects and product structure | [M6 project plan](plans/M6-projects-and-product-structure.md) |

## Reading design documents

The PRD, architecture, roadmap, and ADRs include both implemented milestones and future scope. Phase 1 (milestones 1.1–1.7) is implemented: persistence, services, approvals, a local client, CLI integration with `cowbot.toml` and channel/message commands, the assignment scheduler with cross-root concurrency, the desktop shell with vendor-first model providers and model discovery, workspace transfer, automatic updates, release builds, and the user guide. Implemented on top of that baseline are token usage accounting and conversation context with compaction (M1/M2, ADR-0029/ADR-0030), approval rules, session decisions, Plan mode, and the Bypass Critical tri-state (ADR-0003), task worktrees with background cards (ADR-0031), and group conversations with mention-scoped and opt-in model routing (M3, ADR-0032). Phase 2 (self-hosted team mode) has not started. Check the root README and source for precise availability.

## Visual assets

[Runtime architecture](assets/architecture.svg) depicts current code. [Workspace concept](assets/workspace-concept.svg) illustrates the workspace direction and must retain its concept label; it is not an application screenshot. Both ship a `-light.svg` counterpart for light backgrounds; the README serves each through a `<picture>` element so the diagram follows the reader's color scheme. SVGs are editable source assets with accessible titles and descriptions; keep them self-contained, readable, and free of embedded scripts or external resources. Update the README alt text and both variants when changing their meaning.

Additional references: [Desktop setup](../desktop/README.md), [template format](TEMPLATE_FORMAT.md), [design references](REFERENCES.md), and [brand assets](../assets/README.md).
