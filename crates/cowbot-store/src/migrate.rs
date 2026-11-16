//! Migration compatibility checks.
//!
//! `sqlx` records a SHA-384 checksum of each migration file's exact bytes and
//! refuses to run when a previously applied migration no longer matches.
//! The checksum therefore depends on the checkout's line endings: a Windows
//! checkout with `core.autocrlf=true` materializes the migration files with
//! CRLF line endings, while the repository and release builds use LF. Both
//! variants contain the same SQL, but their checksums differ.
//!
//! [`run`] repairs the recorded checksum of a database created by the other
//! line-ending variant before handing it to `sqlx`. The repair is deliberately
//! narrow:
//!
//! * the recorded checksum must equal the SHA-384 of the current migration
//!   SQL with either LF or CRLF line endings;
//! * the live schema must exactly match a fresh application of the embedded
//!   migrations up to the highest recorded version;
//! * only the `checksum` column is updated, in one transaction.
//!
//! Anything else is reported as a storage error with recovery steps. Migration
//! files and the recorded migration history are never rewritten.

use std::path::Path;
use std::str::FromStr;

use cowbot_core::error::{CowError, CowResult};
use sha2::{Digest, Sha384};
use sqlx::migrate::{Migration, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

const MIGRATIONS_TABLE: &str = "_sqlx_migrations";

/// Apply all embedded migrations, repairing line-ending checksum variants.
pub(crate) async fn run(pool: &SqlitePool) -> CowResult<()> {
    let Some(applied) = applied_migrations(pool).await? else {
        return MIGRATOR.run(pool).await.map_err(storage_error);
    };
    reject_newer_database(&applied)?;
    repair_compatible_checksums(pool, &applied).await?;
    MIGRATOR.run(pool).await.map_err(storage_error)
}

/// The newest schema version this build provides.
pub fn latest_version() -> i64 {
    MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or_default()
}

/// The newest schema version recorded in a database file.
///
/// The file is read without being created, migrated, or otherwise changed, so
/// a caller can decide whether to open it first. `None` means a new database:
/// the file does not exist or records no migration yet.
pub async fn database_version(path: impl AsRef<Path>) -> CowResult<Option<i64>> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(None);
    }

    let pool = inspect_pool(path).await?;
    let version = applied_migrations(&pool)
        .await
        .map(|applied| applied.and_then(|applied| applied.iter().map(|m| m.version).max()));
    pool.close().await;
    version
}

/// Open an existing database for inspection without writing to it.
///
/// SQLite refuses a read-only connection to a WAL database whose `-shm`
/// sidecar is not readable, so a database left behind by a crash falls back to
/// a read-write connection. Either way only `_sqlx_migrations` is queried.
async fn inspect_pool(path: &Path) -> CowResult<SqlitePool> {
    let read_only = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(true);
    match SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(read_only)
        .await
    {
        Ok(pool) => Ok(pool),
        Err(_) => {
            let read_write = SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(false);
            SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(read_write)
                .await
                .map_err(storage_error)
        }
    }
}

/// Refuse a database that a newer build has already migrated.
fn reject_newer_database(applied: &[AppliedMigration]) -> CowResult<()> {
    let Some(newest) = applied.iter().map(|migration| migration.version).max() else {
        return Ok(());
    };
    let supported = latest_version();
    if newest > supported {
        return Err(newer_database_error(newest, supported));
    }
    Ok(())
}

struct AppliedMigration {
    version: i64,
    checksum: Vec<u8>,
    success: bool,
}

async fn repair_compatible_checksums(
    pool: &SqlitePool,
    applied: &[AppliedMigration],
) -> CowResult<()> {
    if applied.is_empty() {
        return Ok(());
    }

    let mut repairs: Vec<(i64, Vec<u8>)> = Vec::new();
    for recorded in applied {
        let Some(migration) = MIGRATOR
            .iter()
            .find(|migration| migration.version == recorded.version)
        else {
            // A database newer than this build was already rejected. A gap in
            // the embedded set is left for sqlx to report.
            return Ok(());
        };

        if recorded.checksum == migration.checksum.as_ref() {
            continue;
        }
        if !recorded.success {
            // A failed migration is reported by sqlx; never rewrite it.
            continue;
        }

        if line_ending_variants(migration)
            .iter()
            .any(|variant| variant == &recorded.checksum)
        {
            repairs.push((migration.version, migration.checksum.to_vec()));
        } else {
            return Err(migration_content_error(recorded, migration));
        }
    }

    if repairs.is_empty() {
        return Ok(());
    }

    let max_applied_version = applied
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or_default();
    let expected = expected_schema(max_applied_version).await?;
    let actual = schema_objects(pool).await?;
    if actual != expected {
        return Err(schema_mismatch_error(
            max_applied_version,
            &actual,
            &expected,
        ));
    }

    let mut transaction = pool.begin().await.map_err(storage_error)?;
    for (version, checksum) in &repairs {
        sqlx::query(&format!(
            "UPDATE {MIGRATIONS_TABLE} SET checksum = ? WHERE version = ?"
        ))
        .bind(checksum)
        .bind(version)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
    }
    transaction.commit().await.map_err(storage_error)?;

    let versions = repairs
        .iter()
        .map(|(version, _)| version.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    eprintln!(
        "cowbot-store: repaired the recorded checksum of migration(s) {versions}; \
         the database uses a different line-ending variant of the same migration SQL"
    );

    Ok(())
}

async fn applied_migrations(pool: &SqlitePool) -> CowResult<Option<Vec<AppliedMigration>>> {
    let exists: Option<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(MIGRATIONS_TABLE)
            .fetch_optional(pool)
            .await
            .map_err(storage_error)?;
    if exists.is_none() {
        return Ok(None);
    }

    let rows = sqlx::query(&format!(
        "SELECT version, checksum, success FROM {MIGRATIONS_TABLE} ORDER BY version"
    ))
    .fetch_all(pool)
    .await
    .map_err(storage_error)?;

    let applied = rows
        .into_iter()
        .map(|row| {
            Ok(AppliedMigration {
                version: row.try_get("version").map_err(storage_error)?,
                checksum: row.try_get("checksum").map_err(storage_error)?,
                success: row.try_get("success").map_err(storage_error)?,
            })
        })
        .collect::<CowResult<Vec<_>>>()?;
    Ok(Some(applied))
}

/// SHA-384 of the migration SQL with LF and with CRLF line endings.
fn line_ending_variants(migration: &Migration) -> [Vec<u8>; 2] {
    let lf = migration.sql.replace("\r\n", "\n");
    let crlf = lf.replace('\n', "\r\n");
    [digest(lf.as_bytes()), digest(crlf.as_bytes())]
}

fn digest(bytes: &[u8]) -> Vec<u8> {
    Sha384::digest(bytes).to_vec()
}

/// Build a reference schema by applying the embedded migrations up to the
/// given version to a fresh in-memory database.
async fn expected_schema(up_to_version: i64) -> CowResult<Vec<SchemaObject>> {
    let options = SqliteConnectOptions::from_str("sqlite::memory:").map_err(storage_error)?;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(storage_error)?;

    for migration in MIGRATOR
        .iter()
        .filter(|migration| migration.version <= up_to_version)
    {
        sqlx::raw_sql(&migration.sql)
            .execute(&pool)
            .await
            .map_err(storage_error)?;
    }

    let objects = schema_objects(&pool).await?;
    pool.close().await;
    Ok(objects)
}

#[derive(Debug, PartialEq, Eq)]
struct SchemaObject {
    kind: String,
    name: String,
    table: String,
    sql: String,
}

async fn schema_objects(pool: &SqlitePool) -> CowResult<Vec<SchemaObject>> {
    let rows = sqlx::query(
        "SELECT type, name, tbl_name, sql FROM sqlite_master \
         WHERE name NOT LIKE 'sqlite_%' AND name <> ? \
         ORDER BY type, name",
    )
    .bind(MIGRATIONS_TABLE)
    .fetch_all(pool)
    .await
    .map_err(storage_error)?;

    rows.into_iter()
        .map(|row| {
            Ok(SchemaObject {
                kind: row.try_get("type").map_err(storage_error)?,
                name: row.try_get("name").map_err(storage_error)?,
                table: row.try_get("tbl_name").map_err(storage_error)?,
                sql: row
                    .try_get::<Option<String>, _>("sql")
                    .map_err(storage_error)?
                    .unwrap_or_default()
                    // SQLite stores CREATE statements with the exact line
                    // endings they were written with. Line endings do not
                    // change the schema, so they are not a divergence.
                    .replace("\r\n", "\n"),
            })
        })
        .collect()
}

fn newer_database_error(applied: i64, supported: i64) -> CowError {
    CowError::Storage(format!(
        "this database has schema version {applied}, which is newer than this build provides \
         (schema version {supported}). It was most likely opened by a newer CowBot. CowBot left \
         the database unchanged.\n\
         Recovery steps:\n\
         1. Install a CowBot version that provides schema version {applied} or later, or open a \
            different workspace.\n\
         2. Back up the database file and any -wal and -shm sidecars before changing anything.\n\
         3. Do not open this database with an older build."
    ))
}

fn migration_content_error(recorded: &AppliedMigration, migration: &Migration) -> CowError {
    CowError::Storage(format!(
        "database migration {} (\"{}\") was previously applied with different content, and its \
         checksum does not match a known CowBot variant. CowBot left the database unchanged.\n\
         recorded checksum: {}\n\
         expected checksum: {}\n\
         Recovery steps:\n\
         1. Back up the workspace database file and any -wal and -shm sidecars.\n\
         2. Create a new workspace, or import a .cowpkg export to keep working.\n\
         3. If you believe this database is correct, report this message with the database file.\n\
         Do not delete or edit rows in {MIGRATIONS_TABLE} by hand.",
        recorded.version,
        migration.description,
        hex(&recorded.checksum),
        hex(migration.checksum.as_ref()),
    ))
}

fn schema_mismatch_error(
    up_to_version: i64,
    actual: &[SchemaObject],
    expected: &[SchemaObject],
) -> CowError {
    CowError::Storage(format!(
        "database migrations use a known line-ending checksum variant, but the schema does not \
         match the embedded migrations through version {up_to_version}: {}. CowBot left the \
         database unchanged.\n\
         Recovery steps:\n\
         1. Back up the workspace database file and any -wal and -shm sidecars.\n\
         2. Create a new workspace, or import a .cowpkg export to keep working.\n\
         3. If you believe this database is correct, report this message with the database file.\n\
         Do not delete or edit rows in {MIGRATIONS_TABLE} by hand.",
        describe_difference(actual, expected),
    ))
}

fn describe_difference(actual: &[SchemaObject], expected: &[SchemaObject]) -> String {
    for expected_object in expected {
        match actual.iter().find(|object| {
            object.kind == expected_object.kind && object.name == expected_object.name
        }) {
            None => {
                return format!(
                    "missing {} '{}'",
                    expected_object.kind, expected_object.name
                )
            }
            Some(actual_object) if actual_object != expected_object => {
                return format!(
                    "{} '{}' has a different definition",
                    expected_object.kind, expected_object.name
                )
            }
            Some(_) => {}
        }
    }

    for actual_object in actual {
        let expected_object = expected
            .iter()
            .any(|object| object.kind == actual_object.kind && object.name == actual_object.name);
        if !expected_object {
            return format!("unexpected {} '{}'", actual_object.kind, actual_object.name);
        }
    }

    "schema object lists differ".to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn storage_error(error: impl std::fmt::Display) -> CowError {
    CowError::Storage(error.to_string())
}
