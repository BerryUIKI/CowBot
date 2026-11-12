# Architecture decision records

These records accompany the [architecture](../ARCHITECTURE.md). A decision describes intended direction; it does not prove implementation or override a record's status.

| Record | Topic |
| --- | --- |
| [0001](0001-runtime-and-deployment.md) | Runtime and deployment |
| [0002](0002-template-format-and-distribution.md) | Template format and distribution |
| [0003](0003-risk-classification-and-approval.md) | Risk classification and approval |
| [0004](0004-import-export-package.md) | Import/export package |
| [0005](0005-authentication-and-identity.md) | Authentication and identity |
| [0006](0006-assignment-runtime.md) | Assignment runtime |
| [0007](0007-client-api.md) | Client API |
| [0008](0008-cli-and-local-execution.md) | CLI and local execution |
| [0009](0009-desktop-ipc-and-state.md) | Desktop IPC and state |
| [0010](0010-cowbot-product-name.md) | CowBot product name |
| [0011](0011-cowbot-software-rename.md) | CowBot software rename |
| [0012](0012-local-model-configuration.md) | Desktop model provider configuration |
| [0013](0013-first-run-onboarding.md) | First-run onboarding |
| [0014](0014-desktop-release-builds.md) | Desktop release builds |
| [0015](0015-pnpm-for-desktop-dependencies.md) | pnpm for desktop dependencies |
| [0016](0016-model-provider-protocols.md) | Model provider protocols |
| [0017](0017-desktop-workspace-transfer.md) | Desktop workspace transfer |
| [0018](0018-desktop-auto-updates.md) | Desktop automatic updates |
| [0019](0019-assignment-scheduler.md) | Assignment scheduler and cross-root concurrency |
| [0020](0020-cli-configuration-and-channels.md) | CLI configuration and channel commands |
| [0021](0021-vendor-first-model-providers.md) | Vendor-first model providers and model discovery |
| [0022](0022-channel-mention-turns.md) | Channel mention turns and agent replies |
| [0023](0023-channel-turn-scheduler-scope.md) | Scoped assignment scheduling for background turns |
| [0024](0024-agent-direct-conversations.md) | Agent direct conversations |
| [0025](0025-database-migration-checksum-compatibility.md) | Database migration checksum compatibility |
| [0026](0026-channel-and-conversation-management.md) | Channel and conversation management |
| [0027](0027-rich-agent-output.md) | Rich agent output: Markdown and sandboxed HTML artifacts |
| [0028](0028-main-assistant-and-workspace-actions.md) | Main assistant and desktop workspace actions |
| [0029](0029-token-usage-accounting.md) | Token usage accounting |
| [0030](0030-conversation-sessions-and-context.md) | Conversation sessions and context accounting |
| [0031](0031-task-worktrees-and-background-cards.md) | Task worktrees and background task cards |
| [0032](0032-group-conversations-and-routing.md) | Group conversations and routing |
| [0033](0033-agent-memory-and-shared-notes.md) | Agent memory and shared notes |
| [0034](0034-compaction-concurrency.md) | Compaction concurrency and cutoff insertion |
| [0035](0035-model-aware-token-counting.md) | Model-aware token counting |

For a new decision, copy [the template](TEMPLATE.md), assign the next unused four-digit number, and use a descriptive English filename. Discuss it in a linked issue/PR. Record context, alternatives, consequences, and status; do not silently rewrite an accepted decision when direction changes. Add a superseding record and link both records instead.

Product naming: [0010: CowBot product name](0010-cowbot-product-name.md) and [0011: CowBot software rename](0011-cowbot-software-rename.md).
