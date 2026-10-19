//! Gets the messages of a turn from the Discord HTTP API.

use anyhow::Context;
use futures::future::try_join_all;
use serenity::builder::GetMessages;
use serenity::model::channel::{GuildChannel, Message, MessageType};
use serenity::model::id::{ChannelId, GuildId, MessageId};

use crate::app::App;
use crate::discord::attachments::{ResolvedMessage, resolve_message};
use crate::discord::authors::authors_of;
use crate::discord::chat_message::ChatMessage;
use crate::discord::http_error::is_not_found;
use crate::discord::pipeline::first_message;
use crate::discord::thread_marker::ThreadMarker;
use crate::store::intakes::Intake;

/// Discord gives at most 100 messages in one request.
const PAGE_SIZE: u8 = 100;

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
    guild_id: GuildId,
    unit: &[Message],
) -> anyhow::Result<Vec<ResolvedMessage>> {
    let first = first_message(unit);
    let earlier_count = app.config.context_messages.saturating_sub(unit.len());
    let earlier = messages_before(app, first.channel_id, first.id, earlier_count).await?;
    let messages = earlier.into_iter().chain(unit.iter().cloned()).collect();
    resolve(app, guild_id, messages).await
}

/// The messages of the intake, oldest first, from the first message of the unit that started it.
///
/// In an existing thread or forum post, the intake started in the thread, so the history is the
/// thread messages from that message on. A new thread is made from the last message of the unit,
/// and has the ID of that starter message. Then the history is the earlier messages of the unit
/// in the parent channel, the starter message, and all messages of the thread.
pub async fn fetch_thread_history(
    app: &App,
    guild_id: GuildId,
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
    resolve(app, guild_id, conversation).await
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
/// each author in the server `guild_id`, and the attachments in the input modalities of the
/// active model.
async fn resolve(
    app: &App,
    guild_id: GuildId,
    messages: Vec<Message>,
) -> anyhow::Result<Vec<ResolvedMessage>> {
    let authors = authors_of(app, guild_id, &messages).await?;
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
    let input_modalities = input_modalities(app, &chat_messages).await?;
    let resolved = chat_messages
        .iter()
        .zip(&messages)
        .map(|(chat_message, message)| {
            resolve_message(
                chat_message,
                started_marker(message),
                &input_modalities,
                &app.rest,
            )
        });
    Ok(try_join_all(resolved).await?)
}

/// The thread that `message` started, from the `thread` object that Discord gives on a starter
/// message.
fn started_marker(message: &Message) -> Option<ThreadMarker> {
    message.thread.as_ref().map(|thread| ThreadMarker::Started {
        name: thread.name.clone(),
        reply_count: thread.message_count.unwrap_or(0),
    })
}

/// The input modalities that OpenRouter lists now for the active model. Messages without
/// attachments do not need them, so then the bot does not ask OpenRouter.
async fn input_modalities(app: &App, messages: &[ChatMessage]) -> anyhow::Result<Vec<String>> {
    if messages
        .iter()
        .all(|message| message.attachments.is_empty())
    {
        return Ok(Vec::new());
    }
    Ok(app
        .models
        .input_modalities(&app.active_model.current())
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREAD_STARTER: &str =
        include_str!("../../tests/fixtures/discord/history_thread_starter.json");
    const PLAIN_MESSAGE: &str =
        include_str!("../../tests/fixtures/discord/context_message_with_attachment.json");

    #[test]
    fn a_message_that_started_a_thread_has_the_name_and_the_reply_count_of_the_thread() {
        let message: Message = serde_json::from_str(THREAD_STARTER).unwrap();

        assert_eq!(
            started_marker(&message),
            Some(ThreadMarker::Started {
                name: "nuclear is playing wrong streams, what should I do?".to_string(),
                reply_count: 5,
            })
        );
    }

    #[test]
    fn a_message_without_a_thread_has_no_marker() {
        let message: Message = serde_json::from_str(PLAIN_MESSAGE).unwrap();

        assert_eq!(started_marker(&message), None);
    }
}
