//! Migration checksum compatibility tests.
//!
//! These build databases with CRLF migration files, which is what a Windows
//! checkout using `core.autocrlf=true` produced before the repository pinned
//! the migration files to LF. The SQL and schema are identical to the embedded
//! migrations; only the recorded SHA-384 checksums differ.

use std::fs;
use std::path::{Path, PathBuf};

use cowbot_store::Store;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};

const MIGRATION_FILES: [&str; 2] = ["0001_init.sql", "0002_assignments.sql"];

fn migrations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations")
}

/// Copy the shipped migrations into `root` with CRLF line endings.
fn write_crlf_migrations(root: &Path, files: &[&str]) -> PathBuf {
    let target = root.join("migrations");
    fs::create_dir_all(&target).unwrap();
    for name in files {
        let source = migrations_dir().join(name);
        let contents = fs::read_to_string(&source).unwrap();
        let crlf = contents.replace("\r\n", "\n").replace('\n', "\r\n");
        fs::write(target.join(name), crlf).unwrap();
    }
    target
}

async fn create_legacy_database(path: &Path, root: &Path, files: &[&str]) {
    let migrator = Migrator::new(write_crlf_migrations(root, files))
        .await
        .unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    migrator.run(&pool).await.unwrap();
    pool.close().await;
}

async fn recorded_checksums(pool: &SqlitePool) -> Vec<(i64, Vec<u8>)> {
    sqlx::query_as::<_, (i64, Vec<u8>)>(
        "SELECT version, checksum FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

fn embedded_checksums() -> Vec<(i64, Vec<u8>)> {
    sqlx::migrate!("./migrations")
        .iter()
        .map(|migration| (migration.version, migration.checksum.to_vec()))
        .collect()
}

#[tokio::test]
async fn repairs_crlf_checksums_after_schema_validation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("workspace.db");
    create_legacy_database(&path, temp.path(), &MIGRATION_FILES).await;

    // User data written by the old build must survive the repair.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    sqlx::query("INSERT INTO workspaces (id, name, created_at) VALUES (?, ?, ?)")
        .bind("workspace-1")
        .bind("Legacy workspace")
        .bind("2026-11-05T00:00:00Z")
        .execute(&pool)
        .await
        .unwrap();
    let before = recorded_checksums(&pool).await;
    pool.close().await;
    assert_ne!(before, embedded_checksums());

    let store = Store::connect(&path).await.unwrap();
    store.migrate().await.unwrap();

    assert_eq!(recorded_checksums(store.pool()).await, embedded_checksums());
    let name: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id = 'workspace-1'")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(name, "Legacy workspace");

    // Running again is a no-op.
    store.migrate().await.unwrap();
    assert_eq!(recorded_checksums(store.pool()).await, embedded_checksums());
}

#[tokio::test]
async fn repairs_crlf_checksums_with_pending_migrations() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("workspace.db");
    // A database created before migration 2 existed.
    create_legacy_database(&path, temp.path(), &MIGRATION_FILES[..1]).await;

    let store = Store::connect(&path).await.unwrap();
    store.migrate().await.unwrap();

    assert_eq!(recorded_checksums(store.pool()).await, embedded_checksums());
}

#[tokio::test]
async fn rejects_crlf_checksums_when_the_schema_diverges() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("workspace.db");
    create_legacy_database(&path, temp.path(), &MIGRATION_FILES).await;

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    sqlx::query("DROP INDEX idx_tasks_channel")
        .execute(&pool)
        .await
        .unwrap();
    let before = recorded_checksums(&pool).await;
    pool.close().await;

    let store = Store::connect(&path).await.unwrap();
    let error = store.migrate().await.unwrap_err().to_string();

    assert!(error.contains("does not match the embedded migrations"));
    assert!(error.contains("missing index 'idx_tasks_channel'"));
    assert!(error.contains("Back up the workspace database"));
    assert_eq!(recorded_checksums(store.pool()).await, before);
}

#[tokio::test]
async fn rejects_unknown_checksums_without_touching_the_database() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("workspace.db");

    let store = Store::connect(&path).await.unwrap();
    store.migrate().await.unwrap();

    let unknown = vec![0xAA_u8; 48];
    sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = 1")
        .bind(&unknown)
        .execute(store.pool())
        .await
        .unwrap();

    let error = store.migrate().await.unwrap_err().to_string();
    assert!(error.contains("previously applied with different content"));
    assert!(error.contains("Recovery steps"));
    assert_eq!(
        recorded_checksums(store.pool()).await[0],
        (1, unknown.clone())
    );
}

#[tokio::test]
async fn migrates_a_fresh_database_without_repairs() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("workspace.db");

    let store = Store::connect(&path).await.unwrap();
    store.migrate().await.unwrap();

    assert_eq!(recorded_checksums(store.pool()).await, embedded_checksums());
}
