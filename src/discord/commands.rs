use serenity::all::{CommandInteraction, CommandOptionType, Permissions, ResolvedValue};
use serenity::builder::{
    CreateAutocompleteResponse, CreateCommand, CreateCommandOption, CreateInteractionResponse,
    CreateInteractionResponseMessage,
};
use serenity::http::Http;
use thiserror::Error;

use crate::llm::models_list::{ModelChoice, ModelsClient, ModelsError, choices};
use crate::llm::selection::ActiveModel;
use crate::store::StoreError;
use crate::store::settings::ModelSelection;

pub const CHANGE_MODEL_COMMAND: &str = "change-model";
const MODEL_OPTION: &str = "model";

/// A Discord response failed. Boxed, because `serenity::Error` is large.
#[derive(Debug, Error)]
#[error("Discord request failed: {0}")]
pub struct CommandError(Box<serenity::Error>);

impl From<serenity::Error> for CommandError {
    fn from(error: serenity::Error) -> Self {
        Self(Box::new(error))
    }
}

/// Why `/change-model` could not change the model. The user sees it in the reply.
#[derive(Debug, Error)]
enum ChangeError {
    #[error(transparent)]
    Models(#[from] ModelsError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The `/change-model` command, which only administrators see by default.
pub fn change_model_command() -> CreateCommand {
    let model = CreateCommandOption::new(CommandOptionType::String, MODEL_OPTION, "Model to use")
        .required(true)
        .set_autocomplete(true);
    CreateCommand::new(CHANGE_MODEL_COMMAND)
        .description("Change the LLM model of Nuki")
        .add_option(model)
        .default_member_permissions(Permissions::ADMINISTRATOR)
}

/// Answers the autocomplete of the `model` option with the OpenRouter models that match the text.
pub async fn autocomplete(
    http: &Http,
    interaction: &CommandInteraction,
    models: &ModelsClient,
) -> Result<(), CommandError> {
    let query = interaction
        .data
        .autocomplete()
        .map(|focused| focused.value)
        .expect("Discord sends an autocomplete interaction with a focused option");
    let response = model_choices(models, query)
        .await
        .into_iter()
        .fold(CreateAutocompleteResponse::new(), |response, choice| {
            response.add_string_choice(choice.name, choice.value)
        });
    interaction
        .create_response(http, CreateInteractionResponse::Autocomplete(response))
        .await?;
    Ok(())
}

/// Gives no choices when OpenRouter fails, because the autocomplete has no way to show an error.
async fn model_choices(models: &ModelsClient, query: &str) -> Vec<ModelChoice> {
    match models.list().await {
        Ok(listed) => choices(&listed, query),
        Err(error) => {
            tracing::error!("could not list the models for the autocomplete: {error}");
            Vec::new()
        }
    }
}

/// Selects the model of the `model` option and tells the user the result.
/// Returns `Err` only when the reply fails.
pub async fn execute(
    http: &Http,
    interaction: &CommandInteraction,
    models: &ModelsClient,
    active: &ActiveModel,
) -> Result<(), CommandError> {
    let model_id = model_option(interaction).expect("Discord always sends the required option");
    let user = interaction.user.tag();
    let text = match change_model(models, active, model_id, &user).await {
        Ok(text) => text,
        Err(error) => {
            tracing::error!(user, model_id, "could not change the model: {error}");
            format!("Could not change the model: {error}")
        }
    };
    reply(http, interaction, text).await
}

/// Selects `model_id` if OpenRouter lists it, and gives the reply text.
async fn change_model(
    models: &ModelsClient,
    active: &ActiveModel,
    model_id: &str,
    user: &str,
) -> Result<String, ChangeError> {
    let listed = models.list().await?;
    let Some(model) = listed.into_iter().find(|model| model.id == model_id) else {
        return Ok(format!("Unknown model: {model_id}"));
    };

    let previous = active.current().model_id;
    active
        .set(ModelSelection {
            model_id: model.id,
            input_modalities: model.input_modalities,
        })
        .await?;
    tracing::info!(user, previous, new = model_id, "the model changed");

    Ok(changed_message(&previous, model_id))
}

fn model_option(interaction: &CommandInteraction) -> Option<&str> {
    interaction
        .data
        .options()
        .into_iter()
        .find_map(|option| match (option.name, option.value) {
            (MODEL_OPTION, ResolvedValue::String(value)) => Some(value),
            _ => None,
        })
}

async fn reply(
    http: &Http,
    interaction: &CommandInteraction,
    text: String,
) -> Result<(), CommandError> {
    let message = CreateInteractionResponseMessage::new()
        .content(text)
        .ephemeral(true);
    interaction
        .create_response(http, CreateInteractionResponse::Message(message))
        .await?;
    Ok(())
}

fn changed_message(previous: &str, new: &str) -> String {
    format!("Model changed: **{previous}** → **{new}**")
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use serenity::http::HttpBuilder;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::store::Store;
    use crate::store::test_support::selection;
    use crate::test_support::json_response;

    const INTERACTION: &str =
        include_str!("../../tests/fixtures/discord/command_change_model.json");
    const MODELS: &str = include_str!("../../tests/fixtures/llm/models.json");
    const CALLBACK_PATH: &str =
        "/api/v10/interactions/1800000000000000001/interaction-token/callback";

    /// The two mock servers and the state that `execute` needs.
    struct Fixture {
        discord: MockServer,
        http: Http,
        models: ModelsClient,
        active: ActiveModel,
    }

    impl Fixture {
        /// OpenRouter lists `google/gemini-3.8-flash` and `anthropic/claude-sonnet-4`.
        /// The active model is `google/gemini-3.8-flash`.
        async fn start() -> Fixture {
            let openrouter = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/api/v1/models"))
                .respond_with(json_response(200, MODELS))
                .mount(&openrouter)
                .await;
            let discord = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(CALLBACK_PATH))
                .respond_with(ResponseTemplate::new(204))
                .mount(&discord)
                .await;
            let http = HttpBuilder::new("test-token")
                .proxy(discord.uri())
                .ratelimiter_disabled(true)
                .build();
            let settings = Store::open_in_memory().await.unwrap().settings();
            let active = ActiveModel::load(settings, "google/gemini-3.8-flash")
                .await
                .unwrap();
            Fixture {
                discord,
                http,
                models: ModelsClient::new(openrouter.uri()),
                active,
            }
        }

        async fn execute(&self, model_id: &str) {
            let interaction: CommandInteraction =
                serde_json::from_str(&INTERACTION.replace("MODEL_ID", model_id)).unwrap();
            execute(&self.http, &interaction, &self.models, &self.active)
                .await
                .unwrap();
        }

        async fn callback_body(&self) -> Value {
            let requests = self.discord.received_requests().await.unwrap();
            let [callback] = requests.as_slice() else {
                panic!("expected one callback, got {}", requests.len());
            };
            serde_json::from_slice(&callback.body).unwrap()
        }
    }

    #[tokio::test]
    async fn execute_changes_the_model_and_replies_with_both_names() {
        let fixture = Fixture::start().await;

        fixture.execute("anthropic/claude-sonnet-4").await;

        assert_eq!(
            fixture.callback_body().await,
            json!({
                "type": 4,
                "data": {
                    "content": "Model changed: **google/gemini-3.8-flash** → **anthropic/claude-sonnet-4**",
                    "flags": 64,
                    "attachments": []
                }
            })
        );
        assert_eq!(
            fixture.active.current(),
            selection("anthropic/claude-sonnet-4", &["image", "text"])
        );
    }

    #[tokio::test]
    async fn execute_rejects_an_unknown_model() {
        let fixture = Fixture::start().await;

        fixture.execute("vendor/unknown").await;

        assert_eq!(
            fixture.callback_body().await,
            json!({
                "type": 4,
                "data": {
                    "content": "Unknown model: vendor/unknown",
                    "flags": 64,
                    "attachments": []
                }
            })
        );
        assert_eq!(
            fixture.active.current(),
            selection("google/gemini-3.8-flash", &["text", "image"])
        );
    }

    #[test]
    fn change_model_command_has_the_option_and_the_admin_permission() {
        assert_eq!(
            serde_json::to_value(change_model_command()).unwrap(),
            json!({
                "name": "change-model",
                "name_localizations": {},
                "description": "Change the LLM model of Nuki",
                "description_localizations": {},
                "options": [{
                    "type": 3,
                    "name": "model",
                    "description": "Model to use",
                    "required": true,
                    "choices": [],
                    "options": [],
                    "channel_types": [],
                    "min_value": null,
                    "max_value": null,
                    "min_length": null,
                    "max_length": null,
                    "autocomplete": true
                }],
                "default_member_permissions": "8",
                "nsfw": false
            })
        );
    }

    #[test]
    fn changed_message_names_both_models() {
        assert_eq!(
            changed_message("google/gemini-3.8-flash", "anthropic/claude-sonnet-4"),
            "Model changed: **google/gemini-3.8-flash** → **anthropic/claude-sonnet-4**"
        );
    }
}
