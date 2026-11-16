//! Startup safeguards for the local workspace database.
//!
//! The desktop never migrates a database silently. Before opening the local
//! client it compares the database's recorded schema version with the newest
//! migration this build provides:
//!
//! * A database from a newer build is refused, with the choice to restore a
//!   compatible backup or start from an empty workspace.
//! * Pending migrations are applied only after the user confirms the one-way
//!   upgrade; a timestamped copy is taken first.
//! * Any other open failure offers the same recovery instead of leaving the
//!   app to exit without a message.
//!
//! The dialogs come from `rfd` rather than the Tauri dialog plugin because
//! startup runs on the main thread before the event loop can drive an
//! asynchronous dialog, and because a native message box is the only UI that
//! exists when the app cannot open its data.

use std::fs;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cowbot_client::{database_schema, DatabaseSchema, LocalClient};
use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

/// Suffixes SQLite keeps beside the main database file.
const SIDECARS: [&str; 2] = ["-wal", "-shm"];

/// Prefix of the backup folders a user may keep beside the data directory.
const BACKUP_FOLDER_PREFIX: &str = "backup-";

/// How many recovery rounds one start may take before CowBot gives up.
const MAX_ATTEMPTS: u32 = 3;

/// Open the local workspace database, asking before a one-way upgrade and
/// offering recovery when the database cannot be opened.
///
/// Returns `None` when the user chose to quit, so the caller should exit
/// without managing any application state.
pub fn open_local_database(
    db_path: &Path,
    data_dir: &Path,
    locale_preference: Option<&str>,
) -> Option<LocalClient> {
    let locale = Locale::resolve(locale_preference);

    for _ in 0..MAX_ATTEMPTS {
        let schema = match block_on(database_schema(db_path)) {
            Ok(schema) => schema,
            Err(error) => {
                fatal(
                    locale,
                    &locale.cannot_inspect_message(db_path, data_dir, &error.to_string()),
                );
                return None;
            }
        };

        if schema.is_newer() {
            if !recover(locale, db_path, data_dir, Problem::Newer(schema)) {
                return None;
            }
            continue;
        }

        // A new database has nothing to lose, so only an existing one is a
        // one-way upgrade that needs consent.
        let upgrade = schema.current.is_some() && schema.needs_upgrade();
        if upgrade && !confirm_upgrade(locale, db_path, data_dir, schema) {
            return None;
        }

        match block_on(LocalClient::open(db_path)) {
            Ok(client) => return Some(client),
            Err(error) => {
                let message = error.to_string();
                if !recover(locale, db_path, data_dir, Problem::OpenFailed(&message)) {
                    return None;
                }
            }
        }
    }

    fatal(locale, &locale.too_many_attempts_message(db_path, data_dir));
    None
}

fn block_on<F: Future>(future: F) -> F::Output {
    tauri::async_runtime::block_on(future)
}

/// What went wrong when the database could not be opened.
enum Problem<'a> {
    /// The database records a schema version this build does not provide.
    Newer(DatabaseSchema),
    /// Opening failed for any other reason.
    OpenFailed(&'a str),
}

/// Ask before applying a one-way migration, then keep a copy of the database.
fn confirm_upgrade(
    locale: Locale,
    db_path: &Path,
    data_dir: &Path,
    schema: DatabaseSchema,
) -> bool {
    let current = schema.current.unwrap_or_default();
    let backup = unique_path(&labelled_path(db_path, &format!("backup-{current}")));
    let upgrade = locale.upgrade_button().to_string();
    let quit = locale.quit_button().to_string();

    let result = MessageDialog::new()
        .set_level(MessageLevel::Warning)
        .set_title(locale.upgrade_title())
        .set_description(locale.upgrade_message(current, schema.target, &backup))
        .set_buttons(MessageButtons::OkCancelCustom(upgrade.clone(), quit))
        .show();
    if !affirmative(result, &upgrade) {
        return false;
    }

    if let Err(error) = backup_database(db_path, &backup) {
        fatal(
            locale,
            &locale.fatal_message(
                &locale.backup_failed_message(&backup, &error),
                db_path,
                data_dir,
            ),
        );
        return false;
    }
    true
}

/// Offer the recovery choices for a database that cannot be opened.
///
/// Returns `true` when the database was replaced and the caller should try
/// again, and `false` when the user chose to quit or the recovery failed.
fn recover(locale: Locale, db_path: &Path, data_dir: &Path, problem: Problem<'_>) -> bool {
    let backups = compatible_backups(db_path, data_dir);
    let rejected = db_path
        .exists()
        .then(|| unique_path(&labelled_path(db_path, "rejected")));

    if let Some(backup) = backups.first() {
        let restore = locale.restore_button().to_string();
        let fresh = locale.fresh_button().to_string();
        let quit = locale.quit_button().to_string();
        let message = locale.recovery_message(&problem, Some(backup), rejected.as_deref());
        let result = MessageDialog::new()
            .set_level(MessageLevel::Warning)
            .set_title(locale.recovery_title())
            .set_description(message)
            .set_buttons(MessageButtons::YesNoCancelCustom(
                restore.clone(),
                fresh.clone(),
                quit.clone(),
            ))
            .show();

        match choose(result, &restore, &fresh) {
            Answer::Yes => match restore_database(backup, db_path, rejected.as_deref()) {
                Ok(()) => true,
                Err(error) => {
                    fatal(
                        locale,
                        &locale.fatal_message(
                            &locale.restore_failed_message(backup, &error),
                            db_path,
                            data_dir,
                        ),
                    );
                    false
                }
            },
            Answer::No => start_fresh(locale, db_path, data_dir, rejected.as_deref()),
            Answer::Cancel => false,
        }
    } else {
        let fresh = locale.fresh_button().to_string();
        let quit = locale.quit_button().to_string();
        let message = locale.recovery_message(&problem, None, rejected.as_deref());
        let result = MessageDialog::new()
            .set_level(MessageLevel::Warning)
            .set_title(locale.recovery_title())
            .set_description(message)
            .set_buttons(MessageButtons::OkCancelCustom(fresh.clone(), quit))
            .show();

        if affirmative(result, &fresh) {
            start_fresh(locale, db_path, data_dir, rejected.as_deref())
        } else {
            false
        }
    }
}

fn start_fresh(locale: Locale, db_path: &Path, data_dir: &Path, rejected: Option<&Path>) -> bool {
    let Some(rejected) = rejected else {
        return true;
    };
    match move_database_aside(db_path, rejected) {
        Ok(()) => true,
        Err(error) => {
            fatal(
                locale,
                &locale.fatal_message(
                    &locale.start_fresh_failed_message(db_path, &error),
                    db_path,
                    data_dir,
                ),
            );
            false
        }
    }
}

/// Report a startup failure that is not about the database itself.
pub fn report_startup_error(message: &str) {
    let locale = Locale::system();
    MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title(locale.fatal_title())
        .set_description(message.to_string())
        .set_buttons(MessageButtons::OkCustom(locale.quit_button().to_string()))
        .show();
}

fn fatal(locale: Locale, message: &str) {
    MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title(locale.fatal_title())
        .set_description(message)
        .set_buttons(MessageButtons::OkCustom(locale.quit_button().to_string()))
        .show();
}

fn affirmative(result: MessageDialogResult, label: &str) -> bool {
    matches!(result, MessageDialogResult::Ok | MessageDialogResult::Yes)
        || matches!(&result, MessageDialogResult::Custom(value) if value == label)
}

enum Answer {
    Yes,
    No,
    Cancel,
}

fn choose(result: MessageDialogResult, yes: &str, no: &str) -> Answer {
    match result {
        MessageDialogResult::Yes | MessageDialogResult::Ok => Answer::Yes,
        MessageDialogResult::No => Answer::No,
        MessageDialogResult::Custom(label) => {
            if label == yes {
                Answer::Yes
            } else if label == no {
                Answer::No
            } else {
                Answer::Cancel
            }
        }
        MessageDialogResult::Cancel => Answer::Cancel,
    }
}

/// Copy a database and its sidecars to `target`.
fn backup_database(db_path: &Path, target: &Path) -> io::Result<()> {
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

/// Put a backup in place of the database, keeping the replaced file.
///
/// The backup is copied beside the database first, so a failed copy leaves the
/// current database untouched.
fn restore_database(backup: &Path, db_path: &Path, rejected: Option<&Path>) -> io::Result<()> {
    let staged = unique_path(&labelled_path(db_path, "restore"));
    fs::copy(backup, &staged)?;
    for suffix in SIDECARS {
        let source = sidecar_path(backup, suffix);
        if source.exists() {
            fs::copy(&source, sidecar_path(&staged, suffix))?;
        }
    }

    if db_path.exists() {
        let rejected = rejected
            .map(Path::to_path_buf)
            .unwrap_or_else(|| unique_path(&labelled_path(db_path, "rejected")));
        move_database_aside(db_path, &rejected)?;
    }

    fs::rename(&staged, db_path)?;
    for suffix in SIDECARS {
        let source = sidecar_path(&staged, suffix);
        if source.exists() {
            fs::rename(&source, sidecar_path(db_path, suffix))?;
        }
    }
    Ok(())
}

/// Database copies this build could open, newest first.
fn compatible_backups(db_path: &Path, data_dir: &Path) -> Vec<PathBuf> {
    backup_candidates(db_path, data_dir)
        .into_iter()
        .filter(|candidate| {
            block_on(database_schema(candidate))
                .map(|schema| !schema.is_newer())
                .unwrap_or(false)
        })
        .collect()
}

/// Backup files beside the database and backup folders under the data directory.
fn backup_candidates(db_path: &Path, data_dir: &Path) -> Vec<PathBuf> {
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

    if let Ok(entries) = fs::read_dir(data_dir) {
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

/// Language of the startup dialogs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Locale {
    En,
    ZhCn,
    ZhTw,
}

impl Locale {
    fn resolve(preference: Option<&str>) -> Self {
        match preference.map(str::trim) {
            Some("en") => Self::En,
            Some("zh-CN") => Self::ZhCn,
            Some("zh-TW") => Self::ZhTw,
            _ => Self::system(),
        }
    }

    fn system() -> Self {
        let tag = sys_locale::get_locale()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if tag.starts_with("zh") {
            let traditional = tag.contains("hant")
                || tag.contains("tw")
                || tag.contains("hk")
                || tag.contains("mo");
            if traditional {
                Self::ZhTw
            } else {
                Self::ZhCn
            }
        } else {
            Self::En
        }
    }

    fn upgrade_title(self) -> &'static str {
        match self {
            Self::En => "Database upgrade",
            Self::ZhCn => "数据库升级",
            Self::ZhTw => "資料庫升級",
        }
    }

    fn recovery_title(self) -> &'static str {
        match self {
            Self::En => "CowBot cannot open its workspace database",
            Self::ZhCn => "CowBot 无法打开工作区数据库",
            Self::ZhTw => "CowBot 無法開啟工作區資料庫",
        }
    }

    fn fatal_title(self) -> &'static str {
        match self {
            Self::En => "CowBot could not start",
            Self::ZhCn => "CowBot 无法启动",
            Self::ZhTw => "CowBot 無法啟動",
        }
    }

    fn upgrade_button(self) -> &'static str {
        match self {
            Self::En => "Upgrade",
            Self::ZhCn => "升级",
            Self::ZhTw => "升級",
        }
    }

    fn restore_button(self) -> &'static str {
        match self {
            Self::En => "Restore backup",
            Self::ZhCn => "恢复备份",
            Self::ZhTw => "還原備份",
        }
    }

    fn fresh_button(self) -> &'static str {
        match self {
            Self::En => "Start fresh",
            Self::ZhCn => "使用空白工作区",
            Self::ZhTw => "使用空白工作區",
        }
    }

    fn quit_button(self) -> &'static str {
        match self {
            Self::En => "Quit",
            Self::ZhCn => "退出",
            Self::ZhTw => "結束",
        }
    }

    fn upgrade_message(self, current: i64, target: i64, backup: &Path) -> String {
        let version = env!("CARGO_PKG_VERSION");
        match self {
            Self::En => format!(
                "This workspace database uses schema version {current}. CowBot {version} \
                 upgrades it to schema version {target}.\n\n\
                 The upgrade is one-way: an older CowBot cannot open the upgraded database. \
                 Before upgrading, CowBot copies the current database to:\n{}\n\n\
                 Upgrade now?",
                backup.display()
            ),
            Self::ZhCn => format!(
                "当前工作区数据库的结构版本是 {current}，CowBot {version} 会把它升级到结构版本 \
                 {target}。\n\n\
                 升级是单向的：旧版 CowBot 无法打开升级后的数据库。升级前会先把当前数据库复制到：\n{}\n\n\
                 现在升级吗？",
                backup.display()
            ),
            Self::ZhTw => format!(
                "目前工作區資料庫的結構版本是 {current}，CowBot {version} 會將它升級到結構版本 \
                 {target}。\n\n\
                 升級是單向的：舊版 CowBot 無法開啟升級後的資料庫。升級前會先將目前資料庫複製到：\n{}\n\n\
                 現在升級嗎？",
                backup.display()
            ),
        }
    }

    fn recovery_message(
        self,
        problem: &Problem<'_>,
        backup: Option<&Path>,
        rejected: Option<&Path>,
    ) -> String {
        let intro = match (self, problem) {
            (Self::En, Problem::Newer(schema)) => format!(
                "This database uses schema version {}, but this CowBot only provides schema \
                 version {}. It was probably opened by a newer CowBot, and opening it with this \
                 version could damage it.",
                schema.current.unwrap_or_default(),
                schema.target
            ),
            (Self::En, Problem::OpenFailed(error)) => {
                format!("CowBot could not open its workspace database.\n\n{error}")
            }
            (Self::ZhCn, Problem::Newer(schema)) => format!(
                "这个数据库的结构版本是 {}，当前 CowBot 只提供到结构版本 {}。它可能被更新版本的 \
                 CowBot 打开过，继续用这个版本打开可能损坏数据。",
                schema.current.unwrap_or_default(),
                schema.target
            ),
            (Self::ZhCn, Problem::OpenFailed(error)) => {
                format!("CowBot 无法打开工作区数据库。\n\n{error}")
            }
            (Self::ZhTw, Problem::Newer(schema)) => format!(
                "這個資料庫的結構版本是 {}，目前 CowBot 只提供到結構版本 {}。它可能被更新版本的 \
                 CowBot 開啟過，繼續用這個版本開啟可能損壞資料。",
                schema.current.unwrap_or_default(),
                schema.target
            ),
            (Self::ZhTw, Problem::OpenFailed(error)) => {
                format!("CowBot 無法開啟工作區資料庫。\n\n{error}")
            }
        };

        let hint = match (self, backup, rejected) {
            (Self::En, Some(backup), Some(rejected)) => format!(
                "A compatible backup is available:\n{}\n\n\
                 Restore it and continue, or start with an empty workspace. The current database \
                 is kept here:\n{}",
                backup.display(),
                rejected.display()
            ),
            (Self::En, Some(backup), None) => format!(
                "A compatible backup is available:\n{}\n\n\
                 Restore it and continue, or start with an empty workspace.",
                backup.display()
            ),
            (Self::En, None, Some(rejected)) => format!(
                "Start with an empty workspace and leave the current database untouched? It is \
                 kept here:\n{}",
                rejected.display()
            ),
            (Self::En, None, None) => "Start with an empty workspace?".to_string(),
            (Self::ZhCn, Some(backup), Some(rejected)) => format!(
                "找到一个兼容的备份：\n{}\n\n\
                 可以恢复该备份后继续，或改用空白工作区。当前数据库会保留在这里：\n{}",
                backup.display(),
                rejected.display()
            ),
            (Self::ZhCn, Some(backup), None) => format!(
                "找到一个兼容的备份：\n{}\n\n可以恢复该备份后继续，或改用空白工作区。",
                backup.display()
            ),
            (Self::ZhCn, None, Some(rejected)) => format!(
                "要改用空白工作区吗？当前数据库会保留在这里：\n{}",
                rejected.display()
            ),
            (Self::ZhCn, None, None) => "要改用空白工作区吗？".to_string(),
            (Self::ZhTw, Some(backup), Some(rejected)) => format!(
                "找到一個相容的備份：\n{}\n\n\
                 可以還原該備份後繼續，或改用空白工作區。目前資料庫會保留在這裡：\n{}",
                backup.display(),
                rejected.display()
            ),
            (Self::ZhTw, Some(backup), None) => format!(
                "找到一個相容的備份：\n{}\n\n可以還原該備份後繼續，或改用空白工作區。",
                backup.display()
            ),
            (Self::ZhTw, None, Some(rejected)) => format!(
                "要改用空白工作區嗎？目前資料庫會保留在這裡：\n{}",
                rejected.display()
            ),
            (Self::ZhTw, None, None) => "要改用空白工作區嗎？".to_string(),
        };

        format!("{intro}\n\n{hint}")
    }

    fn cannot_inspect_message(self, db_path: &Path, data_dir: &Path, error: &str) -> String {
        let detail = match self {
            Self::En => format!("CowBot could not read the database schema.\n\n{error}"),
            Self::ZhCn => format!("CowBot 无法读取数据库结构版本。\n\n{error}"),
            Self::ZhTw => format!("CowBot 無法讀取資料庫結構版本。\n\n{error}"),
        };
        self.fatal_message(&detail, db_path, data_dir)
    }

    fn too_many_attempts_message(self, db_path: &Path, data_dir: &Path) -> String {
        let detail = match self {
            Self::En => "CowBot could not open its workspace database after several attempts.",
            Self::ZhCn => "CowBot 多次尝试后仍无法打开工作区数据库。",
            Self::ZhTw => "CowBot 多次嘗試後仍無法開啟工作區資料庫。",
        };
        self.fatal_message(detail, db_path, data_dir)
    }

    fn backup_failed_message(self, backup: &Path, error: &io::Error) -> String {
        match self {
            Self::En => format!(
                "CowBot could not back up the database before upgrading, so it did not upgrade \
                 it.\n\n{}\n\n{error}",
                backup.display()
            ),
            Self::ZhCn => format!(
                "CowBot 无法在升级前备份数据库，因此没有升级。\n\n{}\n\n{error}",
                backup.display()
            ),
            Self::ZhTw => format!(
                "CowBot 無法在升級前備份資料庫，因此沒有升級。\n\n{}\n\n{error}",
                backup.display()
            ),
        }
    }

    fn restore_failed_message(self, backup: &Path, error: &io::Error) -> String {
        match self {
            Self::En => format!(
                "CowBot could not restore the backup:\n{}\n\n{error}",
                backup.display()
            ),
            Self::ZhCn => format!("CowBot 无法恢复备份：\n{}\n\n{error}", backup.display()),
            Self::ZhTw => format!("CowBot 無法還原備份：\n{}\n\n{error}", backup.display()),
        }
    }

    fn start_fresh_failed_message(self, db_path: &Path, error: &io::Error) -> String {
        match self {
            Self::En => format!(
                "CowBot could not move the current database aside:\n{}\n\n{error}",
                db_path.display()
            ),
            Self::ZhCn => format!(
                "CowBot 无法移走当前数据库：\n{}\n\n{error}",
                db_path.display()
            ),
            Self::ZhTw => format!(
                "CowBot 無法移開目前資料庫：\n{}\n\n{error}",
                db_path.display()
            ),
        }
    }

    fn fatal_message(self, detail: &str, db_path: &Path, data_dir: &Path) -> String {
        match self {
            Self::En => format!(
                "{detail}\n\nDatabase: {}\nData folder: {}",
                db_path.display(),
                data_dir.display()
            ),
            Self::ZhCn => format!(
                "{detail}\n\n数据库：{}\n数据目录：{}",
                db_path.display(),
                data_dir.display()
            ),
            Self::ZhTw => format!(
                "{detail}\n\n資料庫：{}\n資料目錄：{}",
                db_path.display(),
                data_dir.display()
            ),
        }
    }
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
        let target = temp.path().join("cowbot.db.rejected-1");

        move_database_aside(&db, &target).unwrap();

        assert!(!db.exists());
        assert!(!sidecar_path(&db, "-wal").exists());
        assert_eq!(fs::read(&target).unwrap(), b"db");
        assert_eq!(fs::read(sidecar_path(&target, "-wal")).unwrap(), b"wal");
        assert_eq!(fs::read(sidecar_path(&target, "-shm")).unwrap(), b"shm");
    }

    #[test]
    fn restoring_a_backup_keeps_the_database_it_replaces() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("cowbot.db");
        write(&db, b"current");
        write(&sidecar_path(&db, "-wal"), b"current-wal");
        let backup = temp.path().join("cowbot.db.backup-17-1");
        write(&backup, b"backup");
        write(&sidecar_path(&backup, "-wal"), b"backup-wal");
        let rejected = temp.path().join("cowbot.db.rejected-1");

        restore_database(&backup, &db, Some(&rejected)).unwrap();

        assert_eq!(fs::read(&db).unwrap(), b"backup");
        assert_eq!(fs::read(sidecar_path(&db, "-wal")).unwrap(), b"backup-wal");
        assert_eq!(fs::read(&rejected).unwrap(), b"current");
        assert_eq!(
            fs::read(sidecar_path(&rejected, "-wal")).unwrap(),
            b"current-wal"
        );
        // The backup itself stays where it was.
        assert_eq!(fs::read(&backup).unwrap(), b"backup");
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
    fn unique_paths_do_not_overwrite_an_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cowbot.db.backup-17-1");
        write(&path, b"taken");

        let next = unique_path(&path);

        assert_ne!(next, path);
        assert!(!next.exists());
    }

    #[test]
    fn locale_preferences_resolve_before_the_system_language() {
        assert!(matches!(Locale::resolve(Some("zh-CN")), Locale::ZhCn));
        assert!(matches!(Locale::resolve(Some("zh-TW")), Locale::ZhTw));
        assert!(matches!(Locale::resolve(Some("en")), Locale::En));
    }
}
