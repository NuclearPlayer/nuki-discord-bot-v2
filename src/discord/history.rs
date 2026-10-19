//! Gets the conversation data of a turn from the Discord HTTP API.

use std::collections::HashMap;

use anyhow::Context;
use chrono::Utc;
use futures::future::try_join_all;
use serenity::builder::GetMessages;
use serenity::model::channel::{GuildChannel, Message, MessageType};
use serenity::model::guild::Role;
use serenity::model::id::{ChannelId, MessageId, RoleId, UserId};

use crate::app::App;
use crate::discord::attachments::{ResolvedMessage, resolve_message};
use crate::discord::chat_message::ChatMessage;
use crate::discord::pipeline::first_message;
use crate::prompts::system::{CustomEmoji, RuntimeContext};
use crate::routing::trigger::Trigger;
use crate::store::intakes::Intake;
use crate::workflows::TurnContext;

/// Discord gives at most 100 messages in one request.
const PAGE_SIZE: u8 = 100;

/// The display name and the role names of an author.
struct Author {
    name: String,
    roles: Vec<String>,
}

/// A message that a person or a bot wrote. All other kinds are system messages.
pub fn is_system(message: &Message) -> bool {
    !matches!(
        message.kind,
        MessageType::Regular | MessageType::InlineReply
    )
}

pub async fn fetch_channel(app: &App, channel_id: ChannelId) -> anyhow::Result<GuildChannel> {
    channel_id
        .to_channel(&app.http)
        .await?
        .guild()
        .with_context(|| format!("the channel {channel_id} is not a channel of a server"))
}

/// The `CONTEXT_MESSAGES` last messages of the channel: the history before the unit, then the unit.
pub async fn fetch_unit_context(
    app: &App,
    unit: &[Message],
) -> anyhow::Result<Vec<ResolvedMessage>> {
    let first = first_message(unit);
    let earlier_count = app.config.context_messages.saturating_sub(unit.len());
    let earlier = messages_before(app, first.channel_id, first.id, earlier_count).await?;
    let messages = earlier.into_iter().chain(unit.iter().cloned()).collect();
    resolve(app, messages).await
}

/// The messages of the intake, oldest first, from the first message of the unit that started it.
///
/// In an existing thread or forum post, the intake started in the thread, so the history is the
/// thread messages from that message on. A new thread is made from the last message of the unit,
/// and has the ID of that starter message. Then the history is the earlier messages of the unit
/// in the parent channel, the starter message, and all messages of the thread.
pub async fn fetch_thread_history(
    app: &App,
    intake: &Intake,
) -> anyhow::Result<Vec<ResolvedMessage>> {
    let origin_id = MessageId::new(intake.origin_message_id);
    let thread_messages = messages_since(app, ChannelId::new(intake.thread_id), origin_id).await?;
    let messages = if intake.origin_channel_id == intake.thread_id {
        thread_messages
    } else {
        with_parent_messages(app, intake, thread_messages).await?
    };
    let conversation = messages
        .into_iter()
        .filter(|message| !is_system(message))
        .collect();
    resolve(app, conversation).await
}

/// The earlier messages of the unit in the parent channel, the starter message, then the thread.
async fn with_parent_messages(
    app: &App,
    intake: &Intake,
    thread_messages: Vec<Message>,
) -> anyhow::Result<Vec<Message>> {
    let before_starter = origin_messages(app, intake).await?;
    let starter = starter_message(app, intake).await?;
    Ok(before_starter
        .into_iter()
        .chain(starter)
        .chain(thread_messages)
        .collect())
}

/// The `count` messages before `message_id`, oldest first, without system messages.
async fn messages_before(
    app: &App,
    channel_id: ChannelId,
    message_id: MessageId,
    count: usize,
) -> anyhow::Result<Vec<Message>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let limit = u8::try_from(count.min(usize::from(PAGE_SIZE))).expect("at most 100");
    let newest_first = channel_id
        .messages(
            &app.http,
            GetMessages::new().before(message_id).limit(limit),
        )
        .await?;
    Ok(newest_first
        .into_iter()
        .rev()
        .filter(|message| !is_system(message))
        .collect())
}

/// The messages of a channel from `since` on, oldest first, page by page from the newest.
async fn messages_since(
    app: &App,
    channel_id: ChannelId,
    since: MessageId,
) -> anyhow::Result<Vec<Message>> {
    let mut newest_first: Vec<Message> = Vec::new();
    loop {
        let page_request = match newest_first.last() {
            Some(oldest) => GetMessages::new().before(oldest.id).limit(PAGE_SIZE),
            None => GetMessages::new().limit(PAGE_SIZE),
        };
        let page = channel_id.messages(&app.http, page_request).await?;
        let is_last_page = page.len() < usize::from(PAGE_SIZE)
            || page.last().is_some_and(|oldest| oldest.id <= since);
        newest_first.extend(page.into_iter().take_while(|message| message.id >= since));
        if is_last_page {
            break;
        }
    }
    newest_first.reverse();
    Ok(newest_first)
}

/// The message that a new thread was made from. A deleted starter message is left out.
async fn starter_message(app: &App, intake: &Intake) -> anyhow::Result<Option<Message>> {
    let fetched = ChannelId::new(intake.origin_channel_id)
        .message(&app.http, MessageId::new(intake.thread_id))
        .await;
    match fetched {
        Ok(starter) => Ok(Some(starter)),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The messages of the parent channel from the first message of the unit that started the
/// intake to the message before the starter message. The thread is made from the last message
/// of that unit, so the earlier messages of the unit stay in the parent channel.
async fn origin_messages(app: &App, intake: &Intake) -> anyhow::Result<Vec<Message>> {
    let origin_id = MessageId::new(intake.origin_message_id);
    let starter_id = MessageId::new(intake.thread_id);
    if origin_id >= starter_id {
        return Ok(Vec::new());
    }
    // A unit is a few messages, so one page is enough. A unit of more than 100 messages loses its oldest messages.
    let newest_first = ChannelId::new(intake.origin_channel_id)
        .messages(
            &app.http,
            GetMessages::new().before(starter_id).limit(PAGE_SIZE),
        )
        .await?;
    let mut in_range: Vec<Message> = newest_first
        .into_iter()
        .take_while(|message| message.id >= origin_id)
        .collect();
    in_range.reverse();
    Ok(in_range)
}

/// Maps the messages to the form of the context builder, with the display name and the roles of
/// each author, and the attachments in the input modalities of the active model.
async fn resolve(app: &App, messages: Vec<Message>) -> anyhow::Result<Vec<ResolvedMessage>> {
    let authors = authors_of(app, &messages).await?;
    let chat_messages: Vec<ChatMessage> = messages
        .iter()
        .map(|message| {
            let author = &authors[&message.author.id];
            ChatMessage::from_serenity(
                message,
                author.name.clone(),
                author.roles.clone(),
                app.nuki.user_id,
            )
        })
        .collect();
    let input_modalities = app.active_model.current().input_modalities;
    let resolved = chat_messages
        .iter()
        .map(|chat_message| resolve_message(chat_message, &input_modalities, &app.rest));
    Ok(try_join_all(resolved).await?)
}

/// Gets each author of the messages one time.
async fn authors_of(app: &App, messages: &[Message]) -> anyhow::Result<HashMap<UserId, Author>> {
    let roles = app.guild_id.roles(&app.http).await?;
    let one_message_per_author: HashMap<UserId, &Message> = messages
        .iter()
        .map(|message| (message.author.id, message))
        .collect();
    let authors = one_message_per_author
        .into_iter()
        .map(|(user_id, message)| {
            let roles = &roles;
            async move { anyhow::Ok((user_id, author(app, message, roles).await?)) }
        });
    Ok(try_join_all(authors).await?.into_iter().collect())
}

/// The nickname, else the global name, else the username, and the names of the roles.
/// An author who is not a member, for example a user who left or a webhook, has no roles.
async fn author(
    app: &App,
    message: &Message,
    roles: &HashMap<RoleId, Role>,
) -> anyhow::Result<Author> {
    match app.guild_id.member(&app.http, message.author.id).await {
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

fn is_not_found(error: &serenity::Error) -> bool {
    match error {
        serenity::Error::Http(http_error) => http_error
            .status_code()
            .is_some_and(|status| status.as_u16() == 404),
        _ => false,
    }
}

async fn runtime_context(app: &App, channel_name: &str) -> anyhow::Result<RuntimeContext> {
    let emoji = app
        .guild_id
        .emojis(&app.http)
        .await?
        .into_iter()
        .map(|emoji| CustomEmoji {
            name: emoji.name,
            id: emoji.id.get(),
            animated: emoji.animated,
        })
        .collect();
    Ok(RuntimeContext {
        bot_name: app.nuki.name.clone(),
        bot_user_id: app.nuki.user_id,
        emoji,
        channel_name: channel_name.to_string(),
        now: Utc::now(),
    })
}

/// The turn context, with the stored tool turns of the messages of the history.
pub async fn turn_context(
    app: &App,
    trigger: Trigger,
    channel_name: &str,
    history: Vec<ResolvedMessage>,
    unit_len: usize,
) -> anyhow::Result<TurnContext> {
    let stored_turns = stored_turns(app, &history).await?;
    Ok(TurnContext {
        trigger,
        channel_name: channel_name.to_string(),
        history,
        unit_len,
        stored_turns,
        runtime: runtime_context(app, channel_name).await?,
    })
}

/// The store keeps each turn with the channel of its message. The history of an intake has
/// messages of two channels: the parent channel and the thread.
async fn stored_turns(
    app: &App,
    history: &[ResolvedMessage],
) -> anyhow::Result<HashMap<u64, Vec<serde_json::Value>>> {
    let mut message_ids_by_channel: HashMap<u64, Vec<u64>> = HashMap::new();
    for resolved in history {
        message_ids_by_channel
            .entry(resolved.message.channel_id)
            .or_default()
            .push(resolved.message.id);
    }
    let tool_turns = app.store.tool_turns();
    let loads = message_ids_by_channel
        .iter()
        .map(|(channel_id, message_ids)| tool_turns.load(*channel_id, message_ids));
    let by_channel = try_join_all(loads).await?;
    Ok(by_channel.into_iter().flatten().collect())
}
