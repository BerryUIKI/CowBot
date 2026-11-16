# ADR 0051: Database lifecycle safeguards for the desktop

- Status: Accepted
- Date: 2026-11-16
- Related: [ADR-0025](0025-database-migration-checksum-compatibility.md),
  [ADR-0030](0030-conversation-sessions-and-context.md),
  [ADR-0050](0050-desktop-interface-localisation.md);
  `crates/cowbot-store/src/migrate.rs`, `desktop/src-tauri/src/startup.rs`
- Supersedes: None. Amends the newer-database behaviour recorded in ADR-0025.

## Context

Migrations are forward-only and recorded in `_sqlx_migrations`. A development
build (0.1.10, schema 20) and the installed release (0.1.9, schema 17) shared one
data directory, `%APPDATA%\com.cowbot.desktop`, because both used the same bundle
identifier. Running the development build migrated the database to schema 20;
the next launch of the installed release failed in `Store::migrate` with sqlx's
`migration 18 was previously applied but is missing in the resolved migrations`.
Because the error propagated out of Tauri's `setup` into `.expect(...)`, the
windowed Windows build exited with no window and no message.

Three separate problems produced that outcome:

1. Debug and release builds share a data directory, so a development run can
   upgrade the database an installed release still needs.
2. A database from a newer build is reported as a terse sqlx error, and pending
   migrations are applied without warning even though they are one-way.
3. A startup failure is invisible in a windowed build, and the user gets no
   recovery path when the database cannot be opened.

## Decision

### 1. Debug builds get their own data directory

`config::resolve_data_dir` resolves the directory holding `config.json` and the
workspace databases:

- `COWBOT_DATA_DIR`, when set and non-empty. A relative value is resolved under
  the app data directory.
- Otherwise `app_data_dir()/dev` for a debug build, which is what `tauri dev`
  and `cargo run` produce.
- Otherwise `app_data_dir()`, unchanged for installed releases.

The legacy `com.cowai.desktop` copy runs only against the release data
directory, so a development run cannot recreate it inside `dev`.

### 2. The store names both schema versions

`cowbot-store` exposes `latest_version()` (the newest migration compiled into
the build) and `database_version(path)` (the newest migration recorded in a
file, read without creating, migrating, or otherwise changing it).
`Store::migrate` now refuses a database whose newest recorded version is newer
than the build's and returns one storage error that names both versions and the
recovery steps. A missing version *below* the build's newest is a different
condition (a gap in a fork), and is still left for sqlx to report.

`cowbot-client` wraps both in `database_schema(path)` and `DatabaseSchema`, so a
caller can plan before opening the database, and re-exports the newest version
for the About section.

### 3. The window asks before a one-way upgrade

`initialize` resolves the paths and inspects the schema before the event loop
starts, but it opens the client only when the database needs no decision. The
window then shows a startup screen rendered by the React app:

- **Pending migrations.** The screen names the current and target schema
  versions and states that an older CowBot cannot open the upgraded database.
  The user chooses **Upgrade** (migrate directly), **Back up and upgrade**
  (copy the database and its sidecars to `<db>.backup-<from>-<timestamp>`
  first), or **Quit CowBot**.
- **Newer than the build.** The screen lists the compatible backups; the user
  restores one, starts with an empty workspace, or quits.
- **Any other open failure.** The screen shows the error with the same recovery
  choices.

Recovery never deletes a file. A replaced database is moved to
`<db>.replaced-<timestamp>` beside the original. Backups are discovered beside
the database (`<db>.backup-*`) and in `backup-*/` folders under the database
folder; only one whose recorded version is not newer than the build is offered,
so the conditional rollback cannot restore another unusable database.

### 4. The startup screen is part of the app

The window is created before initialization, so the React app can render the
startup screen with the ordinary title bar, the trilingual catalogues from
ADR-0050, and the app's styles. The commands `get_startup_status` and
`resolve_startup` carry the decision; `resolve_startup` installs the application
state once the database opens. No native dialog is involved, so the screen
matches the rest of the interface and needs no second set of strings.

### 5. The schema version is separate from the app version

The startup screen and Settings → About speak of a database **schema version**
(the migration number), never the app version. `get_app_info` reports it as
`databaseSchema`, so a bug report can name the schema and the app version
independently.

### 6. The first-run guide can move the database folder

`AppConfig` gains `database_dir`. The welcome step shows the current folder and
opens a folder picker; choosing one writes the absolute path, clears the active
workspace, and restarts into a `cowbot.db` in that folder. A value that is not
absolute is ignored, so a hand-edited configuration cannot move databases to a
relative path. The folder is also where backup discovery looks.

### 7. One instance per machine

`tauri-plugin-single-instance` makes a second launch exit and focus the open
window, so two processes cannot open the same workspace database. The plugin is
registered first, as it requires.

## Alternatives considered

- **`Migrator::ignore_missing(true)`.** Would let an older build open a newer
  database and run against a schema it does not know. Rejected: hidden data
  damage is worse than a refusal.
- **A separate `tauri dev` identifier passed with `--config`.** Isolates the
  data directory, but every developer and every command would have to remember
  it. The automatic debug directory needs no discipline; `COWBOT_DATA_DIR`
  remains for deliberate sharing.
- **Native message dialogs (`rfd`).** Rejected: they cannot use the app's
  catalogues or styles, and a synchronous TaskDialog called from Tauri's
  run-event callback returned its first button without showing in practice.
- **Consent in `LocalClient::open`.** Would cover the CLI too, but breaks every
  test and embedded caller and changes a transport-neutral API. The CLI is an
  operator tool and keeps migrating automatically; it still receives the store's
  explicit newer-database error.
- **Deleting the database when the user starts fresh.** Moving it aside costs
  nothing and keeps the decision reversible.
- **Never offering a backup during recovery.** Would leave a dead end when the
  user has no backup of their own; discovery plus a schema check is a few lines.
- **Always copying the database before an upgrade.** Costs disk space and time
  on large workspaces; the confirmation offers it as a choice instead.

## Consequences

- `tauri dev` starts with its own workspace and provider configuration; the
  first development launch after this change needs onboarding again. Set
  `COWBOT_DATA_DIR` to share an installed workspace on purpose.
- Every schema upgrade shows one screen; the user chooses whether to keep a
  backup copy. A user who declines exits cleanly instead of migrating silently.
- The startup screen uses the same trilingual catalogues as the rest of the app,
  so a new interface language needs no separate strings.
- The welcome step can move the workspace databases to another folder and
  restarts the app to apply it. An existing workspace is not moved; the new
  folder starts with its own `cowbot.db`.
- A second launch focuses the running window instead of opening a second copy,
  including a debug build while an installed release runs, because both use the
  same bundle identifier.
- A backup kept outside the database folder is not discovered; copy it in or
  rename it to `<db>.backup-*` first.
- The desktop still refuses a database whose newest *recorded* migration is
  newer, even if the user would accept data loss; starting fresh is the offered
  way forward.
- Coverage: `crates/cowbot-store/tests/migrations.rs` covers the newer-database
  refusal and read-only version inspection; `desktop/src-tauri/src/startup.rs`
  covers backup discovery, copying, moving aside, and unique names;
  `config.rs` covers data-directory and database-folder resolution.
