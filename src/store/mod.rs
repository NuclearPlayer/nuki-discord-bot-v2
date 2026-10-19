pub mod intakes;
pub mod settings;
pub mod tool_turns;

use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::Row;
use rusqlite::types::Type;
use rusqlite_migration::{M, Migrations};
use thiserror::Error;
use tokio_rusqlite::Connection;

use intakes::IntakeStore;
use settings::SettingsStore;
use tool_turns::ToolTurnStore;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("migration error: {0}")]
    Migration(#[from] rusqlite_migration::Error),
    #[error("the database connection is closed")]
    ConnectionClosed,
    #[error("invalid JSON in the store: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Clone)]
pub struct Store {
    conn: Connection,
}

impl Store {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::migrated(Connection::open(path).await?).await
    }

    pub async fn open_in_memory() -> Result<Self> {
        Self::migrated(Connection::open_in_memory().await?).await
    }

    async fn migrated(conn: Connection) -> Result<Self> {
        call(&conn, |c| {
            Migrations::new(vec![M::up(include_str!("../../migrations/001_init.sql"))])
                .to_latest(c)
                .map_err(StoreError::from)
        })
        .await?;
        Ok(Self { conn })
    }

    pub fn settings(&self) -> SettingsStore {
        SettingsStore::new(self.conn.clone())
    }

    pub fn intakes(&self) -> IntakeStore {
        IntakeStore::new(self.conn.clone())
    }

    pub fn tool_turns(&self) -> ToolTurnStore {
        ToolTurnStore::new(self.conn.clone())
    }
}

/// Runs `f` on the database thread and flattens the connection error into `StoreError`.
async fn call<R, F>(conn: &Connection, f: F) -> Result<R>
where
    R: Send + 'static,
    F: FnOnce(&mut rusqlite::Connection) -> Result<R> + Send + 'static,
{
    conn.call(f).await.map_err(|error| match error {
        tokio_rusqlite::Error::Error(store_error) => store_error,
        tokio_rusqlite::Error::Close((_, close_error)) => StoreError::Database(close_error),
        // The enum is non-exhaustive; ConnectionClosed is the only other variant in 0.8.
        _ => StoreError::ConnectionClosed,
    })
}

/// The fixed-width RFC 3339 form, so that SQL text comparison is time comparison.
fn timestamp_text(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

fn timestamp_column(row: &Row, index: usize) -> rusqlite::Result<DateTime<Utc>> {
    let text: String = row.get(index)?;
    DateTime::parse_from_rfc3339(&text)
        .map(|at| at.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::settings::ModelSelection;

    #[tokio::test]
    async fn open_keeps_the_data_in_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nuki.db");
        let selection = ModelSelection {
            model_id: "anthropic/claude-sonnet-4".to_string(),
            input_modalities: vec!["text".to_string(), "image".to_string()],
        };
        let store = Store::open(&path).await.unwrap();
        store
            .settings()
            .set_model_selection(&selection)
            .await
            .unwrap();
        drop(store);

        let reopened = Store::open(&path).await.unwrap();

        assert_eq!(
            reopened.settings().model_selection().await.unwrap(),
            Some(selection)
        );
    }
}
