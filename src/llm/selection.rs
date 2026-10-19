use std::sync::RwLock;

use crate::store::Result;
use crate::store::settings::{ModelSelection, SettingsStore};

/// The model that the LLM agent uses now. Administrators change it with `/change-model`.
pub struct ActiveModel {
    selection: RwLock<ModelSelection>,
    settings: SettingsStore,
}

impl ActiveModel {
    /// Uses the stored selection, or `default_model_id` with text and image input when none is stored.
    pub async fn load(settings: SettingsStore, default_model_id: &str) -> Result<ActiveModel> {
        let selection = settings
            .model_selection()
            .await?
            .unwrap_or_else(|| default_selection(default_model_id));
        Ok(ActiveModel {
            selection: RwLock::new(selection),
            settings,
        })
    }

    pub fn current(&self) -> ModelSelection {
        self.selection
            .read()
            .expect("no writer panics while it holds the lock")
            .clone()
    }

    /// Stores the selection first, so that memory never holds a selection the store lost.
    pub async fn set(&self, selection: ModelSelection) -> Result<()> {
        self.settings.set_model_selection(&selection).await?;
        *self
            .selection
            .write()
            .expect("no writer panics while it holds the lock") = selection;
        Ok(())
    }
}

fn default_selection(model_id: &str) -> ModelSelection {
    ModelSelection {
        model_id: model_id.to_string(),
        input_modalities: vec!["text".to_string(), "image".to_string()],
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

    async fn settings() -> SettingsStore {
        Store::open_in_memory().await.unwrap().settings()
    }

    #[tokio::test]
    async fn load_uses_the_stored_selection() {
        let settings = settings().await;
        let stored = selection("openai/gpt-5", &["text"]);
        settings.set_model_selection(&stored).await.unwrap();

        let active = ActiveModel::load(settings, "google/gemini-3.8-flash")
            .await
            .unwrap();

        assert_eq!(active.current(), stored);
    }

    #[tokio::test]
    async fn load_uses_the_default_model_when_nothing_is_stored() {
        let active = ActiveModel::load(settings().await, "google/gemini-3.8-flash")
            .await
            .unwrap();

        assert_eq!(
            active.current(),
            selection("google/gemini-3.8-flash", &["text", "image"])
        );
    }

    #[tokio::test]
    async fn set_stores_and_replaces_the_selection() {
        let settings = settings().await;
        let active = ActiveModel::load(settings.clone(), "google/gemini-3.8-flash")
            .await
            .unwrap();
        let replacement = selection("anthropic/claude-sonnet-4", &["text", "image", "file"]);

        active.set(replacement.clone()).await.unwrap();

        assert_eq!(active.current(), replacement);
        assert_eq!(settings.model_selection().await.unwrap(), Some(replacement));
    }
}
