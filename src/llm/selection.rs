use std::sync::RwLock;

use thiserror::Error;

use crate::llm::models_list::{ModelsClient, ModelsError};
use crate::store::settings::{ModelSelection, SettingsStore};
use crate::store::{Result, StoreError};

/// The model before an administrator selects one with `/change-model`.
pub const DEFAULT_MODEL_ID: &str = "google/gemini-3.8-flash";

#[derive(Debug, Error)]
pub enum LoadError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Models(#[from] ModelsError),
    #[error("OpenRouter does not list the default model {0}")]
    DefaultNotListed(String),
}

/// The model that the LLM agent uses now. Administrators change it with `/change-model`.
pub struct ActiveModel {
    selection: RwLock<ModelSelection>,
    settings: SettingsStore,
}

impl ActiveModel {
    /// Uses the stored selection. When none is stored, uses `default_model_id` with the input
    /// modalities that OpenRouter lists for it. The default is not stored, so each start asks
    /// OpenRouter again until an administrator selects a model.
    pub async fn load(
        settings: SettingsStore,
        models: &ModelsClient,
        default_model_id: &str,
    ) -> std::result::Result<ActiveModel, LoadError> {
        let selection = match settings.model_selection().await? {
            Some(stored) => stored,
            None => listed_selection(models, default_model_id)
                .await?
                .ok_or_else(|| LoadError::DefaultNotListed(default_model_id.to_string()))?,
        };
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

/// The selection of `model_id` with the input modalities that OpenRouter lists for it, or
/// `None` when OpenRouter does not list the model.
pub async fn listed_selection(
    models: &ModelsClient,
    model_id: &str,
) -> std::result::Result<Option<ModelSelection>, ModelsError> {
    let listed = models.list().await?;
    Ok(listed
        .into_iter()
        .find(|model| model.id == model_id)
        .map(|model| ModelSelection {
            model_id: model.id,
            input_modalities: model.input_modalities,
        }))
}

#[cfg(test)]
mod tests {
    use wiremock::MockServer;

    use super::*;
    use crate::llm::test_support::mount_models;
    use crate::store::Store;
    use crate::store::test_support::selection;

    async fn settings() -> SettingsStore {
        Store::open_in_memory().await.unwrap().settings()
    }

    async fn stored_settings(stored: &ModelSelection) -> SettingsStore {
        let settings = settings().await;
        settings.set_model_selection(stored).await.unwrap();
        settings
    }

    #[tokio::test]
    async fn load_uses_the_stored_selection() {
        let stored = selection("openai/gpt-5", &["text"]);
        let server = MockServer::start().await;

        let active = ActiveModel::load(
            stored_settings(&stored).await,
            &ModelsClient::new(server.uri()),
            "google/gemini-3.8-flash",
        )
        .await
        .unwrap();

        assert_eq!(active.current(), stored);
    }

    #[tokio::test]
    async fn load_uses_the_default_model_with_its_listed_modalities_when_nothing_is_stored() {
        let server = MockServer::start().await;
        mount_models(&server).await;

        let active = ActiveModel::load(
            settings().await,
            &ModelsClient::new(server.uri()),
            "google/gemini-3.8-flash",
        )
        .await
        .unwrap();

        assert_eq!(
            active.current(),
            selection(
                "google/gemini-3.8-flash",
                &["text", "image", "file", "audio"]
            )
        );
    }

    #[tokio::test]
    async fn load_fails_when_openrouter_does_not_list_the_default_model() {
        let server = MockServer::start().await;
        mount_models(&server).await;

        let error = ActiveModel::load(
            settings().await,
            &ModelsClient::new(server.uri()),
            "unknown/model",
        )
        .await
        .err()
        .unwrap();

        assert_eq!(
            error.to_string(),
            "OpenRouter does not list the default model unknown/model"
        );
    }

    #[tokio::test]
    async fn set_stores_and_replaces_the_selection() {
        let settings = stored_settings(&selection("openai/gpt-5", &["text"])).await;
        let server = MockServer::start().await;
        let active = ActiveModel::load(
            settings.clone(),
            &ModelsClient::new(server.uri()),
            "google/gemini-3.8-flash",
        )
        .await
        .unwrap();
        let replacement = selection("anthropic/claude-sonnet-4", &["text", "image", "file"]);

        active.set(replacement.clone()).await.unwrap();

        assert_eq!(active.current(), replacement);
        assert_eq!(settings.model_selection().await.unwrap(), Some(replacement));
    }
}
