# CowBot user guide

This guide covers the 0.1.x local desktop app: installing it, connecting a
model, creating a workspace, running a task with an agent crew, handling
approvals, and moving workspaces in and out of the app.

CowBot runs entirely on your machine. Messages, tasks, approvals, and run
history live in local SQLite databases; the only network traffic is the model
request sent to the endpoint you configure, plus one update check against GitHub
at launch when automatic update checks are enabled (see [Updates](#updates)).

## Install

Download the build for your platform from the
[releases page](https://github.com/BerryUIKI/CowBot/releases):

| Platform | Files |
| --- | --- |
| Windows 10/11 (x64) | `CowBot_<version>_x64-setup.exe` (NSIS) or `.msi` |
| macOS (Apple Silicon + Intel) | `CowBot_<version>_universal.dmg` |
| Linux (x64) | `CowBot_<version>_amd64.deb` or `.AppImage` |

- **Windows:** run the installer. SmartScreen may warn because builds are
  unsigned; choose **More info → Run anyway**.
- **macOS:** open the DMG and drag CowBot to Applications. On first launch,
  right-click the app and choose **Open**, or allow it in **System Settings →
  Privacy & Security**.
- **Linux:** `sudo apt install ./CowBot_<version>_amd64.deb`, or
  `chmod +x CowBot_<version>_amd64.AppImage && ./CowBot_<version>_amd64.AppImage`.

Builds are unsigned. From 0.1.2 onward CowBot can download and install updates
itself when you ask it to; see [Updates](#updates). Users upgrading from 0.1.1
or earlier must install 0.1.2 manually once because those versions have no
update code.

## First run

The three-step setup guide covers:

1. **Welcome** — CowBot is local-first; data stays on this machine.
2. **Model** — connect a model provider, or skip and use the offline mock model.
3. **Team** — add a starter team from a built-in template.

You can skip setup at any point. Everything is available later in **Settings**
(the gear at the right of your name in the sidebar).

## Connect a model

Open **Settings → Model**. Providers are model endpoints; CowBot ships with an
offline mock model and uses it whenever no provider is active.

1. **Add provider** opens a vendor catalogue grouped into model vendors,
   third-party platforms and relays, cloud providers, and local runtimes. Search
   the catalogue by name or URL.
2. Pick a vendor to pre-fill the form, or **Custom** for a blank form and any
   compatible endpoint. Every field stays editable.
3. **Protocol** is a dropdown in the form:
   - **OpenAI Chat** — the Chat Completions API, used by OpenAI, DeepSeek,
     Ollama, OpenRouter, and most relays.
   - **Anthropic** — the Messages API, used by Claude and Claude
     Code-compatible relays.
   - **Responses** — the OpenAI Responses API, used by Codex-style endpoints.

   Changing the protocol keeps the name, base URL, and model you entered.
4. **Fetch models** asks the endpoint which models the key can use and adds them
   to the model dropdown. Pick the active model there. If the endpoint has no
   usable listing, the error appears beside the field; use **Add manually** to
   type a model name instead. Discovery is optional and never blocks saving.
5. Enter the API key if the endpoint needs one. Local endpoints usually do not.
6. **Test connection** sends one small request. The result appears in the
   editor.
7. **Save provider**. The first provider becomes active; use **Use** on another
   card to switch, or **Use offline mock** to run without a model. Each card
   shows its active model and how many models the provider knows.

A provider can hold several models. To change the active one, choose **Edit**,
pick another model, and save; the stored API key is kept, so you never retype
it.

The base URL may be a bare host (`https://api.anthropic.com`), include `/v1`, or
be the full endpoint (`https://api.anthropic.com/v1/messages`). CowBot appends
the protocol path when it is missing.

API keys are stored in plain text in the local configuration file. Protect that
file as sensitive data.

### Connection errors

- **401 / unauthorized** — the endpoint wants a key or the key is wrong.
- **Connection refused / error sending request** — nothing is listening at that
  URL, or a proxy is blocking it.
- **404** — the endpoint is reachable but does not serve the selected protocol.
  Check that the protocol matches the provider (for example, a Claude relay
  needs **Anthropic**, not **OpenAI Chat**).
- **Model listing failed** — the endpoint does not expose a models catalogue, or
  it returned an unexpected shape. Type the model name with **Add manually** and
  continue.

## Workspaces

A workspace contains its channels, agents, tasks, and history. CowBot keeps
**one local database per workspace**. The first workspace is created
automatically; **Settings → Workspace** manages the rest.

- **Create workspace** — enter a name and choose **Create workspace**. CowBot
  restarts into the new, empty workspace.
- **Switch** — selects another registered workspace and restarts CowBot.
- **Export workspace** — writes the active workspace to a `.cowpkg` package.
  Exported packages contain channels, messages, agents, templates, tasks, runs,
  audit events, and conversation participants, settings, and context branches
  with SHA-256 checksums; credentials are never included.
- **Import package** — verifies a `.cowpkg` package, creates a new workspace
  database named `<name> (imported)`, registers it, and restarts into it. Import
  never merges with or overwrites existing data.

Creating, importing, and switching restart the app; finish running tasks first,
or CowBot refuses the change. Workspace databases and `config.json` live in the
app data directory:

- Windows: `%APPDATA%\com.cowbot.desktop\`
- macOS: `~/Library/Application Support/com.cowbot.desktop/`
- Linux: `~/.local/share/com.cowbot.desktop/`

The default database is `cowbot.db`; imported or created workspaces live under
`workspaces/`. Deleting a database file removes that workspace from the switcher
on the next start.

## Navigation

Chat is the main surface. The left sidebar lists conversations, agent contacts,
channels, and tasks. The header row switches back to Chat and starts a new
channel; the local user row at the bottom holds Settings.

Select a task to open its **Task** tab in the right inspector; the conversation
stays visible. The chat header has an approval button and a background button,
each with a count, and they open the **Approvals** and **Background** tabs of the
same inspector. Task detail, approvals, and running work share one right-side
panel instead of replacing Chat.

The inspector remembers its last tab and closes with **×**. On windows narrower
than about 1080 px the sidebar hides while the inspector is open so the chat and
panel stay readable side by side; below 760 px the inspector overlays the chat.

## Projects and worktrees

A **project** is a local git repository that CowBot may run tasks in. Projects
are registered per machine: their paths live in the desktop `config.json` and
are never included in `.cowpkg` packages.

- **Register a repository** in **Settings → Projects** with **Add folder**.
  Choose any folder inside the repository; CowBot resolves the work-tree root,
  derives the project name from the folder, and shows the current branch.
  Duplicate folders and folders outside git are rejected with an explanation.
- **Remove** deletes only the registry entry; files and existing worktrees on
  disk are untouched. A folder that is no longer available is marked
  **Missing** without failing the list.
- **Run a task in a worktree**: in **Tasks → New task**, enable **Run in a git
  worktree**, pick the project and a base branch (the project's current branch
  by default), and create the task. When you press **Run**, CowBot creates
  `<repo>/.cowbot/worktrees/<slug>` on branch `cowbot/<slug>-<id>` and roots
  the agents' file, code, and command tools there. The checkout is created from
  the base branch at run time, so a queued task does not hold a stale checkout.
  `.cowbot/` is added to the repository's local `.git/info/exclude`, not the
  tracked `.gitignore`.
- The branch is **not merged automatically**. Inspect it with git or from the
  task detail, then merge it in your own tooling.
- **Inspect a worktree** in the task detail's Worktree card: status, branch
  (with **Copy**), base branch, project, path, and creation time.
  **Open folder** reveals the checkout; **View diff** shows `git status`, the
  diffstat, and the patch (up to 256 KiB, with a truncation notice).
- **Discard** removes the checkout folder and the branch after a confirmation.
  It is disabled while the task is running and loses uncommitted changes. The
  record stays as **Removed**. A worktree whose folder disappeared shows
  **Missing** and can be discarded to clean up its record; create a new task to
  run in a worktree again.
- If creating the worktree fails (not a repository, unknown base branch,
  existing branch or path, git not installed), the run is rejected with the git
  error and the task stays pending.
- Worktrees are only created for tasks that requested a project. Channel turns
  and tasks without a project keep using the shared app-data workspace.

## Team

**Settings → Team** lists the built-in templates and the agents in the
workspace. The **Main Assistant** is the primary agent for a one-person
company: agent `main` plans work, can create channels, start specialist teams,
and queue tasks for other agents. **Add agents** instantiates every agent in a
template with its role, goal, and tool access. The 0.1.x app creates agents
from templates; a standalone agent editor is not part of this release.

- **Edit an agent** with **Edit** in Settings → Team or **Edit agent…** in an
  agent conversation's context menu. You can change its name, role, and tool
  selection. Names must be unique; the agent's conversation follows the new
  name.
- To turn an existing Assistant into the main assistant, open **Edit**, set the
  name to `main`, choose a role such as `Primary Assistant`, and enable the
  workspace tools (`create_channel`, `create_task`, `instantiate_team`,
  `list_agents`, `list_templates`). Adding the Main Assistant template is the
  alternative.
- Agent actions that change the workspace ask for approval under the
  **Manual** and **Auto** policies. The approval card shows the tool name,
  arguments, risk, and reason.
- The Main Assistant can target a registered project: `create_task` accepts
  `project` and an optional `base_branch`, validates them, and requests a
  worktree for the queued task. Its environment block lists registered projects
  with their current branches, and the approval card shows the project and base
  branch arguments before you approve.

## Agents, channels, and tasks

- **Agents** — the sidebar lists every agent as a contact with its latest
  message and time. Select one and write in the composer: the message goes to
  that agent directly and no mention is needed. The header shows the agent's
  role and the approval policy for this conversation; messages appear as a
  chat timeline, and a typing bubble marks a turn in progress.
- **Conversations** — the **Conversations** section holds user-created sessions
  with one agent or a group of two to eight. Choose **New conversation**, give
  it a title, and select participants in the order you want them listed; use
  ↑ and ↓ to reorder a selected participant. With two or more selected, choose
  the **default responder** — preselected to the Main Assistant (`main`) when it
  is in the group, otherwise the first participant — and optionally enable
  **Route unaddressed messages with the model**. Unlike an agent contact, a
  conversation keeps its own model context and a fixed model; you can have
  several conversations with the same agent, and the header shows participant
  chips and a context meter (see below). Conversations can be pinned, renamed,
  deleted, compacted, and cleared like other sidebar rows.
- **Group routing** — in a group conversation, `@Agent` addresses that
  participant and `@team` (also `@all`, `@everyone`, `@here`) addresses every
  participant; names outside the group are rejected with a notice. A message
  that addresses nobody goes to the default responder. With routing enabled,
  the conversation's model picks up to three participants first and falls back
  to the default responder when it cannot. Type `@` in the composer to open a
  participant list: arrow keys move, Enter or Tab inserts, and Escape closes it.
- **Channels** — group rooms sit below the agents. Create one with **New
  channel** or the **+** button, post messages, and follow the timeline. Names
  must be unique in the workspace, case-insensitively. Mention an agent by name
  to get an answer in the channel: `@Planner, which module owns the parser?`
  The agents in the workspace are listed in the
  channel header, and `@team` (also `@all`, `@everyone`, `@here`) addresses all
  of them. Matching is case-insensitive; an unknown name produces a system
  notice listing the agents that exist.
- **Manage conversations** — right-click an agent, conversation, or channel row
  (or use its **⋯** button) to pin it to the top, rename it, delete a group
  channel or conversation, clear an agent conversation's history, or compact and
  clear a user-created conversation's context (one participant's branch or all
  branches). Deleting a channel or
  conversation deletes its messages (and, for a conversation, its context);
  tasks keep their history but are no longer linked to that channel. Clearing an
  agent conversation keeps the agent and removes only its messages. Agent
  conversations cannot be deleted from the channel list.
- **Search** — the field above the sidebar list filters agents, conversations,
  channels, and tasks by name.
- **Rich answers** — agent and user messages render Markdown: headings, lists,
  tables, code blocks, and links. Links open in your system browser. A fenced
  `html` code block appears as an **HTML preview** card with **Preview**,
  **Code**, and **Expand**; the preview runs in a sandbox that permits inline
  CSS and JavaScript but blocks network access, external images, form
  submission, and access to the app. Use it for interactive results such as
  forms, calculators, or small dashboards.
- **Approval** — the mode control in the composer footer opens the five-mode
  menu: Auto, Manual, Accept edits, Plan, and Bypass permissions. It applies
  to answers in that conversation and uses the same policies as tasks, so a
  risky tool call raised while an agent answers appears in **Approvals**.
  Settings → Approvals also manages workspace allow / ask / deny rules and
  session decisions; Settings → Audit filters decisions by source.
- A direct message or a mention becomes a task for that agent: it is listed in
  **Tasks** with its assignment, run, and tool calls, and it can be cancelled
  there. The answer is also posted back to the conversation as a message from
  that agent.
- **Tasks** — create a task with a description, expected output, an agent, an
  optional channel, an approval policy, and optionally a project worktree.
  Tasks are listed in the sidebar; select one to open its **Task** tab in the
  right inspector and choose **Run** to execute the Planner → Coder → Reviewer
  crew. The submitted task becomes the run's owning task: its detail shows the
  run tasks, assignments, assignment events, runs, tool calls, and the worktree
  when one exists.
- **Crew runs** create three related tasks (Planner, Coder, Reviewer) under the
  task you ran.
- **Cancel** stops a running task.

Progress refreshes from live events; all history persists across restarts.

### Background tasks

Running work is visible without leaving Chat.

- **Sidebar cards** — under a group channel, up to two compact cards show that
  channel's active runs: status dot, title, latest activity (`Reading …`,
  `Running: …`, or the tool name), a worktree branch chip, and elapsed time.
  Select a card to open the owning task.
- **Background panel** — the **Background** button in the chat header shows the
  active run count and opens the **Background** tab in the right inspector. Each
  run shows its status, elapsed time, an `x/y` progress bar, expandable
  assignment rows with the responsible agent and latest detail, and an **Up
  next** line for queued steps. **Finished** is collapsed by default and lists
  the last 20 runs with an outcome snippet and duration; select one to open its
  task. The inspector remembers its last tab.
- Cards update from assignment, tool-call, task, and worktree events. The
  inspector shares its right-side panel with task detail and approvals; at
  narrow window widths the sidebar hides while it is open.

> Answers use the active model provider. With no provider configured the app
> falls back to the offline mock model, which only echoes the prompt; with a
> provider that cannot answer, the channel shows a system notice with the error
> instead of staying silent.

### Conversation context

A user-created conversation keeps an independent, append-only context for each
participant. The visible timeline and the model context are separate: clearing
or compacting the context never deletes messages.

- The **context ring** in the header shows the highest occupancy among
  participants. Hover it for percent used and left, the token ratio, and a
  breakdown into system prompt, tool schemas, history, and the latest message.
  Below 70% the ring is neutral, 70–89% is a warning, 90–99% is a danger
  colour, and 100% is full.
- In a group, the popover has one tab per participant with that branch's
  breakdown, latest-run cache figure, and conversation-total cache figure. The
  tabs are keyboard accessible with the left and right arrow keys.
- Percentages need a context window. Set **Context window** (and optionally
  **Max output**) for the model in **Settings → Model**. Without it the ring
  shows absolute tokens only.
- **Cache hit rate** for the latest run and the whole conversation comes from
  the usage the provider reports. Router and compaction calls count toward the
  conversation total but not toward a participant's latest run. Providers
  without prompt caching show no cached tokens.
- **Compact now** summarizes older context entries into one summary and stops
  sending the summarized entries; recent turns stay verbatim. In a group it
  compacts the active participant's branch. A system notice in the timeline
  reports the token change, and the original messages remain visible.
- **Compact automatically** is off by default. When enabled, CowBot compacts a
  selected participant's branch before its turn if it would cross the configured
  percentage (80% by default) of the usable window.
- **Clear context** stops sending earlier messages but keeps them in the
  timeline; in a group, **Clear this branch** affects the active participant and
  **Clear all** affects every participant. **Delete conversation** removes the
  conversation, its messages, and its context.
- A single-agent conversation blocks sending at 100% and offers compact, clear,
  and new-conversation actions. A group is not blocked up front: after routing,
  a full participant is skipped with a notice naming it while the others answer,
  and a turn whose selected participants are all full posts a notice without a
  model call.
- Each user message in a group gets an **Answered by** line under it, showing
  whether mentions, routing, or the default responder chose the responders,
  which participants replied, and any failures or skips. Open **Routing** for
  the selected roster, unmatched names, the router's reason, and outcome counts.
  These details survive a restart because the route is stored with the message.
- A message that addresses nobody in a group with no default responder stays
  unanswered; the timeline shows a short note instead of inventing an agent.
- The fixed model is recorded when the conversation is created; changing the
  active provider in Settings does not switch existing conversations. A
  conversation whose provider was deleted falls back to the active provider
  with its recorded model name.

### Approval policies

- **Auto** — approve read-only and low-risk tools automatically; medium and high risk are decided by the active model's permission evaluator, which falls back to asking you when it is unsure or unavailable.
- **Manual** — approve read-only tools automatically; ask before anything that
  changes state (default for interactive use).
- **Accept edits** — run workspace file edits automatically; other tools
  follow Manual.
- **Plan** — run only read-only tools and deny everything else. The agent is
  asked to answer with a plan; it is stored with the task, and **Approve plan
  and continue** re-runs that task in Manual mode with the approved plan as
  context. Plan mode also applies to conversation and channel turns.
- **Bypass permissions** — run every tool call. Critical-risk calls follow
  **Settings → Approvals → Critical calls under Bypass**: Allow (default), Ask
  with a red approval card, or Block. Destructive channel deletion always asks,
  even under Bypass. Selecting Bypass warns every time, and the mode control
  stays red while it is active.

Approvals appear in the **Approvals** tab of the right inspector, opened by the
approval button in the chat header; pending inline approval cards remain in the
conversation. Workspace rules in Settings → Approvals are evaluated before the
model evaluator: Deny wins over Ask, and Ask over Allow. Each decision records
its source in the audit log, and Settings → Audit filters by rule, model,
session, human, policy, or system. Approval is a guardrail, not OS isolation:
the subprocess sandbox runs commands with your user permissions, so treat
model-generated commands as untrusted.

## Data and backups

- Workspace data: the workspace databases under the app data directory.
- Model providers, API keys, the onboarding flag, and the workspace registry:
  `config.json` in the same directory (plain text).
- Backups: use **Export workspace** for a portable package, or copy the database
  files while CowBot is closed.
- Uninstalling the app does not delete your data; remove the app data directory
  manually if you want a clean install.

## Updates

CowBot 0.1.2 and later can check for and install new versions. Release builds
are signed with the updater's own key, so updates are verified even though the
installers are not code-signed.

- **Automatic checks.** With **Settings → About → Check for updates
  automatically** enabled (the default), CowBot asks GitHub Releases once at
  launch whether a newer stable version exists. The check sends no workspace
  data and can be turned off. When offline, the check fails silently.
- **Manual checks.** Open **Settings → About** and choose **Check for updates**.
- **Installing.** When an update is available, a banner appears above the main
  pane and the About page shows the version and release date. Choose **Download
  and install**; progress appears in place. Finish or cancel running tasks
  first: installing is disabled while a crew runs.
- **Restarting.** On macOS and Linux, choose **Restart now** after the download
  finishes. On Windows the installer closes CowBot automatically.

Updates work for the NSIS installer on Windows, the app installed from the DMG
on macOS, and the AppImage on Linux. If you installed the Windows MSI or the
Linux deb package, download and install new releases manually. macOS may ask you
to allow CowBot again after an update because builds are not notarized.

CowBot 0.1.1 and earlier have no updater. Install 0.1.2 manually once; later
versions update themselves.

## Command line

The `cowbot` CLI covers headless and scripted use. It does not read the desktop
`config.json`; it reads an optional `cowbot.toml` from the working directory and
takes flags and environment variables.

```toml
# cowbot.toml
db = "cowbot.db"
model = "gpt-4o"
base_url = "https://api.openai.com/v1"
approval = "ask"       # ask | accept-edits | safe-auto | plan-only | trusted
concurrency = 4        # cross-root concurrency limit
```

Lookup order is `--config <path>`, then `COWBOT_CONFIG`, then `cowbot.toml` in
the working directory. Command-line values and environment variables override
the file. API keys are never read from the file; use `OPENAI_API_KEY`.

```bash
cowbot init
cowbot channel create --name general --topic "General"
cowbot channel list
cowbot message post --channel <id> --content "Starting the run"
cowbot message post --channel <id> --content "@Planner what next?" --wait
cowbot message list --channel <id>
cowbot run --task "Analyze and write code" --dry-run --channel <id>
cowbot run --task "Fix the bug" --model gpt-4o --approval ask --concurrency 2
cowbot tools
cowbot export --output my-workspace.cowpkg --db cowbot.db
cowbot import --input my-workspace.cowpkg --db other.db
```

- `--db` / `COWBOT_DB` overrides the configured database; the default is
  `cowbot.db` in the working directory.
- `--dry-run` forces the offline mock model.
- `--model`, `--base-url` (`OPENAI_BASE_URL`), and `--api-key`
  (`OPENAI_API_KEY`) configure the OpenAI Chat Completions endpoint.
- `--approval` accepts `ask`, `accept-edits`, `safe-auto`, `plan-only`, and `trusted`.
- `--concurrency` bounds how many root assignments run at the same time.
- `run --channel <id>` stores the crew's tasks in that channel.
- `message post --wait` lets the agents mentioned in the message answer before
  the command prints the channel; it uses the same provider settings as `run`.

## Troubleshooting

- **Setup guide does not appear** — it runs once. Use **Settings** to configure
  models, agents, and workspaces.
- **A task fails immediately** — check the active provider and connection test;
  with no provider, tasks run against the offline mock model.
- **A direct message gets no answer** — select the agent in the sidebar and send
  from the composer; the message should reach that agent without a mention. If
  the agent list is empty, add agents from a template in **Settings → Team**.
- **A group channel message gets no answer** — agents answer there only when
  they are mentioned by name (`@Planner`); an unknown name posts a notice
  listing the agents that exist.
- **An agent answer stops with an error** — the conversation shows a system
  notice with the model error; check the active provider in **Settings → Model**.
- **A workspace cannot be switched** — finish or cancel running tasks first.
- **A workspace shows Missing** — its database file was moved or deleted; remove
  it from the registry by restarting, or restore the file.
- **A migration checksum error appears** — earlier builds on Windows could
  record migration checksums for CRLF line endings. Current builds repair that
  case automatically after validating the schema and keep all workspace data.
  If the message persists, back up the workspace database file with its
  `-wal`/`-shm` sidecars and report the message; do not delete rows from
  `_sqlx_migrations` by hand.
- **Package import fails** — the package may be corrupt, from an unsupported
  schema version, or larger than 512 MB.
- **An update check fails** — CowBot needs to reach `github.com`. Check the
  network or proxy; automatic checks stay silent when offline. A manual check
  reports an error when the newest published release has no `latest.json` yet.
- **No update is offered** — only published, non-prerelease releases are
  visible; draft releases do not appear.
- **An update fails to install** — retry from **Settings → About**. The app must
  be able to write to its installation location (the AppImage directory or
  `/Applications`).
- **The old version still runs after updating** — close and reopen CowBot. On
  Windows the installer may still be finishing.

Report bugs at https://github.com/BerryUIKI/CowBot/issues. For security
reporting, see [SECURITY.md](../SECURITY.md).

## Known limitations in 0.1.x

- Local/offline single-user mode only: no server, remote sessions, or
  multi-user workspaces.
- Channel answers are one turn per addressed agent: no threads as conversation
  scope, no proactive agent replies to unmentioned messages in group channels,
  and no streaming.
- HTML previews cannot reach the network, external images, or the app, and a
  button inside one cannot send a message back to the agent yet. Reply in the
  composer after using the preview.
- Workspace tools are available in the desktop app only; the CLI's channel
  turns do not expose them. `create_task` queues a pending task and does not run
  the assigned agent automatically.
- Worktrees are per task run and are not merged back from CowBot: there is no
  merge or pull-request action, and deleting a task does not delete its
  worktree. Registered project paths and worktree records stay on one machine
  and are not part of workspace packages.
- Agents cannot be deleted from the UI; edit their name, role, or tools
  instead.
- Sequential Planner → Coder → Reviewer crews; no hierarchical orchestration or
  DAG workflows.
- User-created conversations are single-agent in this release; group
  conversations and mention routing arrive in a later milestone.
- No streaming responses, retries, or rate limiting. Token usage is recorded per
  model call, but there are no cost or billing views.
- Context occupancy and compaction are estimated from a character heuristic
  between calls; provider-reported usage is authoritative once a call returns.
- The subprocess sandbox is not OS-level isolation.
- Workspace deletion and renaming are not available in the UI.
- Builds are unsigned; macOS may require approval again after an update.
  Automatic updates cover NSIS, the DMG-installed app, and AppImage; MSI and
  deb installs update manually.
