use std::sync::RwLock;

use crate::store::Result;
use crate::store::settings::SettingsStore;

/// The model before an administrator selects one with `/change-model`.
pub const DEFAULT_MODEL_ID: &str = "google/gemini-3.8-flash";

/// The ID of the model that the LLM agent uses now. Administrators change it with
/// `/change-model`.
pub struct ActiveModel {
    model_id: RwLock<String>,
    settings: SettingsStore,
}

impl ActiveModel {
    /// Uses the stored model ID, or `default_model_id` when none is stored.
    pub async fn load(settings: SettingsStore, default_model_id: &str) -> Result<ActiveModel> {
        let model_id = settings
            .model_id()
            .await?
            .unwrap_or_else(|| default_model_id.to_string());
        Ok(ActiveModel {
            model_id: RwLock::new(model_id),
            settings,
        })
    }

    pub fn current(&self) -> String {
        self.model_id
            .read()
            .expect("no writer panics while it holds the lock")
            .clone()
    }

    /// Stores the model ID first, so that memory never holds a model ID the store lost.
    pub async fn set(&self, model_id: &str) -> Result<()> {
        self.settings.set_model_id(model_id).await?;
        *self
            .model_id
            .write()
            .expect("no writer panics while it holds the lock") = model_id.to_string();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    async fn settings() -> SettingsStore {
        Store::open_in_memory().await.unwrap().settings()
    }

    #[tokio::test]
    async fn load_uses_the_stored_model_id() {
        let settings = settings().await;
        settings.set_model_id("openai/gpt-5").await.unwrap();

        let active = ActiveModel::load(settings, "google/gemini-3.8-flash")
            .await
            .unwrap();

        assert_eq!(active.current(), "openai/gpt-5");
    }

    #[tokio::test]
    async fn load_uses_the_default_model_when_nothing_is_stored() {
        let active = ActiveModel::load(settings().await, "google/gemini-3.8-flash")
            .await
            .unwrap();

        assert_eq!(active.current(), "google/gemini-3.8-flash");
    }

    #[tokio::test]
    async fn set_stores_and_replaces_the_model_id() {
        let settings = settings().await;
        let active = ActiveModel::load(settings.clone(), "google/gemini-3.8-flash")
            .await
            .unwrap();

        active.set("anthropic/claude-sonnet-4").await.unwrap();

        assert_eq!(active.current(), "anthropic/claude-sonnet-4");
        assert_eq!(
            settings.model_id().await.unwrap(),
            Some("anthropic/claude-sonnet-4".to_string())
        );
    }
}
