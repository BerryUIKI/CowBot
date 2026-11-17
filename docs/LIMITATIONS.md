# What CowBot does today, and what it does not

**Applies to:** CowBot 0.1.13 and the `dev` branch at the time of writing.
**Last reviewed:** 2026-11-16.

This page is the honest boundary of the product. It says what works, what does
not, and which behaviours are deliberate choices that may surprise you. The
[roadmap](ROADMAP.md) says when the gaps are planned; this page says what they
are.

If something here contradicts what the app does, the app is right and this page
is stale — please open an issue.

## What works today

CowBot 0.1.13 is a local-first **desktop app and CLI**. The default mode runs
everything on your machine against a SQLite database; an optional self-hosted
server shares a workspace with a small team over HTTP and WebSocket.

| Area | What you can do |
| --- | --- |
| Chats | Direct chat with every agent, group channels, and user-created single or group conversations with two to eight participants |
| Context | Per-participant context meters, manual and automatic compaction, a read-only next-request inspector, and race-safe compaction |
| Memory | A workspace note, conversation notes, and scoped memory entries with deterministic recall, a `remember` tool, and reviewed extraction |
| Tasks | Create a task, run a crew, watch assignments and tool calls, approve risky steps, cancel, and inspect history |
| Worktrees | Register a local git project and run a task in its own worktree, with a branch, diff, and discard action |
| Approvals | Risk-based policy, workspace rules, session decisions, Plan mode, and a Bypass Critical tri-state |
| Models | OpenAI Chat Completions, Anthropic Messages, and OpenAI Responses, with endpoint model discovery, several models per provider, one default model separate from connections, and per-conversation model scope |
| Audit | Normalized token usage with cache fields, sealed redacted prompt snapshots with a retention window, and a source-filtered audit view |
| Composer | Sixteen `/commands` as desktop actions, and `/btw` for a side question that never enters the conversation |
| Streaming | Turns stream into a live bubble with a Stop button; a stopped turn keeps what it produced |
| Packages | Export a workspace to `.cowpkg` and import it into a new workspace, with credentials redacted |
| Server | Optional self-hosted HTTP/WebSocket API with accounts, sessions, workspace roles and invitations, a live event stream, API tokens, and workspace data surfaces; `RemoteClient` speaks it |
| Interface | Thirteen languages (English, Simplified and Traditional Chinese, French, German, Hindi, Indonesian, Italian, Japanese, Korean, Brazilian Portuguese, Latin American and European Spanish), with a language picker, a device-local preference, and an account menu |
| Updates | Signed in-app updates from GitHub Releases on Windows (NSIS), macOS (DMG), and Linux (AppImage) |

## What does not work yet

These are the gaps you are most likely to run into. Each links to where it is
tracked.

### Collaboration and hosting

- **The app does not switch to a server from its settings yet.**
  `cowbot-server` serves accounts, sessions, workspaces, roles, invitations,
  channels, messages, tasks, approvals, audit, API tokens, a live event stream,
  memory, conversations and their context, previews, recorded prompts,
  workspace-scoped templates, and package import/export over HTTP and WebSocket
  (ADR-0043 through ADR-0047). `RemoteClient` implements the same `Client` trait,
  so every surface except worktrees, in-process crew execution, live-turn
  cancellation, and process-local session decisions works against a server —
  but the desktop's settings do not yet offer choosing one. The local runtime
  still auto-creates one user and remains the default way to use CowBot. See the
  [roadmap](ROADMAP.md).
- **Only one machine runs agent turns.** A remote client can read a conversation
  and its context, but a turn still executes in the desktop's process with its
  model provider, so a team member without that provider configured cannot make
  an agent answer.
- **Remote access is limited to the served surfaces.** The server exposes HTTP
  and a workspace-scoped WebSocket, and `RemoteClient` implements the `Client`
  trait, but there is no public API reference or SDK beyond that Rust client. A
  local-mode workspace still lives in one SQLite file on one machine.
- **No live-turn transport.** A remote client can read a conversation and its
  context, but streaming a live turn over the network, cancelling it, running a
  crew in-process, and preparing a worktree are not available remotely.

### Isolation and security

- **The sandbox is not an isolation boundary.** Code execution runs as your host
  user in a subprocess. It is not a container, VM, or OS sandbox, and it does not
  cap memory, block the network, or guarantee that every child process dies. See
  [SECURITY.md](../SECURITY.md).
- **File tools are unrestricted.** They join paths against a base directory but
  do not enforce traversal or symlink containment.
- **No secrets management.** Provider API keys live in environment variables and
  the desktop's local `config.json`; they are never returned to the frontend.
  There is no vault, rotation, or per-agent scoping.

### Orchestration

- **No hierarchical orchestration or DAG workflows.** A crew runs sequentially;
  the assignment scheduler handles depth and concurrency but cannot express a
  graph.
- **No consolidated group-plan approval** and no turn grouping in the background
  panel. Each participant's work is approved and shown on its own.
- **Group routing is mention-, router-, or default-based.** Reply rotation,
  participant mute, and reply cleanup are tracked in [#183](https://github.com/BerryUIKI/CowBot/issues/183).
- **A worktree branch cannot be merged back or opened as a pull request from the
  app.** Inspect the diff and use normal git tooling.

### Knowledge and retrieval

- **Recall is lexical, not semantic.** Memory recall is deterministic and
  budgeted over an FTS5 index with a `LIKE` fallback. There are no embeddings, no
  vector store, and no project knowledge base.
- **No prompt-cache optimisation.** Anthropic and Responses prompt caching is M5.

### Audit and accounting

- **Audit events and tool calls are not linked to the prompt snapshot that
  produced them.** The audit view lists snapshots on their own; you cannot yet
  click through from a tool call to the exact request.
- **Token counting is exact only for OpenAI-compatible families.** `o200k_base`
  and `cl100k_base` are compiled in; Claude, DeepSeek, and local runtimes use a
  labelled character estimate.

### Desktop and CLI

- **Slash commands are executed by the desktop only.** The CLI shares the
  catalogue and parser but does not run commands. There is no command history,
  and `/model` cannot reach a model on an inactive provider.
- **A side question cannot use conversation history**, by design.
- **The CLI cannot create group conversations**, and it does not render the
  desktop's panels.
- **Database migrations are forward-only.** An older CowBot cannot open a
  database a newer one upgraded; the desktop asks before upgrading and keeps a
  copy, and refuses a newer database with recovery choices instead of starting.
  The CLI upgrades without asking. Development builds use their own `dev` data
  directory so they cannot upgrade an installed workspace.
- **Interface localisation covers the desktop chrome.** Thirteen interface
  languages are shipped: English, Simplified and Traditional Chinese, French,
  German, Hindi, Indonesian, Italian, Japanese, Korean, Brazilian Portuguese,
  Latin American Spanish, and European Spanish. Service diagnostics, agent
  profiles, template content, stored audit reasons, and the CLI remain in
  English; right-to-left layouts and plural categories beyond one/other are not
  implemented.

## Deliberate surprises

These are not bugs. They are choices that behave differently from what you may
expect.

- **Slash commands never become messages.** `/clear`, `/compact`, `/rename`, and
  the rest act on the app; they do not post anything into the chat, and they do
  not appear in the transcript.
- **`/btw` answers are ephemeral.** The answer is in a panel, not the
  conversation. Dismiss it or switch chats and it is gone. The agent will not
  remember it, because nothing was written. Its cost is still recorded, as a
  `side_question` call, so audit stays honest.
- **A path that starts with `/` is still a message.** `/usr/bin/env python` is
  sent as text; the command menu only takes over while the draft is a single
  `/` token, and Escape overrides an unknown token.
- **Context commands require a user-created conversation.** In a channel or an
  agent's direct chat there is no independent branch to compact or clear, so
  those commands decline instead of guessing.
- **A stopped turn is kept, not discarded.** Stopping mid-reply stores what the
  model produced with a `_[Stopped]_` marker, so a reload shows what you saw.
  The alternative — silently discarding it — loses work you may want.
- **Group replies are not blocked on a full branch.** A full participant is
  skipped with a chip, and the rest of the turn continues, rather than failing
  the whole send.
- **Workspace packages never carry device-local state.** Prompt snapshots, LLM
  usage, group turn rows, and model providers stay on the device that produced
  them.
- **A schema upgrade is one-way.** CowBot warns on a startup screen and offers
  either a direct upgrade or a copy first, but the upgraded file cannot be
  opened by an older build. Recovery restores a backup or starts an empty
  workspace; it never deletes the database it replaces.
- **Only one CowBot window runs at a time.** Launching the app again focuses the
  open window instead of starting a second copy. A development build is exempt:
  it keeps its own data under `dev/`, so it can run beside the installed app.
- **Builds are unsigned.** macOS may ask you to allow CowBot again after an
  update, and Windows may show a SmartScreen prompt. See
  [ADR-0014](ADR/0014-desktop-release-builds.md).

## Getting help

- **Something is broken:** open an issue with the app version (Settings →
  About), the platform, and what you expected.
- **A security problem:** follow [SECURITY.md](../SECURITY.md); do not post
  exploit details publicly.
- **You want a feature:** check the [roadmap](ROADMAP.md) and the open issues
  first, then open one describing the workflow you are trying to complete.

See also the [user guide](USER_GUIDE.md), the [developer guide](DEVELOPMENT.md),
and the [decision records](ADR/README.md).
