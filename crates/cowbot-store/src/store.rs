use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use cowbot_core::error::{CowError, CowResult};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

/// SQLite-backed persistence layer for CowBot.
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Open (and create if needed) a SQLite database at the given path.
    pub async fn connect(path: impl AsRef<Path>) -> CowResult<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .map_err(|err| CowError::Storage(err.to_string()))?;

        Ok(Self { pool })
    }

    /// Open a single-connection in-memory database, primarily for tests.
    pub async fn connect_in_memory() -> CowResult<Self> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")
            .map_err(|err| CowError::Storage(err.to_string()))?
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|err| CowError::Storage(err.to_string()))?;

        Ok(Self { pool })
    }

    /// Apply all embedded migrations.
    ///
    /// A database whose recorded migration checksums differ only by line
    /// endings (LF versus CRLF) is repaired after its schema is validated;
    /// see [`crate::migrate`].
    pub async fn migrate(&self) -> CowResult<()> {
        crate::migrate::run(&self.pool).await
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn close(self) {
        self.pool.close().await;
    }
}
