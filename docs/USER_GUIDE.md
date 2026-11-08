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
(the gear in the sidebar footer).

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
  and audit events with SHA-256 checksums; credentials are never included.
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

## Team

**Settings → Team** lists the built-in templates and the agents in the
workspace. **Add agents** instantiates every agent in a template with its role,
goal, and tool access. The 0.1.x app creates agents from templates; a standalone
agent editor is not part of this release.

## Channels and tasks

- **Channels** — create a channel, post messages, and follow the timeline.
  Mention an agent by name to get an answer in the channel: `@Planner, which
  module owns the parser?` The agents in the workspace are listed in the
  channel header, and `@team` (also `@all`, `@everyone`, `@here`) addresses all
  of them. Matching is case-insensitive; an unknown name produces a system
  notice listing the agents that exist.
- **Approval** — the dropdown in the channel header sets the approval policy
  for answers in that channel. It uses the same policies as tasks, so a risky
  tool call raised while an agent answers appears in **Approvals**.
- A mention becomes a task for the mentioned agent: it is listed in **Tasks**
  with its assignment, run, and tool calls, and it can be cancelled there. The
  answer is also posted back to the channel as a message from that agent.
- **Tasks** — create a task with a description, expected output, an agent, an
  optional channel, and an approval policy. Select the task and choose **Run**
  to execute the Planner → Coder → Reviewer crew. The task detail shows
  assignments, assignment events, runs, and tool calls.
- **Cancel** stops a running task.

Progress refreshes from live events; all history persists across restarts.

> Answers use the active model provider. With no provider configured the app
> falls back to the offline mock model, which only echoes the prompt; with a
> provider that cannot answer, the channel shows a system notice with the error
> instead of staying silent.

### Approval policies

- **Ask** — prompt for risky tool calls (default for interactive use).
- **Safe auto** — automatically approve low-risk reads while asking for
  higher-risk actions.
- **Plan only** — deny actions that would modify the machine.
- **Trusted** — approve everything except critical actions.

Approvals appear in **Approvals**. Each decision is recorded in the audit log.
Approval is a guardrail, not OS isolation: the subprocess sandbox runs commands
with your user permissions, so treat model-generated commands as untrusted.

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
approval = "ask"       # ask | safe-auto | plan-only | trusted
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
- `--approval` accepts `ask`, `safe-auto`, `plan-only`, and `trusted`.
- `--concurrency` bounds how many root assignments run at the same time.
- `run --channel <id>` stores the crew's tasks in that channel.
- `message post --wait` lets the agents mentioned in the message answer before
  the command prints the channel; it uses the same provider settings as `run`.

## Troubleshooting

- **Setup guide does not appear** — it runs once. Use **Settings** to configure
  models, agents, and workspaces.
- **A task fails immediately** — check the active provider and connection test;
  with no provider, tasks run against the offline mock model.
- **A channel message gets no answer** — agents answer when they are mentioned
  by name (`@Planner`). Add agents from a template in **Settings → Team** first;
  an unknown name posts a notice listing the agents that exist.
- **An agent answer stops with an error** — the channel shows a system notice
  with the model error; check the active provider in **Settings → Model**.
- **A workspace cannot be switched** — finish or cancel running tasks first.
- **A workspace shows Missing** — its database file was moved or deleted; remove
  it from the registry by restarting, or restore the file.
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
- Channel answers are one turn per mentioned agent: no threads as conversation
  scope, no proactive agent replies to unmentioned messages, and no streaming.
- Sequential Planner → Coder → Reviewer crews; no hierarchical orchestration or
  DAG workflows.
- No streaming responses, retries, rate limiting, or token accounting.
- The subprocess sandbox is not OS-level isolation.
- Workspace deletion and renaming are not available in the UI.
- Builds are unsigned; macOS may require approval again after an update.
  Automatic updates cover NSIS, the DMG-installed app, and AppImage; MSI and
  deb installs update manually.
