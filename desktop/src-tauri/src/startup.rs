//! Database preparation for the desktop startup.
//!
//! The desktop never migrates a database silently. Before the event loop
//! starts, [`inspect`] compares the database's recorded schema version with the
//! newest migration this build provides. The result is served to the frontend,
//! which asks the user before a one-way upgrade and offers recovery when the
//! database belongs to a newer build.
//!
//! Recovery moves files, never deletes them: a replaced database is renamed to
//! `<db>.replaced-<timestamp>` and a backup is copied before an upgrade.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cowbot_client::database_schema;
use parking_lot::Mutex;
use serde::Serialize;

/// Suffixes SQLite keeps beside the main database file.
const SIDECARS: [&str; 2] = ["-wal", "-shm"];

/// Prefix of the backup folders a user may keep beside the database.
const BACKUP_FOLDER_PREFIX: &str = "backup-";

/// What the desktop needs to do before it can open the workspace database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum StartupStatus {
    /// The database can be opened without asking.
    Ready,
    /// The database is older than this build and needs a one-way upgrade.
    Upgrade {
        current: i64,
        target: i64,
        database: String,
        /// File that receives a backup before the upgrade runs.
        backup: String,
    },
    /// The database was written by a newer build.
    Newer {
        current: i64,
        target: i64,
        database: String,
        /// Copies this build can open, newest first.
        backups: Vec<String>,
        /// Where the refused database is moved if the user starts fresh.
        replaced: String,
    },
    /// The database could not be inspected.
    Failed {
        message: String,
        database: String,
        backups: Vec<String>,
        replaced: Option<String>,
    },
}

impl StartupStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Paths and state shared by the startup commands until the client is open.
pub struct StartupState {
    /// App data directory that holds `config.json`.
    pub data_dir: PathBuf,
    /// Folder that holds the workspace databases.
    pub database_dir: PathBuf,
    /// Workspace database name relative to `database_dir`.
    pub database: String,
    pub db_path: PathBuf,
    pub config_path: PathBuf,
    status: Mutex<StartupStatus>,
}

impl StartupState {
    pub fn new(
        data_dir: PathBuf,
        database_dir: PathBuf,
        database: String,
        db_path: PathBuf,
        config_path: PathBuf,
        status: StartupStatus,
    ) -> Self {
        Self {
            data_dir,
            database_dir,
            database,
            db_path,
            config_path,
            status: Mutex::new(status),
        }
    }

    /// State for a failure that happened before any path could be resolved.
    pub fn failed(message: String) -> Self {
        Self {
            data_dir: PathBuf::new(),
            database_dir: PathBuf::new(),
            database: String::new(),
            db_path: PathBuf::new(),
            config_path: PathBuf::new(),
            status: Mutex::new(StartupStatus::Failed {
                message,
                database: String::new(),
                backups: Vec::new(),
                replaced: None,
            }),
        }
    }

    pub fn status(&self) -> StartupStatus {
        self.status.lock().clone()
    }

    pub fn set_status(&self, status: StartupStatus) {
        *self.status.lock() = status;
    }

    /// Failure state after the paths are known, with the recovery options.
    pub fn failure(&self, message: String) -> StartupStatus {
        StartupStatus::Failed {
            message,
            database: display(&self.db_path),
            backups: backup_candidates(&self.db_path, &self.database_dir)
                .into_iter()
                .map(|path| display(&path))
                .collect(),
            replaced: self
                .db_path
                .exists()
                .then(|| display(&unique_path(&labelled_path(&self.db_path, "replaced")))),
        }
    }
}

/// Read the database's schema state and decide what startup needs to ask.
pub async fn inspect(db_path: &Path, database_dir: &Path) -> StartupStatus {
    let backups = compatible_backups(db_path, database_dir)
        .await
        .into_iter()
        .map(|path| display(&path))
        .collect::<Vec<_>>();
    let database = display(db_path);
    let replaced = display(&unique_path(&labelled_path(db_path, "replaced")));

    match database_schema(db_path).await {
        Ok(schema) if schema.is_newer() => StartupStatus::Newer {
            current: schema.current.unwrap_or_default(),
            target: schema.target,
            database,
            backups,
            replaced,
        },
        Ok(schema) if schema.current.is_some() && schema.needs_upgrade() => {
            let current = schema.current.unwrap_or_default();
            StartupStatus::Upgrade {
                current,
                target: schema.target,
                database,
                backup: display(&unique_path(&labelled_path(
                    db_path,
                    &format!("backup-{current}"),
                ))),
            }
        }
        Ok(_) => StartupStatus::Ready,
        Err(error) => StartupStatus::Failed {
            message: error.to_string(),
            database,
            backups,
            replaced: db_path.exists().then_some(replaced),
        },
    }
}

/// Copy a database and its sidecars to `target`.
pub fn backup_database(db_path: &Path, target: &Path) -> io::Result<()> {
    fs::copy(db_path, target)?;
    for suffix in SIDECARS {
        let source = sidecar_path(db_path, suffix);
        if source.exists() {
            fs::copy(&source, sidecar_path(target, suffix))?;
        }
    }
    Ok(())
}

/// Move a database and its sidecars to `target`.
fn move_database_aside(db_path: &Path, target: &Path) -> io::Result<()> {
    fs::rename(db_path, target)?;
    for suffix in SIDECARS {
        let source = sidecar_path(db_path, suffix);
        if source.exists() {
            fs::rename(&source, sidecar_path(target, suffix))?;
        }
    }
    Ok(())
}

/// Replace the database with a compatible backup, keeping the old file.
///
/// The backup must be one of the copies [`compatible_backups`] reports, so the
/// frontend cannot point startup at an arbitrary path.
pub async fn restore_from(
    backup: &Path,
    db_path: &Path,
    database_dir: &Path,
) -> Result<(), String> {
    let candidates = compatible_backups(db_path, database_dir).await;
    if !candidates.iter().any(|candidate| candidate == backup) {
        return Err("Choose one of the compatible backups.".to_string());
    }

    let replaced = unique_path(&labelled_path(db_path, "replaced"));
    let staged = unique_path(&labelled_path(db_path, "restore"));
    fs::copy(backup, &staged).map_err(|error| error.to_string())?;
    for suffix in SIDECARS {
        let source = sidecar_path(backup, suffix);
        if source.exists() {
            fs::copy(&source, sidecar_path(&staged, suffix)).map_err(|error| error.to_string())?;
        }
    }

    if db_path.exists() {
        move_database_aside(db_path, &replaced).map_err(|error| error.to_string())?;
    }
    fs::rename(&staged, db_path).map_err(|error| error.to_string())?;
    for suffix in SIDECARS {
        let source = sidecar_path(&staged, suffix);
        if source.exists() {
            fs::rename(&source, sidecar_path(db_path, suffix))
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// Move the database aside so the next open creates an empty one.
pub fn start_fresh(db_path: &Path) -> Result<String, String> {
    if !db_path.exists() {
        return Ok(String::new());
    }
    let replaced = unique_path(&labelled_path(db_path, "replaced"));
    move_database_aside(db_path, &replaced)
        .map(|()| display(&replaced))
        .map_err(|error| error.to_string())
}

/// Database copies this build could open, newest first.
pub async fn compatible_backups(db_path: &Path, database_dir: &Path) -> Vec<PathBuf> {
    let mut compatible = Vec::new();
    for candidate in backup_candidates(db_path, database_dir) {
        if database_schema(&candidate)
            .await
            .map(|schema| !schema.is_newer())
            .unwrap_or(false)
        {
            compatible.push(candidate);
        }
    }
    compatible
}

/// Backup files beside the database and backup folders under the database directory.
fn backup_candidates(db_path: &Path, database_dir: &Path) -> Vec<PathBuf> {
    let Some(name) = db_path.file_name() else {
        return Vec::new();
    };
    let prefix = format!("{}.backup-", name.to_string_lossy());
    let mut candidates = Vec::new();

    if let Some(parent) = db_path.parent() {
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                let file_name = file_name.to_string_lossy();
                if file_name.starts_with(&prefix)
                    && !SIDECARS.iter().any(|suffix| file_name.ends_with(suffix))
                    && entry.path().is_file()
                {
                    candidates.push(entry.path());
                }
            }
        }
    }

    if let Ok(entries) = fs::read_dir(database_dir) {
        for entry in entries.flatten() {
            let folder = entry.path();
            if folder.is_dir()
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(BACKUP_FOLDER_PREFIX)
            {
                let candidate = folder.join(name);
                if candidate.is_file() {
                    candidates.push(candidate);
                }
            }
        }
    }

    candidates.sort_by_key(|path| modified(path));
    candidates.reverse();
    candidates
}

fn modified(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH)
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

/// A sibling of `path` named `<name>.<label>-<seconds>`.
fn labelled_path(path: &Path, label: &str) -> PathBuf {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let name = match path.file_name() {
        Some(name) => format!("{}.{label}-{seconds}", name.to_string_lossy()),
        None => format!("cowbot.{label}-{seconds}"),
    };
    path.with_file_name(name)
}

/// Add a counter when the labelled name is already taken.
fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    for index in 1..1000 {
        let mut value = path.as_os_str().to_owned();
        value.push(format!("-{index}"));
        let candidate = PathBuf::from(value);
        if !candidate.exists() {
            return candidate;
        }
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &[u8]) {
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn finds_backups_beside_the_database_and_in_backup_folders() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("data");
        fs::create_dir_all(&dir).unwrap();
        let db = dir.join("cowbot.db");
        write(&db, b"current");
        write(&dir.join("cowbot.db.backup-17-1"), b"copy");
        write(&dir.join("cowbot.db.backup-17-1-wal"), b"sidecar");
        write(&dir.join("cowbot.db.backup-17-1-shm"), b"sidecar");
        write(&dir.join("other.db.backup-17-1"), b"other");
        let folder = dir.join("backup-20261116-103102");
        fs::create_dir_all(&folder).unwrap();
        write(&folder.join("cowbot.db"), b"folder copy");

        let mut candidates: Vec<PathBuf> = backup_candidates(&db, &dir)
            .into_iter()
            .map(|path| path.strip_prefix(&dir).unwrap().to_path_buf())
            .collect();
        candidates.sort();

        assert_eq!(
            candidates,
            vec![
                PathBuf::from("backup-20261116-103102/cowbot.db"),
                PathBuf::from("cowbot.db.backup-17-1"),
            ]
        );
    }

    #[test]
    fn moving_the_database_aside_keeps_the_sidecars_together() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("cowbot.db");
        write(&db, b"db");
        write(&sidecar_path(&db, "-wal"), b"wal");
        write(&sidecar_path(&db, "-shm"), b"shm");
        let target = temp.path().join("cowbot.db.replaced-1");

        move_database_aside(&db, &target).unwrap();

        assert!(!db.exists());
        assert!(!sidecar_path(&db, "-wal").exists());
        assert_eq!(fs::read(&target).unwrap(), b"db");
        assert_eq!(fs::read(sidecar_path(&target, "-wal")).unwrap(), b"wal");
        assert_eq!(fs::read(sidecar_path(&target, "-shm")).unwrap(), b"shm");
    }

    #[test]
    fn backing_up_copies_the_database_and_its_sidecars() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("cowbot.db");
        write(&db, b"db");
        write(&sidecar_path(&db, "-wal"), b"wal");
        let target = temp.path().join("cowbot.db.backup-17-1");

        backup_database(&db, &target).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"db");
        assert_eq!(fs::read(sidecar_path(&target, "-wal")).unwrap(), b"wal");
        assert_eq!(fs::read(&db).unwrap(), b"db");
    }

    #[test]
    fn starting_fresh_moves_the_database_and_reports_where_it_went() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("cowbot.db");
        write(&db, b"db");
        write(&sidecar_path(&db, "-wal"), b"wal");

        let replaced = start_fresh(&db).unwrap();

        assert!(!db.exists());
        assert_eq!(fs::read(&replaced).unwrap(), b"db");
    }

    #[test]
    fn unique_paths_do_not_overwrite_an_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cowbot.db.backup-17-1");
        write(&path, b"taken");

        let next = unique_path(&path);

        assert_ne!(next, path);
        assert!(!next.exists());
    }
}
