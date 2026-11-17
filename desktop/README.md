# CowBot Desktop

Tauri v2 shell with a React + TypeScript frontend. The Rust backend is a
standalone Cargo project (outside the root workspace) and reaches the local
core exclusively through `cowbot-client`. All data lives in `cowbot.db` under the
platform app-data directory, so the desktop app and the CLI can use separate
databases without sharing state. On first launch after the CowBot rename, an
existing `cowai.db` from the previous `com.cowai.desktop` directory is copied
into the new app-data directory automatically.

On first launch, a three-step setup guide introduces the local workspace, lets
the user connect a model provider, and adds a starter team from a built-in
template. It can be skipped at any point; completion is recorded in the same
`config.json`, and a configuration written before onboarding existed is treated
as completed so upgrades do not show the guide again.

The current UI is conversation-first: the sidebar lists every agent as a contact
with its latest message and a search field, group channels sit below the agents,
and the main pane is a chat timeline with bubbles, a typing indicator, and a
floating composer. Right-clicking a row (or using its **⋯** button) pins the
conversation, renames or deletes a group channel, or clears an agent
conversation's history; channel names are unique per workspace. **Tasks**
(create tasks, inspect assignment/run history, cancel, and run a crew on a
submitted task), **Approvals** (approve or reject tool calls requested by
running agents), and **Settings** (with **Model**, **Team**, **Workspace**, and
**About** sections for model providers, agents and templates, workspace creation
and transfer, and version and update controls) are reached from the sidebar
footer. A message in an agent's conversation reaches it without an `@mention`;
group channels keep mention-driven replies (ADR-0022, ADR-0024, ADR-0026).
Messages and task deliverables render Markdown and sanitized inline HTML;
fenced `html` blocks open as sandboxed HTML artifacts with a Preview/Code
toggle, where inline scripts run but network access, external images, forms,
and the app itself stay out of reach (ADR-0027).

A new workspace automatically contains the starter template's **Main
Assistant**, created as **CowBot**. It can create channels, instantiate
specialist teams, and queue tasks for other agents via desktop workspace tools; those actions go through the normal approval policy and
their arguments and risk appear on the approval card (ADR-0028). Its first empty
conversation offers a one-time naming card. Agents can be edited from Settings →
Team or the conversation context menu (name, role, tools); names are unique and
the direct conversation follows a rename.

Settings → General also has an **Agent tags** switch that adds a small translated
`bot` tag beside agent names in chats and agent lists. It is stored per device
and off by default; the tag is presentation only and never enters a name, a
mention, or a prompt (ADR-0052).

Model providers are OpenAI Chat Completions, Anthropic Messages, or OpenAI
Responses endpoints kept in `config.json` under the same app-data directory as
`cowbot.db`. **Add provider** opens a searchable, grouped vendor catalogue; each
entry pre-fills a single form whose protocol is a dropdown, and **Custom** opens
a blank form. A provider records its protocol, one or more known models, and the
active model. **Fetch models** queries the endpoint's catalogue (falling back to
manual entry), and the Settings view can add, edit, test, and switch providers.
The active one is used by the next crew run, and no active provider means the
offline mock model. Files written before protocol selection existed load as
OpenAI providers, and files without a model list load with their active model as
the only entry. API keys are stored in plain text in that file and are never
sent back to the frontend. Environment variables do not configure the desktop
(the CLI keeps its flags and environment variables).

## Language and account menu

The interface ships in thirteen languages: English, Simplified and Traditional
Chinese, French, German, Hindi, Indonesian, Italian, Japanese, Korean, Brazilian
Portuguese, Latin American Spanish, and European Spanish.
The bottom-left row shows the user and workspace name and opens an account
menu: Settings, Language, Model configuration, Workspace settings, View
changelog, and Learn more. Language opens a Claude-style picker grid; the choice
applies immediately and is stored per device in `config.json`. Catalogues live
in `src/i18n/`, with English as the typed source of truth; add a locale by
adding a catalogue and a registry entry in `src/i18n/catalog.ts`. See
[ADR-0050](../docs/ADR/0050-desktop-interface-localisation.md).

## Workspaces and packages

Each workspace lives in its own local database. The default is
`app_data_dir()/cowbot.db`; created and imported workspaces use
`app_data_dir()/workspaces/<uuid>.db`. Settings → Workspace creates empty
workspaces, lists registered ones, switches between them, and exports the active
workspace as a `.cowpkg` package. Creating, importing, and switching restart the
app, and all three are refused while a crew is running. Import always creates a
new workspace database and never merges data. See
[ADR-0017](../docs/ADR/0017-desktop-workspace-transfer.md) and the
[user guide](../docs/USER_GUIDE.md).

## Updates

The desktop uses the Tauri updater with GitHub Releases as the stable update
channel. When **Check for updates automatically** is enabled (the default,
stored in `config.json`), CowBot makes one request to GitHub at launch. A newer
published release shows a banner in the main pane and a card in **Settings →
About**; downloading and installing always need a click, and installing is
refused while a crew runs. Restarting finishes the update; the Windows installer
exits the app automatically.

Updates apply to the NSIS installer on Windows, the app installed from the DMG
on macOS, and the AppImage on Linux. MSI and deb installs update by downloading
a new release. Builds remain unsigned, so macOS may ask for approval again after
an update. See [ADR-0018](../docs/ADR/0018-desktop-auto-updates.md).

The updater verifies a minisign signature against the public key in
`src-tauri/tauri.conf.json`. The private key and password are repository
secrets (`TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`);
maintainers must keep an offline backup. Losing them prevents automatic updates
for installed clients. The endpoint uses `releases/latest`, so an update is only
visible after the draft release is published.

## Prerequisites

- Node.js 22.13+ and pnpm (`corepack enable pnpm`, or `npm install -g pnpm`)
- Rust toolchain
- On Windows, the WebView2 runtime (already present on current Windows 10/11)

## Run

```bash
pnpm install
pnpm run tauri dev
```

The dev server runs on `http://localhost:1420`; the Tauri window opens once the
backend compiles.

### Data directory during development

A debug build keeps its workspace and `config.json` under `app_data_dir()/dev`,
separate from an installed release, so `pnpm run tauri dev` can apply new
migrations without making the installed app unopenable. Set `COWBOT_DATA_DIR`
(an absolute path, or a path relative to the app data directory) to point a
build somewhere else; the directory is created on demand. Installed release
builds keep using `app_data_dir()` itself.

Before opening the database the shell compares its schema version with the
build's and shows a startup screen in the app. Pending migrations offer
**Upgrade**, **Back up and upgrade** (which writes
`cowbot.db.backup-<from>-<timestamp>`), or **Quit**; a database from a newer
build offers a compatible backup, an empty workspace, or Quit (ADR-0051). The
first-run guide can move the workspace databases to another folder, and a
second launch focuses the open window instead of starting another copy. Only the
release build takes that instance lock, so `tauri dev` can run beside an
installed CowBot instead of being mistaken for it.

## Build

```bash
pnpm run build                    # type-check and bundle the frontend
pnpm run tauri build -- --no-sign # produce platform bundles without signing
pnpm run tauri build              # production bundles; needs the updater key
```

Local bundle builds need `-- --no-sign` unless `TAURI_SIGNING_PRIVATE_KEY` and
its password are set, because `bundle.createUpdaterArtifacts` signs updater
artifacts during the build. CI supplies the key from repository secrets, so
local release candidates should not drop `--no-sign` accidentally.

## Release builds

The [release workflow](../.github/workflows/release.yml) builds installers on manual dispatch and on `v*` tags:

- **Windows:** NSIS `setup.exe` and `.msi`
- **macOS:** universal `.dmg` for Apple Silicon and Intel
- **Linux:** `.deb` and `.AppImage`

Manual runs upload the bundles as workflow artifacts. Tag runs create or update a draft GitHub Release; a maintainer publishes it after review. Tag runs also sign updater artifacts with the repository secrets and a final `updater-json` job publishes `latest.json` for every platform. The tag must match the version in `src-tauri/tauri.conf.json` (`v0.1.5` for `0.1.5`). GitHub offers manual dispatch only once the workflow exists on the default branch; tag builds run from the tagged commit.

Builds are unsigned: macOS may require allowing the app in System Settings, and Windows may show a SmartScreen prompt. OS code signing and notarization remain follow-up work (ADR-0014); the updater's own minisign signature is implemented in ADR-0018.

Build bundles locally with:

```bash
pnpm run tauri build -- --bundles nsis,msi      # Windows
pnpm run tauri build -- --bundles dmg           # macOS
pnpm run tauri build -- --bundles deb,appimage  # Linux
```

Bundles are written under `src-tauri/target/release/bundle/`, or under the target-specific directory when `--target` is used.

## Quality checks

```bash
pnpm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

The root workspace gates (`cargo fmt/clippy/test --workspace`) intentionally do
not include `src-tauri`; run the desktop checks above when changing the shell.
