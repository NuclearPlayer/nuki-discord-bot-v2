use rusqlite::OptionalExtension;
use tokio_rusqlite::Connection;

use super::{Result, call};

const MODEL_ID_KEY: &str = "model_id";

#[derive(Clone)]
pub struct SettingsStore {
    conn: Connection,
}

impl SettingsStore {
    pub(super) fn new(conn: Connection) -> Self {
        Self { conn }
    }

    /// The ID of the model that an administrator selected with `/change-model`.
    pub async fn model_id(&self) -> Result<Option<String>> {
        call(&self.conn, |c| {
            Ok(c.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                [MODEL_ID_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?)
        })
        .await
    }

    pub async fn set_model_id(&self, model_id: &str) -> Result<()> {
        let model_id = model_id.to_string();
        call(&self.conn, move |c| {
            c.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                (MODEL_ID_KEY, model_id),
            )?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use crate::store::Store;

    #[tokio::test]
    async fn model_id_is_none_before_a_model_is_stored() {
        let settings = Store::open_in_memory().await.unwrap().settings();

        assert_eq!(settings.model_id().await.unwrap(), None);
    }

    #[tokio::test]
    async fn set_model_id_stores_the_model_id() {
        let settings = Store::open_in_memory().await.unwrap().settings();

        settings
            .set_model_id("anthropic/claude-sonnet-4")
            .await
            .unwrap();

        assert_eq!(
            settings.model_id().await.unwrap(),
            Some("anthropic/claude-sonnet-4".to_string())
        );
    }

    #[tokio::test]
    async fn set_model_id_replaces_the_previous_model_id() {
        let settings = Store::open_in_memory().await.unwrap().settings();

        settings
            .set_model_id("anthropic/claude-sonnet-4")
            .await
            .unwrap();
        settings.set_model_id("openai/gpt-5").await.unwrap();

        assert_eq!(
            settings.model_id().await.unwrap(),
            Some("openai/gpt-5".to_string())
        );
    }
}
