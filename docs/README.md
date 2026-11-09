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
| Planned context management | [M1 usage accounting plan](plans/M1-usage-accounting.md) and [ADR-0029](ADR/0029-token-usage-accounting.md) |

## Reading design documents

The PRD, architecture, roadmap, and ADRs include both implemented milestones and future scope. Phase 1 (milestones 1.1–1.7) is implemented: persistence, services, approvals, a local client, CLI integration with `cowbot.toml` and channel/message commands, the assignment scheduler with cross-root concurrency, the desktop shell with vendor-first model providers and model discovery, workspace transfer, automatic updates, release builds, and the user guide. Phase 2 (self-hosted team mode) has not started. Check the root README and source for precise availability.

## Visual assets

[Runtime architecture](assets/architecture.svg) depicts current code. [Workspace concept](assets/workspace-concept.svg) illustrates the workspace direction and must retain its concept label; it is not an application screenshot. Both ship a `-light.svg` counterpart for light backgrounds; the README serves each through a `<picture>` element so the diagram follows the reader's color scheme. SVGs are editable source assets with accessible titles and descriptions; keep them self-contained, readable, and free of embedded scripts or external resources. Update the README alt text and both variants when changing their meaning.

Additional references: [Desktop setup](../desktop/README.md), [template format](TEMPLATE_FORMAT.md), [design references](REFERENCES.md), and [brand assets](../assets/README.md).
