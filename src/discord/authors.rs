//! Gets the display name and the role names of each message author.

use std::collections::HashMap;

use futures::future::try_join_all;
use serenity::model::channel::Message;
use serenity::model::guild::Role;
use serenity::model::id::{GuildId, RoleId, UserId};

use crate::app::App;
use crate::discord::http_error::is_not_found;

/// The display name and the role names of an author.
pub(super) struct Author {
    pub(super) name: String,
    pub(super) roles: Vec<String>,
}

/// Gets each author of the messages one time, as a member of the server `guild_id`.
pub(super) async fn authors_of(
    app: &App,
    guild_id: GuildId,
    messages: &[Message],
) -> anyhow::Result<HashMap<UserId, Author>> {
    let roles = guild_id.roles(&app.http).await?;
    let one_message_per_author: HashMap<UserId, &Message> = messages
        .iter()
        .map(|message| (message.author.id, message))
        .collect();
    let authors = one_message_per_author
        .into_iter()
        .map(|(user_id, message)| {
            let roles = &roles;
            async move { anyhow::Ok((user_id, author(app, guild_id, message, roles).await?)) }
        });
    Ok(try_join_all(authors).await?.into_iter().collect())
}

/// Gets the name and the role names of the author. The name is the nickname. If the author has
/// no nickname, it is the global name. If the author has no global name, it is the username.
/// If the author is not a member, for example a user who left or a webhook, the author has no
/// roles.
async fn author(
    app: &App,
    guild_id: GuildId,
    message: &Message,
    roles: &HashMap<RoleId, Role>,
) -> anyhow::Result<Author> {
    match guild_id.member(&app.http, message.author.id).await {
        Ok(member) => Ok(Author {
            name: member.display_name().to_string(),
            roles: member
                .roles
                .iter()
                .filter_map(|role_id| roles.get(role_id))
                .map(|role| role.name.clone())
                .collect(),
        }),
        Err(error) if is_not_found(&error) => Ok(Author {
            name: message.author.display_name().to_string(),
            roles: Vec::new(),
        }),
        Err(error) => Err(error.into()),
    }
}
