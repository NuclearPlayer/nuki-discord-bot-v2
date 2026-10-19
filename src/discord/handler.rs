//! The Serenity event handler.

use std::sync::Arc;

use serenity::client::{Context, EventHandler};
use serenity::model::application::{Command, Interaction};
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::model::id::{RoleId, UserId};

use crate::app::App;
use crate::discord::commands::{self, CHANGE_MODEL_COMMAND, change_model_command};
use crate::discord::history::is_system;
use crate::routing::debounce::Debouncer;
use crate::routing::trigger::{MessageFacts, trigger_of};

/// `nuki_role_id` is the managed role of Nuki in the server of the message. A mention of that
/// role is a mention of Nuki.
pub fn facts_of(
    message: &Message,
    nuki_user_id: u64,
    nuki_role_id: Option<RoleId>,
    in_open_intake_thread: bool,
) -> MessageFacts {
    let nuki = UserId::new(nuki_user_id);
    let mentions_nuki_role =
        nuki_role_id.is_some_and(|role_id| message.mention_roles.contains(&role_id));
    MessageFacts {
        author_is_bot: message.author.bot,
        is_system: is_system(message),
        is_dm: message.guild_id.is_none(),
        in_open_intake_thread,
        mentions_nuki: message.mentions_user_id(nuki) || mentions_nuki_role,
        replies_to_nuki: message
            .referenced_message
            .as_deref()
            .is_some_and(|referenced| referenced.author.id == nuki),
    }
}

pub struct Handler {
    pub(crate) app: Arc<App>,
    pub(crate) debouncer: Debouncer<Message>,
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
    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!(name = ready.user.name, "connected to Discord");
        register_commands(&ctx).await;
    }

    /// Debounces each message that has a trigger. The pipeline looks up the intake of the
    /// channel and the role of Nuki, so this filter does not need them: every message that it
    /// drops has no trigger.
    async fn message(&self, _ctx: Context, message: Message) {
        let facts = facts_of(&message, self.app.nuki.user_id, None, false);
        if trigger_of(&facts).is_none() {
            return;
        }
        let key = (message.channel_id.get(), message.author.id.get());
        self.debouncer.push(key, message);
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let app = &self.app;
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
    const NUKI_ROLE_ID: RoleId = RoleId::new(1900000000000000001);
    const OTHER_ROLE_ID: RoleId = RoleId::new(1900000000000000002);

    fn message(fixture: &str) -> Message {
        serde_json::from_str(fixture).unwrap()
    }

    #[test]
    fn facts_of_a_regular_message() {
        let regular = message(include_str!(
            "../../tests/fixtures/discord/handler_regular_message.json"
        ));

        assert_eq!(
            facts_of(&regular, NUKI_USER_ID, None, false),
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
            facts_of(&reply, NUKI_USER_ID, None, true),
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
            facts_of(&mention, NUKI_USER_ID, Some(NUKI_ROLE_ID), false),
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
    fn facts_of_a_mention_of_the_role_of_nuki() {
        let mention = message(include_str!(
            "../../tests/fixtures/discord/handler_role_mention.json"
        ));

        assert_eq!(
            facts_of(&mention, NUKI_USER_ID, Some(NUKI_ROLE_ID), false),
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
    fn facts_of_a_mention_of_another_role() {
        let mention = message(include_str!(
            "../../tests/fixtures/discord/handler_role_mention.json"
        ));

        assert_eq!(
            facts_of(&mention, NUKI_USER_ID, Some(OTHER_ROLE_ID), false),
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
    fn facts_of_a_join_message_are_system() {
        let join = message(include_str!(
            "../../tests/fixtures/discord/handler_join_message.json"
        ));

        assert_eq!(
            facts_of(&join, NUKI_USER_ID, None, false),
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
            facts_of(&bot, NUKI_USER_ID, None, false),
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
    fn facts_of_a_dm() {
        let dm = message(include_str!("../../tests/fixtures/discord/handler_dm.json"));

        assert_eq!(
            facts_of(&dm, NUKI_USER_ID, None, false),
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
