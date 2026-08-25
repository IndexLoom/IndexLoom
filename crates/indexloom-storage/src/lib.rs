//! SQLite-first persistence with explicit backend boundaries.

use std::{str::FromStr, time::Duration};

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use thiserror::Error;

/// SQLite storage used by the initial `IndexLoom` release line.
#[derive(Clone, Debug)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Opens the database, enables WAL for file databases and applies migrations.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the URL is invalid, SQLite cannot open the
    /// database, or a migration cannot be applied.
    pub async fn connect(database_url: &str) -> Result<Self, StorageError> {
        let is_memory = database_url.contains(":memory:");
        let mut options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        if !is_memory {
            options = options.journal_mode(SqliteJournalMode::Wal);
        }

        let pool = SqlitePoolOptions::new()
            .max_connections(if is_memory { 1 } else { 5 })
            .connect_with(options)
            .await?;

        sqlx::migrate!("../../migrations/sqlite").run(&pool).await?;
        Ok(Self { pool })
    }

    /// Confirms that the storage connection is usable.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the probe query cannot complete.
    pub async fn ping(&self) -> Result<(), StorageError> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }
}

/// Storage initialization and query failures.
#[derive(Debug, Error)]
pub enum StorageError {
    /// SQLite configuration was invalid.
    #[error("invalid SQLite configuration: {0}")]
    Configuration(#[from] sqlx::Error),
    /// A database migration failed.
    #[error("database migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
}
