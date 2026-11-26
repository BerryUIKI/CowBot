# What CowBot does today, and what it does not

**Applies to:** CowBot 0.1.17 and the `main` branch at the time of writing.
**Last reviewed:** 2026-11-21.

This page is the honest boundary of the product. It says what works, what does
not, and which behaviours are deliberate choices that may surprise you. The
[roadmap](ROADMAP.md) says when the gaps are planned; this page says what they
are.

If something here contradicts what the app does, the app is right and this page
is stale — please open an issue.

## What works today

CowBot 0.1.17 is a local-first **desktop app and CLI**. The default mode runs
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
| Server | Optional self-hosted HTTP/WebSocket API with accounts, sessions, workspace roles and invitations, a live event stream, API tokens, and workspace data surfaces; `RemoteClient` speaks it. Settings → Server signs in, picks a workspace, reconnects on drop, and switches back to the local workspace. A deployment can configure one model provider and answer channel turns itself, with no desktop running (ADR-0055) |
| Interface | Thirteen languages (English, Simplified and Traditional Chinese, French, German, Hindi, Indonesian, Italian, Japanese, Korean, Brazilian Portuguese, Latin American and European Spanish), with a language picker, a device-local preference, and an account menu |
| Updates | Signed in-app updates from GitHub Releases on Windows (NSIS), macOS (DMG), and Linux (AppImage) |

## What does not work yet

These are the gaps you are most likely to run into. Each links to where it is
tracked.

### Collaboration and hosting

- **Only a configured server or the desktop runs agent turns.** By default a
  turn executes in the desktop's process with its model provider, so a team
  member without that provider configured cannot make an agent answer. A server
  can be configured to answer channel turns itself through four
  `COWBOT_SERVER_LLM_*` variables ([ADR-0055](ADR/0055-server-side-turn-execution.md)),
  but its agent is deliberately less capable: it can run code and read or write
  files inside `COWBOT_SERVER_WORKSPACE_DIR`, and nothing else. The desktop's
  workspace tools and project worktrees remain local. Running crews, compacting
  context, and preparing worktrees are local to the machine that opened the
  workspace.
- **Some actions are hidden in server mode, not supported remotely.** Settings →
  Server switches between the local workspace and a server, and the controls
  that need this machine disappear rather than failing: running a task, project
  worktrees and repositories, local workspace management, package export and
  import, compaction, and profile editing. See
  [ADR-0053](ADR/0053-desktop-server-mode.md).
- **Switching backends restarts CowBot.** The transport is chosen at startup, so
  connecting, switching a server workspace, or returning to local mode reloads
  the window. The saved session survives the restart; only Disconnect forgets it.
- **Remote access is limited to the served surfaces.** The server exposes HTTP
  and a workspace-scoped WebSocket, and `RemoteClient` implements the `Client`
  trait, but there is no public API reference or SDK beyond that Rust client. A
  local-mode workspace still lives in one SQLite file on one machine.
- **No live-turn transport.** A remote client can read a conversation and its
  context, but streaming a live turn over the network and cancelling one are not
  available remotely. A dropped stream reconnects on its own and the window
  reloads what it missed. This also bounds a server-run turn: the socket opens a
  bubble when a turn starts and then carries nothing until it settles, so the
  answer appears at once rather than as it is written, and the only way to stop
  one is to cancel the task behind it.
- **A server turn has no spending ceiling.** A deployment that answers turns
  itself spends on a model with nobody present. The approval gate bounds which
  tools run, not how many tokens a turn uses, and there is no per-workspace
  budget yet. A turn's context size *is* bounded when the operator sets
  `COWBOT_SERVER_LLM_CONTEXT_WINDOW`: a full conversation branch is then skipped
  rather than sent, the same way a full group participant is.
- **Server-side group routing skips the capacity gate unless a window is set.**
  A group conversation normally skips a participant whose context branch is
  full, using the window the desktop knows from its model metadata. The server
  passes one only when the operator declares `COWBOT_SERVER_LLM_CONTEXT_WINDOW`,
  so without it a full branch surfaces as a turn failure instead of a skip.
- **The server has no rate limiting.** A shared deployment has no per-user or
  per-workspace request ceiling; an API token or session can issue requests as
  fast as the client and proxy allow. `429` responses are handled by the desktop
  client if a proxy adds them, but the server does not produce them.
- **A write that fails mid-flight is not retried.** Reads are retried up to three
  times over about 600 ms; a create, edit, or approval decision that fails is
  reported so you can decide whether to repeat it, because the API has no
  idempotency key and a silent replay could duplicate it.
- **The desktop checks the server's API version and refuses a mismatch.** A
  server and a desktop from different protocol versions report the mismatch with
  both numbers instead of failing later on an unexpected response; update the
  older side.

- **Four IM platform adapters ship: Mattermost, Telegram, Slack, and Feishu.**
  All four dial out — a Mattermost or Slack bot over a WebSocket, a Telegram bot
  by long polling, a Feishu app over its long connection — so the deployment needs
  no inbound surface for any of them
  ([ADR-0057](ADR/0057-mattermost-adapter.md),
  [ADR-0058](ADR-0058-telegram-adapter.md),
  [ADR-0059](ADR-0059-slack-adapter.md),
  [ADR-0061](ADR-0061-feishu-adapter.md)).
- **A webhook transport means exposing CowBot to the internet**
  ([ADR-0062](ADR/0062-webhook-transports.md)). Every adapter defaults to an
  outbound transport precisely so that is not required; choosing a webhook trades
  that away for a public hostname, a certificate, and a reverse proxy. The
  endpoint verifies every request before parsing it, but the proxy's request-size
  limit is the only bound on an unauthenticated caller.
- **The three webhook transports are not tested against the live platforms.** The
  signing and decryption schemes are covered by unit tests, a published AES test
  vector, and route tests; a deployment is the first place a real platform's
  signature is checked end to end.
- **One process per Slack or Feishu app.** Slack reports how many connections an
  app holds and Feishu delivers each event to one connection at random, so two
  servers on the same app token each see a subset of the traffic and answer with
  the wrong binding table or twice.
- **A Telegram bot cannot read history and cannot open a private chat.** It sees
  only updates delivered while it is polling, so a CowBot channel cannot be
  backfilled from an existing Telegram conversation, and a person has to message
  the bot once before it can post to them.
- **Attachments are stored and named, not understood.** A file somebody sends is
  written below the deployment's tool root and its path is appended to the message
  text, so an agent can read a text file with `read_file` and inspect anything with
  `execute_code` ([ADR-0060](ADR-0060-im-approvals.md)). **It cannot see an
  image**: the LLM layer's message content is a string in every protocol, so a
  photo of a whiteboard is a path, not a picture. Up to four files per message are
  kept and each may be up to 20 MB; the rest are named in the text as not kept.
- **An answer is never sent back as a file.** There is no rule for which of the
  files a turn wrote is the deliverable, so outbound attachments are not built.
- **An answer a platform refuses for rate limiting loses that chunk.** Telegram's
  poll loop honours `retry_after`; a mirrored reply is posted once per chunk and a
  refusal is logged rather than retried.
- **The bot must be able to reach every conversation it is bound to.**
  Mattermost delivers posts only for channels the bot has joined; Telegram only
  from chats the bot was added to, and in a Telegram channel only when it is an
  administrator; Slack needs the bot invited; Feishu needs `is_in_chat` to answer
  true. A binding that misses the requirement is silent rather than broken.
  Bindings are checked when they are created — the response says `reachable`,
  `unreachable`, or `unknown` — and at startup, where a problem posts a notice
  into the CowBot channel it points at
  ([ADR-0057](ADR/0057-mattermost-adapter.md),
  [ADR-0058](ADR/0058-telegram-adapter.md),
  [ADR-0060](ADR-0060-im-approvals.md)). A bot removed from a chat after startup
  is not noticed until the next restart.
- **An approval decided in the IM is attributed to a bridge user id.** The audit
  trail and `tool_calls.approved_by` join on `users.id`, so what they show is
  `bridge:slack:i1:U024` rather than a name; the name is on the bridge user row
  and rendering it is a UI change that has not been made. A refusal is invisible in
  Slack, whose interaction socket has no way to show a toast without an inbound
  request.
- **A refused attachment leaves the message but not the file.** A download that
  fails — a private file the bot cannot read, one deleted between the event and
  the fetch — is reported in the text as "not kept"; nothing retries it later.
- **A stored attachment is readable by any turn on the server.** The tool root is
  per deployment rather than per workspace, and the file tools are rooted rather
  than confined, so an agent that walks out of the root is stopped by the approval
  gate and not by the filesystem.
- **A second bot in a bound channel is treated as a person.** Mattermost does not
  mark a post as coming from a bot, so the adapter cannot reuse the shared echo
  guard for one and two bots would answer each other.
- **A Mattermost disconnect does not recover what was missed.** The socket reconnects
  with backoff, but posts sent while it was down are gone; Mattermost has no
  resume-from-id over this transport.
- **An integration's configuration is stored unencrypted.** A bot token is kept
  as JSON in the deployment's database; a reader of that file sees it. It never
  appears in an API response, a log line, or a `.cowpkg`, but there is no
  encryption at rest and no key management.
- **Each IM account becomes a CowBot user.** A platform account that writes to a
  bound channel is provisioned a `Member` row on first sight, with no password so
  it cannot sign in. In a large tenant the user list grows with the IM directory,
  and no retention action removes an account that stopped writing.

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

- **Closing the window does not quit.** CowBot hides to a tray icon so agent
  turns and crew runs keep going. Settings → General turns that off if you want
  close to mean quit ([ADR-0064](ADR/0064-desktop-background-residency.md)).
- **The tray may be invisible on Linux.** A desktop without an AppIndicator
  extension (a bare GNOME session, for instance) does not show status icons, and
  a hidden window is then only reachable through the account menu's quit —
  which is to say not at all. Nothing detects this and warns you.
- **Nothing tells you when work finishes while the window is hidden.** There are
  no system notifications: a finished run and a waiting approval are only
  visible once you open the window. The tray icon carries no badge or count.
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

- **Closing the window leaves CowBot running.** It is in the system tray, not
  gone, and it is still answering. Use Quit CowBot in the tray menu or the
  account menu to end it — the × only hides it.
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
