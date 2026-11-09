# ADR 0020: CLI Configuration and Channel Commands

- **Status**: Accepted
- **Date**: 2026-11-07
- **Related**: `docs/ADR/0007-client-api.md`, `docs/ADR/0008-cli-and-local-execution.md`, `docs/ADR/0019-assignment-scheduler.md`, `docs/ROADMAP.md` Phase 1

## Context

ADR-0008 left two CLI gaps open: configuration-file support and channel-based collaboration. Every invocation repeated the same `--db`, `--model`, and `--approval` flags, and there was no way to keep those defaults with a project. The client already exposed channels and messages, but the CLI only used them indirectly through task creation, so headless scripts could not create a channel, post a message, or place a crew's tasks in one.

## Decision

### 1. `cowbot.toml`

The CLI reads an optional TOML file:

```toml
db = "cowbot.db"
model = "gpt-4o"
base_url = "https://api.openai.com/v1"
approval = "ask"       # ask | accept-edits | safe-auto | plan-only | trusted
concurrency = 4        # cross-root concurrency limit
```

Lookup order is `--config <path>`, then `COWBOT_CONFIG`, then `cowbot.toml` in the working directory, then the built-in defaults. A missing default file is fine; a missing explicitly named file is an error. Unknown keys are rejected so typos surface immediately.

Value precedence is command-line/environment, then the file, then defaults:

- `db`: `--db` / `COWBOT_DB`, then `db`, then `cowbot.db`.
- `model`: `--model`, then `model`, then `gpt-4o`.
- `base_url`: `--base-url` / `OPENAI_BASE_URL`, then `base_url`, then none.
- `approval`: `--approval`, then `approval`, then `ask`.
- `concurrency`: `--concurrency`, then `concurrency`, then 4, never below 1.

API keys are deliberately excluded from the file; they stay in `OPENAI_API_KEY` so a checked-in configuration cannot leak a secret.

### 2. Channel and message commands

- `cowbot channel list`
- `cowbot channel create --name <name> --topic <topic>`
- `cowbot message list --channel <id> [--limit 50]`
- `cowbot message post --channel <id> --content <text> [--thread <id>]`

All four call the transport-neutral `Client` trait through `LocalClient`, so the remote client added in Phase 2 can back them without changing the command surface. Posting and starting a run validate the channel id first and report a clear error instead of surfacing a SQLite foreign-key failure.

`cowbot run --channel <id>` passes the channel through `CrewRunOptions`, so the run's root tasks are stored against that channel. `cowbot run --concurrency <n>` overrides the scheduler limit for one invocation.

### 3. Out of scope

Template commands remain future CLI work. The CLI does not read the desktop `config.json`; its providers and credentials are separate by design.

## Alternatives considered

- **Reuse the desktop `config.json`**: it lives in the app-data directory, holds API keys, and describes GUI state. Sharing it would couple the CLI to the desktop's storage and widen the secret's blast radius.
- **Environment variables only**: workable for CI but cannot persist a project's model, database, and approval defaults in the repository.
- **Nested TOML sections (`[run]`, `[database]`)**: adds structure the current five keys do not need; a flat schema stays readable as the file grows.
- **Channel commands that bypass the `Client` trait**: would duplicate persistence and event publishing and block the remote client.

## Consequences

- A repository can commit a `cowbot.toml` describing its database, model, and approval policy; secrets stay outside the file.
- `cowbot init`, `run`, `export`, `import`, `channel`, and `message` all honor the same resolution rules.
- The CLI now exercises channel persistence and the `Sender::User` path; agents and the desktop read the same messages.
- Task channel assignment is validated, so a stale channel id fails with a clear message.
- Template commands and remote mode remain future work.
