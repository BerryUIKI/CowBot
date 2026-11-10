# Development guide

## Setup

Use current stable Rust, Cargo, rustfmt, Clippy, and native compiler/linker tools. The workspace uses edition 2021 and does not declare an MSRV. Clone the `dev` branch; see [Contributing](../CONTRIBUTING.md) and [Collaboration](COLLABORATION.md) for the PR workflow.

```sh
rustup component add rustfmt clippy
cargo build --workspace --locked
cargo run -p cowbot-cli -- --help
cargo run -p cowbot-cli -- init --db cowbot.db
cargo run -p cowbot-cli -- run --task "Plan a calculator" --dry-run --db cowbot.db
```

The mock provider needs no model service or API credentials. It produces mock deliverables, but initialization and run history still write to SQLite. Initial builds need registry access unless dependencies are cached.

## Code map

- `cowbot-core`: domain models, assignments, approvals, risk types, tools, and events.
- `cowbot-bus`: broadcast event delivery and bounded in-memory history.
- `cowbot-store`: SQLite migrations and repositories.
- `cowbot-service`: bootstrap, channels, templates, assignments, approvals, and persisted crew execution.
- `cowbot-client`: transport-neutral data/command interface and `LocalClient`; opening it initializes storage and reconciles interrupted state.
- `cowbot-package`: `.cowpkg` workspace packages — manifest, checksums, export, and verified import.
- `cowbot-agent`: sequential crews, runtime loop, and an optional tool gate.
- `cowbot-tools`: tool registry, subprocess/file tools, and static risk classification.
- `cowbot-llm`: OpenAI Chat Completions, Anthropic Messages, and OpenAI Responses transports plus a mock provider.
- `cowbot-cli`: `init`, `run`, `tools`, `export`, and `import`; `run` executes through `LocalClient::run_crew`, and `export`/`import` read the `.cowpkg` format directly.
- `desktop`: React/TypeScript frontend and standalone Tauri backend, outside the root Cargo workspace. The frontend calls Tauri commands wrapping the client boundary, never SQLite directly.

Review [architecture](ARCHITECTURE.md), [ADRs](ADR/README.md), and [template format](TEMPLATE_FORMAT.md) before changing boundaries. Designs are agreed for Phase 1; discuss changes and new scope before implementation. Remote mode, streaming, long-term memory, isolated sandboxes, and other roadmap items remain unfinished; consult [CLAUDE.md](../CLAUDE.md) for current gaps.

## Verification

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo doc --workspace --no-deps --locked
```

Use focused tests such as `cargo test -p cowbot-service` while iterating; run workspace gates for code PRs. Add regression coverage for changed behavior, including persistence, approval rejection/timeouts, and reconciliation where relevant. Keep tests deterministic and free of live API dependencies and secrets. Documentation-only changes should check links, examples, status claims, and SVG rendering.

Desktop changes require separate checks; root workspace commands do not cover Tauri:

```sh
cd desktop
pnpm install --frozen-lockfile
pnpm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings
```

Use `pnpm run tauri dev` for the native shell. See [desktop prerequisites](../desktop/README.md); a browser-only Vite preview does not supply the native client backend.

Desktop release bundles are built by [`.github/workflows/release.yml`](../.github/workflows/release.yml) on manual dispatch and on `v*` tags: NSIS/MSI on Windows, a universal DMG on macOS, and deb/AppImage on Linux. Manual runs upload workflow artifacts; tag runs create draft GitHub Releases that a maintainer publishes after review. Tags must match the version in `desktop/src-tauri/tauri.conf.json`. Builds are unsigned; see [ADR-0014](ADR/0014-desktop-release-builds.md). Updater artifacts are signed with `TAURI_SIGNING_PRIVATE_KEY` and its password; a final `updater-json` job writes `latest.json` from all three platform builds (ADR-0018). Local bundle builds without the key need `pnpm run tauri build -- --no-sign`.

## Runtime configuration

`init` and `run` accept `--db`, falling back to `COWBOT_DB`, then `cowbot.db` in the current directory. Desktop uses `app_data_dir()/cowbot.db`; its state is separate by default. Protect database files and backups as potentially sensitive local data.

`run` accepts `--model` (default `gpt-4o`), `--base-url` / `OPENAI_BASE_URL`, `--api-key` / `OPENAI_API_KEY`, `--dry-run`, and `--approval` (default `ask`). With neither key nor URL it uses the mock; `--dry-run` forces the mock. A key alone selects the OpenAI endpoint. The HTTP client appends `/chat/completions`. CLI config files and automatic `.env` loading are not implemented. Streaming, retries, and rate limiting remain gaps.

The desktop keeps its model providers in `app_data_dir()/config.json` (ADR-0012, extended by ADR-0016): each entry records a protocol (`openai`, `anthropic`, or `openai-responses`), base URL, model, and API key, plus the active provider. API keys are stored in plain text. The Settings view (gear beside the local user row, with Model/Team/Approvals/Audit/Projects/Workspace/About sections) adds, edits, tests, and switches providers; changes take effect without a restart, and no provider selected means the offline mock model. Files written before the protocol field existed load as OpenAI providers. The same file records whether first-run onboarding was completed; files written before that field existed are treated as completed (ADR-0013). It also records the active workspace database and the workspace registry (ADR-0017): the default is `app_data_dir()/cowbot.db`, and created or imported workspaces use `app_data_dir()/workspaces/<uuid>.db`. Settings → Workspace creates empty workspaces, exports the active workspace, imports a package as a new database, and switches between registered databases; creating, switching, and import restart the app and are refused while a crew is running. The update-check preference (`auto_check_updates`, default on) and the About section's update controls are described in ADR-0018. Environment variables do not configure the desktop; the CLI does not read this file. Protect `config.json` as sensitive local data and keep it out of packages and backups shared with others.

Approval policies are `ask`, `accept-edits`, `safe-auto`, `plan-only`, and `trusted`. Headless callers should select an appropriate policy; `ask` prompts on stdin and EOF rejects. Review [ADR 0003](ADR/0003-risk-classification-and-approval.md) and [ADR 0008](ADR/0008-cli-and-local-execution.md). Approval is not OS isolation; see [Security](../SECURITY.md).

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Unexpected mock output | Remove `--dry-run` and supply a key or endpoint |
| HTTP/authentication errors | Verify endpoint prefix, credential, available model, and tool-call compatibility |
| Waiting for approval | Inspect stdin or pending approval state and the chosen policy |
| Desktop and CLI show different data | Check their database locations; they are separate by default |
| Desktop task does not start | The shell supports creation and inspection; execution launch is follow-up work |
| Tool command syntax fails | Agent commands use `cmd /C` on Windows and `sh -c` elsewhere |
| Dependencies/linker unavailable | Check cached registry dependencies and native toolchain prerequisites |

`--verbose` enables debug logs. Redact credentials, prompts, file content, paths, and tool arguments before sharing output. `tools` prints schemas and executes a local `echo` smoke test.
