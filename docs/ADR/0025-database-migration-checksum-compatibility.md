# ADR 0025: Database migration checksum compatibility

- Status: Accepted
- Date: 2026-11-09
- Related: `crates/cowbot-store/src/migrate.rs`, `crates/cowbot-store/migrations/`, `crates/cowbot-store/tests/migrations.rs`

## Context

`cowbot-store` embeds its SQL migrations at compile time with `sqlx::migrate!`. sqlx records the SHA-384 of each migration file's exact bytes in `_sqlx_migrations` and refuses to start when a previously applied file no longer matches, reporting `migration N was previously applied but has been modified`.

The checksum covers file bytes, not SQL semantics. On Windows, a checkout with `core.autocrlf=true` materializes `migrations/*.sql` with CRLF line endings, while the repository and release builds use LF. Both variants contain the same SQL and produce the same SQLite schema, but their checksums differ. A workspace created by such a build then failed on later builds compiled with LF checksums. The affected database holds local single-user data (identity, channels, messages, agents), so resetting or discarding it is not acceptable.

Editing migrations 0001 and 0002 to match one particular variant cannot solve this: it rewrites history, changes the checksum for every existing database, and breaks the databases that currently match the other variant.

## Decision

`Store::migrate` runs a narrow compatibility check before delegating to sqlx:

1. Read the applied rows from `_sqlx_migrations`. If the table is absent or every checksum matches, proceed unchanged.
2. A mismatch is accepted only when the recorded checksum equals the SHA-384 of the current embedded migration SQL with LF **or** CRLF line endings. Both variants are computed from the embedded SQL, so the check follows the migration files instead of hard-coding historical checksums.
3. Before rewriting anything, the live `sqlite_master` schema (tables and indexes, with line endings normalized) must exactly match a fresh in-memory application of the embedded migrations up to the highest recorded version.
4. Only then is the `checksum` column updated, for all mismatched versions in one transaction. Migration SQL and the recorded history are otherwise untouched, and sqlx then performs its normal run.

Any other mismatch — unknown content, a failed migration, or a schema that differs — leaves the database untouched and returns a storage error naming the affected migration, the first schema difference, and the standard recovery steps (back up the database, create or import a workspace, report the message). A database created by a newer build is left for sqlx to report as a missing migration.

The repository also pins `crates/cowbot-store/migrations/*.sql` to `text eol=lf` in `.gitattributes` so new checkouts produce the same bytes on every platform.

## Alternatives considered

- **Rewrite migrations 0001/0002**: rewrites migration history, changes the checksum for every database, and cannot satisfy both line-ending variants at once.
- **Delete or rewrite the affected `_sqlx_migrations` rows**: loses the record of what was applied and skips validation; a genuinely divergent database would then run later migrations against an unknown schema.
- **A separate repair command or one-time script**: requires users to discover and run it and provides no schema validation; startup would still fail by default.
- **Load migrations at runtime and normalize line endings**: removes the dependency on checkout state, but abandons compile-time embedding, makes the binary depend on files on disk, and is a much larger change for the same outcome.
- **Disable checksum validation**: would also hide genuine migration drift.

## Consequences

- Affected local databases self-heal on the next launch; user data and migration history are preserved.
- The repair is idempotent: once the checksums match, the compatibility check is a no-op.
- A database that genuinely diverged from its recorded migrations fails with a message that names the first schema difference and recovery steps instead of sqlx's terse error.
- The schema comparison normalizes CRLF/LF only; any real SQL or object difference is still rejected.
- Coverage: `crates/cowbot-store/tests/migrations.rs` builds CRLF migration databases and covers repair, repair with a pending migration, schema divergence, unknown checksums, and fresh databases.
- If a future migration uses a construct that `raw_sql` cannot replay for the reference schema, the comparison path must be revisited.
