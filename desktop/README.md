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

The current UI has four views: **Channels** (channels and live messages),
**Tasks** (create tasks, inspect assignment/run history, cancel, and run a crew
on a submitted task), **Approvals** (approve or reject tool calls requested by
running agents), and **Settings** (in the sidebar footer, with **Model**,
**Team**, **Workspace**, and **About** sections for model providers, agents and
templates, workspace creation and transfer, and version and update controls).

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

Manual runs upload the bundles as workflow artifacts. Tag runs create or update a draft GitHub Release; a maintainer publishes it after review. Tag runs also sign updater artifacts with the repository secrets and a final `updater-json` job publishes `latest.json` for every platform. The tag must match the version in `src-tauri/tauri.conf.json` (`v0.1.4` for `0.1.4`). GitHub offers manual dispatch only once the workflow exists on the default branch; tag builds run from the tagged commit.

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
