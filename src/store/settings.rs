use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use tokio_rusqlite::Connection;

use super::{Result, StoreError, call};

const MODEL_SELECTION_KEY: &str = "model_selection";

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ModelSelection {
    pub model_id: String,
    pub input_modalities: Vec<String>,
}

#[derive(Clone)]
pub struct SettingsStore {
    conn: Connection,
}

impl SettingsStore {
    pub(super) fn new(conn: Connection) -> Self {
        Self { conn }
    }

    pub async fn model_selection(&self) -> Result<Option<ModelSelection>> {
        call(&self.conn, |c| {
            c.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                [MODEL_SELECTION_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(StoreError::from)
        })
        .await
    }

    pub async fn set_model_selection(&self, selection: &ModelSelection) -> Result<()> {
        let json = serde_json::to_string(selection)?;
        call(&self.conn, move |c| {
            c.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                (MODEL_SELECTION_KEY, json),
            )?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn selection(model_id: &str, input_modalities: &[&str]) -> ModelSelection {
        ModelSelection {
            model_id: model_id.to_string(),
            input_modalities: input_modalities.iter().map(|m| m.to_string()).collect(),
        }
    }

    #[tokio::test]
    async fn model_selection_is_none_before_a_selection_is_stored() {
        let settings = Store::open_in_memory().await.unwrap().settings();

        assert_eq!(settings.model_selection().await.unwrap(), None);
    }

    #[tokio::test]
    async fn set_model_selection_stores_the_selection() {
        let settings = Store::open_in_memory().await.unwrap().settings();
        let stored = selection("anthropic/claude-sonnet-4", &["text", "image"]);

        settings.set_model_selection(&stored).await.unwrap();

        assert_eq!(settings.model_selection().await.unwrap(), Some(stored));
    }

    #[tokio::test]
    async fn set_model_selection_replaces_the_previous_selection() {
        let settings = Store::open_in_memory().await.unwrap().settings();
        let replacement = selection("openai/gpt-5", &["text"]);

        settings
            .set_model_selection(&selection("anthropic/claude-sonnet-4", &["text", "image"]))
            .await
            .unwrap();
        settings.set_model_selection(&replacement).await.unwrap();

        assert_eq!(settings.model_selection().await.unwrap(), Some(replacement));
    }
}
