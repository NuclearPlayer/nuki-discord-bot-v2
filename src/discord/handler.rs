//! The Serenity event handler.

use anyhow::bail;
use serenity::client::{Context, EventHandler};
use serenity::model::application::{Command, Interaction};
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::model::id::{GuildId, UserId};
use tokio::sync::OnceCell;

use crate::config::Config;
use crate::discord::commands::{self, CHANGE_MODEL_COMMAND, change_model_command};
use crate::discord::history::is_system;
use crate::routing::trigger::{MessageFacts, trigger_of};
use crate::services::Services;

pub fn facts_of(message: &Message, nuki_user_id: u64, in_open_intake_thread: bool) -> MessageFacts {
    let nuki = UserId::new(nuki_user_id);
    MessageFacts {
        author_is_bot: message.author.bot,
        is_system: is_system(message),
        is_dm: message.guild_id.is_none(),
        in_open_intake_thread,
        mentions_nuki: message.mentions_user_id(nuki),
        replies_to_nuki: message
            .referenced_message
            .as_deref()
            .is_some_and(|referenced| referenced.author.id == nuki),
    }
}

/// The server of the bot: the one server in `guilds`.
pub fn sole_guild(guilds: impl IntoIterator<Item = GuildId>) -> anyhow::Result<GuildId> {
    let guilds: Vec<GuildId> = guilds.into_iter().collect();
    match guilds.as_slice() {
        [guild] => Ok(*guild),
        _ => bail!(
            "the bot is in {} servers, but it must be in exactly one",
            guilds.len()
        ),
    }
}

/// Starts the services at the first `ready`. Until then, it drops the messages and the
/// interactions.
pub struct Handler {
    config: Config,
    services: OnceCell<Services>,
}

impl Handler {
    pub fn new(config: Config) -> Handler {
        Handler {
            config,
            services: OnceCell::new(),
        }
    }

    async fn start(&self, ready: &Ready) -> anyhow::Result<Services> {
        let guild_id = sole_guild(ready.guilds.iter().map(|guild| guild.id))?;
        Services::start(self.config.clone(), guild_id).await
    }
}

/// Registers `/change-model` as a global command. The HTTP client of the gateway knows the
/// application ID, which the command routes need.
async fn register_commands(ctx: &Context) {
    let registered = Command::set_global_commands(&ctx.http, vec![change_model_command()]).await;
    if let Err(error) = registered {
        tracing::error!("could not register /{CHANGE_MODEL_COMMAND}: {error}");
    }
}

#[serenity::async_trait]
impl EventHandler for Handler {
    /// `ready` fires again after a reconnect. The services start only once.
    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!(name = ready.user.name, "connected to Discord");
        register_commands(&ctx).await;
        let started = self.services.get_or_try_init(|| self.start(&ready)).await;
        if let Err(error) = started {
            tracing::error!("could not start the services: {error:#}");
            std::process::exit(1);
        }
    }

    /// Debounces each message that has a trigger. The pipeline looks up the intake of the
    /// channel, so this filter does not need it: every message that it drops has no trigger.
    async fn message(&self, _ctx: Context, message: Message) {
        let Some(services) = self.services.get() else {
            return;
        };
        let facts = facts_of(&message, services.app.nuki.user_id, false);
        if trigger_of(&facts).is_none() {
            return;
        }
        let key = (message.channel_id.get(), message.author.id.get());
        services.debouncer.push(key, message);
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Some(services) = self.services.get() else {
            return;
        };
        let app = &services.app;
        let answered = match &interaction {
            Interaction::Command(command) if command.data.name == CHANGE_MODEL_COMMAND => {
                commands::execute(&ctx.http, command, &app.models, &app.active_model).await
            }
            Interaction::Autocomplete(command) if command.data.name == CHANGE_MODEL_COMMAND => {
                commands::autocomplete(&ctx.http, command, &app.models).await
            }
            _ => Ok(()),
        };
        if let Err(error) = answered {
            tracing::error!("could not answer the interaction: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NUKI_USER_ID: u64 = 1700000000000000001;

    fn message(fixture: &str) -> Message {
        serde_json::from_str(fixture).unwrap()
    }

    #[test]
    fn facts_of_a_regular_message() {
        let regular = message(include_str!(
            "../../tests/fixtures/discord/handler_regular_message.json"
        ));

        assert_eq!(
            facts_of(&regular, NUKI_USER_ID, false),
            MessageFacts {
                author_is_bot: false,
                is_system: false,
                is_dm: false,
                in_open_intake_thread: false,
                mentions_nuki: false,
                replies_to_nuki: false,
            }
        );
    }

    #[test]
    fn facts_of_a_reply_to_nuki() {
        let reply = message(include_str!(
            "../../tests/fixtures/discord/handler_reply_to_nuki.json"
        ));

        assert_eq!(
            facts_of(&reply, NUKI_USER_ID, true),
            MessageFacts {
                author_is_bot: false,
                is_system: false,
                is_dm: false,
                in_open_intake_thread: true,
                mentions_nuki: false,
                replies_to_nuki: true,
            }
        );
    }

    #[test]
    fn facts_of_a_message_with_a_mention_of_nuki() {
        let mention = message(include_str!(
            "../../tests/fixtures/discord/handler_mention_of_nuki.json"
        ));

        assert_eq!(
            facts_of(&mention, NUKI_USER_ID, false),
            MessageFacts {
                author_is_bot: false,
                is_system: false,
                is_dm: false,
                in_open_intake_thread: false,
                mentions_nuki: true,
                replies_to_nuki: false,
            }
        );
    }

    #[test]
    fn facts_of_a_join_message_are_system() {
        let join = message(include_str!(
            "../../tests/fixtures/discord/handler_join_message.json"
        ));

        assert_eq!(
            facts_of(&join, NUKI_USER_ID, false),
            MessageFacts {
                author_is_bot: false,
                is_system: true,
                is_dm: false,
                in_open_intake_thread: false,
                mentions_nuki: false,
                replies_to_nuki: false,
            }
        );
    }

    #[test]
    fn facts_of_a_message_from_a_bot() {
        let bot = message(include_str!(
            "../../tests/fixtures/discord/handler_bot_message.json"
        ));

        assert_eq!(
            facts_of(&bot, NUKI_USER_ID, false),
            MessageFacts {
                author_is_bot: true,
                is_system: false,
                is_dm: false,
                in_open_intake_thread: false,
                mentions_nuki: false,
                replies_to_nuki: false,
            }
        );
    }

    #[test]
    fn sole_guild_is_the_one_server() {
        let guild = GuildId::new(1300000000000000001);

        assert_eq!(sole_guild([guild]).unwrap(), guild);
    }

    #[test]
    fn sole_guild_fails_without_a_server() {
        let error = sole_guild([]).unwrap_err();

        assert_eq!(
            error.to_string(),
            "the bot is in 0 servers, but it must be in exactly one"
        );
    }

    #[test]
    fn sole_guild_fails_with_two_servers() {
        let guilds = [
            GuildId::new(1300000000000000001),
            GuildId::new(1300000000000000002),
        ];

        let error = sole_guild(guilds).unwrap_err();

        assert_eq!(
            error.to_string(),
            "the bot is in 2 servers, but it must be in exactly one"
        );
    }

    #[test]
    fn facts_of_a_dm() {
        let dm = message(include_str!("../../tests/fixtures/discord/handler_dm.json"));

        assert_eq!(
            facts_of(&dm, NUKI_USER_ID, false),
            MessageFacts {
                author_is_bot: false,
                is_system: false,
                is_dm: true,
                in_open_intake_thread: false,
                mentions_nuki: false,
                replies_to_nuki: false,
            }
        );
    }
}
