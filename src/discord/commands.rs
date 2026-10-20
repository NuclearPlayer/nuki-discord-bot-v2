use serenity::all::{
    CommandInteraction, CommandOptionType, InteractionContext, Permissions, ResolvedValue,
};
use serenity::builder::{
    CreateAutocompleteResponse, CreateCommand, CreateCommandOption, CreateInteractionResponse,
    CreateInteractionResponseMessage,
};
use serenity::http::Http;
use serenity::model::id::GuildId;
use thiserror::Error;

use crate::llm::models_list::{ModelChoice, ModelsClient, ModelsError, choices};
use crate::llm::selection::ActiveModel;
use crate::stats::event::{Event, ModelChanged};
use crate::stats::recorder::{EventScope, Stats};
use crate::store::StoreError;

pub const CHANGE_MODEL_COMMAND: &str = "change-model";
const MODEL_OPTION: &str = "model";

/// The error when a Discord response fails. It boxes `serenity::Error`, because that type is
/// large.
#[derive(Debug, Error)]
#[error("Discord request failed: {0}")]
pub struct CommandError(Box<serenity::Error>);

impl From<serenity::Error> for CommandError {
    fn from(error: serenity::Error) -> Self {
        Self(Box::new(error))
    }
}

/// The error when `/change-model` cannot change the model. The user sees this error in the
/// reply.
#[derive(Debug, Error)]
enum ChangeError {
    #[error(transparent)]
    Models(#[from] ModelsError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// By default, only administrators see the `/change-model` command.
pub fn change_model_command() -> CreateCommand {
    let model = CreateCommandOption::new(CommandOptionType::String, MODEL_OPTION, "Model to use")
        .required(true)
        .set_autocomplete(true);
    CreateCommand::new(CHANGE_MODEL_COMMAND)
        .description("Change the LLM model of Nuki")
        .add_option(model)
        .default_member_permissions(Permissions::ADMINISTRATOR)
        // Discord also shows a global command in the DMs with the bot. In the DMs, Discord does
        // not apply `default_member_permissions`.
        .contexts(vec![InteractionContext::Guild])
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

/// If OpenRouter fails, gives no choices, because the autocomplete cannot show an error.
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
    stats: &Stats,
) -> Result<(), CommandError> {
    let model_id = model_option(interaction).expect("Discord always sends the required option");
    let user = interaction.user.tag();
    let text = match change_model(models, active, model_id).await {
        Ok(Some(previous)) => {
            tracing::info!(user, previous, new = model_id, "the model changed");
            let changed = ModelChanged {
                previous: previous.clone(),
                new: model_id.to_string(),
            };
            stats
                .record(interaction_scope(interaction), Event::ModelChanged(changed))
                .await;
            changed_message(&previous, model_id)
        }
        Ok(None) => format!("Unknown model: {model_id}"),
        Err(error) => {
            tracing::error!(user, model_id, "could not change the model: {error}");
            format!("Could not change the model: {error}")
        }
    };
    reply(http, interaction, text).await
}

/// Returns the scope of an interaction event: the server, the channel, and the user of the
/// interaction. An interaction is not part of a run.
fn interaction_scope(interaction: &CommandInteraction) -> EventScope {
    EventScope {
        run_id: None,
        guild_id: interaction.guild_id.map(GuildId::get),
        channel_id: Some(interaction.channel_id.get()),
        user_id: Some(interaction.user.id.get()),
    }
}

/// If OpenRouter lists `model_id`, selects that model and returns the previous model. If
/// OpenRouter does not list it, returns `None` and does not change the model.
async fn change_model(
    models: &ModelsClient,
    active: &ActiveModel,
    model_id: &str,
) -> Result<Option<String>, ChangeError> {
    let listed = models.list().await?;
    if !listed.iter().any(|model| model.id == model_id) {
        return Ok(None);
    }

    let previous = active.current();
    active.set(model_id).await?;
    Ok(Some(previous))
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
    use chrono::Utc;
    use serde_json::{Value, json};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::llm::test_support::mount_models;
    use crate::stats::test_support::{RunEvent, events_between};
    use crate::store::Store;
    use crate::test_support::discord_http;

    const INTERACTION: &str =
        include_str!("../../tests/fixtures/discord/command_change_model.json");
    const CALLBACK_PATH: &str =
        "/api/v10/interactions/1800000000000000001/interaction-token/callback";

    struct Fixture {
        _openrouter: MockServer,
        discord: MockServer,
        http: Http,
        models: ModelsClient,
        active: ActiveModel,
        store: Store,
        stats: Stats,
    }

    impl Fixture {
        async fn start() -> Fixture {
            let openrouter = MockServer::start().await;
            mount_models(&openrouter).await;
            let discord = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(CALLBACK_PATH))
                .respond_with(ResponseTemplate::new(204))
                .mount(&discord)
                .await;
            let http = discord_http(&discord);
            let store = Store::open_in_memory().await.unwrap();
            let models = ModelsClient::new(openrouter.uri());
            let active = ActiveModel::load(store.settings(), "google/gemini-3.8-flash")
                .await
                .unwrap();
            Fixture {
                models,
                _openrouter: openrouter,
                discord,
                http,
                active,
                stats: Stats::new(store.events()),
                store,
            }
        }

        async fn execute(&self, model_id: &str) {
            let interaction: CommandInteraction =
                serde_json::from_str(&INTERACTION.replace("MODEL_ID", model_id)).unwrap();
            execute(
                &self.http,
                &interaction,
                &self.models,
                &self.active,
                &self.stats,
            )
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
        assert_eq!(fixture.active.current(), "anthropic/claude-sonnet-4");
    }

    #[tokio::test]
    async fn execute_records_the_model_change() {
        let fixture = Fixture::start().await;
        let before = Utc::now();

        fixture.execute("anthropic/claude-sonnet-4").await;

        let after = Utc::now();
        assert_eq!(
            events_between(&fixture.store, before, after).await,
            vec![RunEvent {
                kind: "model.changed".to_string(),
                run_id: None,
                guild_id: Some(1000000000000000001),
                channel_id: Some(1200000000000000001),
                user_id: Some(1300000000000000001),
                data: json!({
                    "previous": "google/gemini-3.8-flash",
                    "new": "anthropic/claude-sonnet-4",
                }),
            }]
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
        assert_eq!(fixture.active.current(), "google/gemini-3.8-flash");
    }

    #[test]
    fn change_model_command_has_the_option_the_admin_permission_and_only_the_guild_context() {
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
                "contexts": [0],
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
