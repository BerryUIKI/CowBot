# ADR 0008: CLI Integration and Local Execution Entry Point

- **Status**: Accepted
- **Date**: 2026-11-04
- **Related**: `docs/ADR/0001-runtime-and-deployment.md`, `docs/ADR/0007-client-api.md`, `docs/ARCHITECTURE.md` §3.9, `docs/ROADMAP.md` Phase 1.6 / 2.3

## Context

`cow-client` provides the local data and command surface, but `cowai` still built a crew directly on `cow-agent` and called `Crew::kickoff`. Nothing was persisted, approvals never ran, and no real entry point invoked startup reconciliation. The CLI needed to join the client boundary without forcing remote-mode design prematurely.

## Decision

### 1. Database location

`cowai init` and `cowai run` accept `--db <path>` (env `COWAI_DB`) and default to `cowai.db` in the working directory. Config-file support is still a gap; this default is deliberately simple and will be replaced when configuration lands.

### 2. Startup path

Both commands open `LocalClient`, which migrates SQLite, bootstraps the local owner/workspace, and runs assignment plus approval startup reconciliation. `cowai init` only reports the resulting session; `cowai run` continues with execution.

### 3. Execution entry point

`cowai run` keeps the existing Planner/Coder/Reviewer crew assembly but executes through `LocalClient::run_crew`, which delegates to `cow_service::CrewRunner`. The runner creates missing task rows, enqueues root assignments, updates task status, records runs and tool calls, gates tools through approvals, and writes audit records.

`run_crew` is an **inherent method on `LocalClient`**, not part of the transport-neutral `Client` trait: a `cow_agent::Crew` carries in-process LLM and tool handles that cannot be serialized for a remote call. `Client` remains the data/command boundary; remote execution needs a server-side API in Phase 2.3.

### 4. Approvals in the CLI

`cowai run` gains `--approval <ask|accept-edits|safe-auto|plan-only|trusted>` (default `ask`). While the crew runs, the CLI subscribes to client events; each `ApprovalRequested` prints the risk, reason, and tool call id and prompts `y/N`. The decision goes through `Client::decide_approval`, so the session user is recorded as the approver. EOF or an unanswered prompt rejects.

## Consequences

- The CLI now persists tasks, assignments, runs, tool calls, approvals, and audit records, and exercises startup reconciliation.
- Headless invocations must choose a non-interactive approval policy (`trusted` or `plan-only`); with the default `ask`, an approval request waits for stdin or until the service timeout expires.
- The CLI still does not use channels or templates; channel-based collaboration remains future CLI work.
- The database-path default and the interactive prompt are CLI-local concerns and do not enter `Client`.
- The desktop UI will consume the same `Client` surface and render approval cards from events; it does not use `run_crew`.
