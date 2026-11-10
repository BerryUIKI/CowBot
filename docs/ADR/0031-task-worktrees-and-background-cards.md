# ADR 0031: Task Worktrees and Background Task Cards

- **Status**: Accepted
- **Date**: 2026-11-10
- **Discussion**: [issue #96](https://github.com/BerryUIKI/CowBot/issues/96), issues #97–#106
- **Supersedes**: None

## Context

The desktop runtime gives every agent the same tool registry rooted at the
app-data directory (`AppState::build_tools`). Agents can read, write, and run
code, but only in a scratch folder: a registered workspace database is not a
code repository, and nothing tells an agent which repository a task belongs to.
Users who want isolated code work must edit files outside CowBot.

Work is also hard to follow. A crew run is submitted from one task, but
`build_standard_crew` creates three unrelated tasks. The submitted task stays
`pending` and its detail shows no assignments, so pressing **Run** looks like
nothing happened. The sidebar lists conversations and tasks, not running work,
and assignment transitions never reach the event stream, so any live view would
have to poll SQLite.

The product direction is a Claude-style desktop workflow: register a project,
ask for a task, let the agents work in an isolated checkout, and watch the work
as compact background cards. This record fixes that design before the
implementation slices land.

## Decision

### 1. Projects are a device-local registry

`AppConfig` (desktop `config.json`) gains `projects: Vec<ProjectEntry { id,
name, path }>`. A project is a local git repository:

- **Add folder** resolves the repository root with
  `git -C <path> rev-parse --show-toplevel`, stores the canonical root, derives
  the name from the folder, and rejects duplicates and folders that are not
  inside a git work tree.
- The registry is device-local. Paths never appear in `.cowpkg` packages and
  are never imported from another machine.
- Removing a project deletes only the registry entry. Existing worktree records
  carry their own path and branch, so removing a project does not break them.
- Settings → Projects reads the current branch from git when the list is
  requested, so an unavailable folder is visible without failing the view.

### 2. A worktree is an opt-in per-task checkout

A git worktree exists only inside a repository, so only a task that belongs to a
registered project can request one. Tasks without a project keep running
against the shared app-data workspace.

- The New task form offers a project and a base branch (default: the project's
  current branch) only when projects exist.
- A requested worktree becomes real when the task runs:
  `<repo>/.cowbot/worktrees/<slug>` on branch `cowbot/<slug>`, created with
  `git worktree add -b <branch> <path> <base>`.
- The slug is derived from the task description, capped at 32 characters, and
  suffixed with four task-id characters, so concurrent tasks cannot collide on
  a branch or a path.
- `.cowbot/` is appended to `.git/info/exclude` (never the tracked
  `.gitignore`), because it is local tooling output rather than a repository
  change.
- Failures are reported as actionable task errors: not a git repository,
  unknown base branch, existing branch or path, and missing git.

### 3. Worktrees are created, inspected, and discarded; nothing is merged

The lifecycle is **request → prepare → ready → removed**, with a `missing`
state for a ready checkout whose folder disappeared.

- `prepare` is idempotent: a ready worktree is returned unchanged.
- `diff` reports `git status --porcelain`, `git diff <base> --stat`, and the
  full patch, capped at 256 KiB with a truncation flag.
- `discard` runs `git worktree remove --force` and `git branch -D`, then marks
  the record `removed`. The record stays for history.
- Startup runs `reconcile` (ready records whose path is gone become `missing`)
  and `git worktree prune` for registered projects.
- Merging a branch back, opening a pull request, and automatic cleanup on task
  deletion are out of scope for this milestone.

### 4. A run owns exactly one worktree through its run group

`Task` gains `run_group: Option<TaskId>`. `build_standard_crew` sets it to the
submitted task's id on the Planner, Coder, and Reviewer tasks. The owning task
is the run group: cards, worktree records, and the overview group by it.

The submitted task is not rewritten to `in_progress`; it stays a container for
the run. Status for cards and the overview is derived from the group's
assignments, and the owning task's detail lists the run's tasks and
assignments. Channel turns keep `run_group = None`; they remain single-task
turns.

### 5. Per-run tools are rooted at the worktree

`AppState::build_tools` splits into client-backed workspace tools and a rooted
builder. A worktree run builds a registry from the worktree path
(`SubprocessSandbox`, `ReadFileTool`, `WriteFileTool`) plus the client-backed
workspace tools, and passes the worktree into crew construction so every crew
task states its checkout and branch. Channel turns and non-worktree runs keep
the shared registry.

A worktree creation failure rejects the run before any crew task executes; the
submitted task stays `pending` and the UI shows the git error. The worktree is
not discarded when the run ends.

### 6. Worktree records are local database state

A `task_worktrees` table keyed by the owning task stores the project id, project
path, base branch, branch, worktree path, status, and timestamps. It has a
cascading foreign key to `tasks`.

The table and its paths are deliberately absent from `.cowpkg` export, because
they describe one machine. A worktree record for a task that is later imported
elsewhere is recreated only if the user registers a matching project and
requests one there.

### 7. Background cards are event-driven

`EventPayload` gains:

- `AssignmentUpdated { assignment_id, task_id, status, kind, detail }`,
  published by the assignment service on every persisted transition.
- `WorktreeUpdated { task_id, status }`, published on request, prepare, discard,
  and reconcile.

`ToolCallRequested` and `ToolCallExecuted` gain `task_id` so a card can show the
current activity without loading run history. The desktop refreshes one
`list_task_overview` payload on these events with a short debounce instead of
polling.

The overview groups assignments by run group and derives a display status:
running wins, then needs input, waiting, resume, queued; a group whose
assignments are all terminal is finished. Cards show elapsed time, progress,
the latest tool call, and a branch chip when a worktree exists.

## Alternatives considered

- **Always run every task in a worktree**: impossible without a project, and it
  would change the behavior of every existing task and channel turn. Opt-in
  keeps the shared workspace as the default.
- **One worktree per crew task**: three checkouts for one run defeats
  isolation; the run group owns one checkout and all three agents share it, as
  they share one repository.
- **Put `.cowbot/` in `.gitignore`**: modifies a tracked file and would show up
  as a change to commit. `.git/info/exclude` is local to the checkout.
- **Create the worktree at task creation**: the branch point should be the
  project's state when the user runs the task, and a user may queue tasks long
  before running them. Prepare at run time avoids stale checkouts.
- **Merge back or open a pull request from the app**: needs remote credentials,
  conflict handling, and review UX. Deferred; the worktree remains a normal
  branch the user can merge outside CowBot.
- **Poll the store for cards**: simple, but the assignment runtime already
  persists every transition; publishing one event per transition is cheaper
  and makes the desktop responsive without a timer.
- **Store the project registry in the workspace database**: projects are paths
  on one device. Keeping them in `config.json` prevents a package import from
  creating paths that do not exist.

## Consequences

- Agents can safely work on real repositories without touching the user's
  checkout; each run gets a branch and a folder that can be inspected and
  removed from task detail.
- The worktree is not merged automatically. Users review the diff and finish
  the branch in their own git tooling.
- A discard is destructive for uncommitted changes; the UI confirms it and
  disables it while the task is running.
- The project registry is per device and per user profile; it is not part of
  workspace backups.
- The run group makes the submitted task a durable summary of a crew run, at
  the cost of keeping an otherwise unassigned task in the task list.
- Event volume grows with assignment transitions and tool calls; the desktop
  debounces refreshes, and the bus already tolerates lagging subscribers.
- Channel turns, remote mode, Docker/WASM sandboxes, and workflow DAGs remain
  out of scope.
- Coverage: `cowbot-store` migration and task/worktree repositories,
  `cowbot-client` worktree methods, `cowbot-service` worktree and assignment
  event tests using temporary git repositories, and the desktop gates.
