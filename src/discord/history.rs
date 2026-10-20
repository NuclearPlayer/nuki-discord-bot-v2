//! Gets the messages of a turn from the Discord HTTP API.

use anyhow::Context;
use futures::future::try_join_all;
use serenity::builder::GetMessages;
use serenity::http::Http;
use serenity::model::channel::{ChannelType, GuildChannel, Message, MessageType};
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

/// System messages are all messages that a person or a bot did not write.
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

pub(super) fn is_thread(channel: &GuildChannel) -> bool {
    matches!(
        channel.kind,
        ChannelType::PublicThread | ChannelType::PrivateThread | ChannelType::NewsThread
    )
}

/// Gets the last `CONTEXT_MESSAGES` messages of the channel: the history before the unit, then
/// the unit. In a thread, the history starts with the thread's starter message, and that message
/// has the thread header. If the thread has no starter message, the first message has the header.
pub async fn fetch_unit_context(
    app: &App,
    channel: &GuildChannel,
    unit: &[Message],
) -> anyhow::Result<Vec<ResolvedMessage>> {
    let first = first_message(unit);
    let earlier_count = app.config.context_messages.saturating_sub(unit.len());
    let earlier = messages_before(app, first.channel_id, first.id, earlier_count).await?;
    let messages = earlier.into_iter().chain(unit.iter().cloned()).collect();
    if is_thread(channel) {
        in_thread(app, channel, messages).await
    } else {
        resolve(app, channel.guild_id, messages).await
    }
}

/// Gets the messages of the intake, oldest first. The messages start at the first message of the
/// unit that started the intake.
///
/// If the intake started in an existing thread or forum post, the history has these messages:
///
/// - the message that started the thread
/// - the thread messages from the first message of the unit
///
/// If not, the bot made a new thread from the last message of the unit. The new thread has the
/// ID of that starter message. Then the history has these messages:
///
/// - the earlier messages of the unit in the parent channel
/// - the starter message
/// - all messages of the thread
///
/// The starter message has the thread header. If there is no starter message, the first message
/// has the header.
pub async fn fetch_thread_history(
    app: &App,
    thread: &GuildChannel,
    intake: &Intake,
) -> anyhow::Result<Vec<ResolvedMessage>> {
    let origin_id = MessageId::new(intake.origin_message_id);
    let thread_messages = messages_since(app, thread.id, origin_id).await?;
    let messages = if intake.origin_channel_id == intake.thread_id {
        thread_messages
    } else {
        let before_starter = origin_messages(app, intake).await?;
        before_starter.into_iter().chain(thread_messages).collect()
    };
    in_thread(app, thread, messages).await
}

/// Returns the history of `messages` in `thread`, without system messages. Adds the thread's
/// starter message in its place, with the marker that opens the thread.
async fn in_thread(
    app: &App,
    thread: &GuildChannel,
    messages: Vec<Message>,
) -> anyhow::Result<Vec<ResolvedMessage>> {
    let parent_id = thread
        .parent_id
        .with_context(|| format!("the thread {} has no parent channel", thread.id))?;
    let parent = fetch_channel(app, parent_id).await?;
    let starter = starter_message(&app.http, thread, &parent).await?;
    let conversation = with_starter(starter, messages)
        .into_iter()
        .filter(|message| !is_system(message))
        .collect();
    let history = resolve(app, thread.guild_id, conversation).await?;
    Ok(with_opens_marker(history, thread, &parent))
}

/// Returns `messages` and `starter`, sorted by ID. If `messages` contain the starter, for example
/// the first message of a forum post, the result contains the starter one time.
fn with_starter(starter: Option<Message>, messages: Vec<Message>) -> Vec<Message> {
    let mut all: Vec<Message> = starter.into_iter().chain(messages).collect();
    all.sort_by_key(|message| message.id);
    all.dedup_by_key(|message| message.id);
    all
}

/// Puts the marker that opens `thread` on the starter message of `history`, in place of its
/// `Started` marker. If `history` does not contain the starter, puts the marker on the first
/// message.
fn with_opens_marker(
    history: Vec<ResolvedMessage>,
    thread: &GuildChannel,
    parent: &GuildChannel,
) -> Vec<ResolvedMessage> {
    let starter_index = history
        .iter()
        .position(|resolved| resolved.message.id == thread.id.get());
    let opens = ThreadMarker::Opens {
        name: thread.name.clone(),
        parent_channel_name: parent.name.clone(),
        reply_count: reply_count(thread),
        has_starter: starter_index.is_some(),
    };
    let opening_index = starter_index.unwrap_or(0);
    history
        .into_iter()
        .enumerate()
        .map(|(index, resolved)| {
            if index == opening_index {
                ResolvedMessage {
                    thread: Some(opens.clone()),
                    ..resolved
                }
            } else {
                resolved
            }
        })
        .collect()
}

/// Gets the `count` messages before `message_id`, oldest first, without system messages.
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

/// Gets the channel's messages from `since`, oldest first. Reads the channel page by page, from
/// the newest message.
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

/// Gets the message that started `thread`. The starter message has the thread's ID. If someone
/// made the thread from a message, that message is in the parent channel. If the thread is a
/// forum post, the message is in the post. If the starter message was deleted, or someone made
/// the thread without a message, returns `None`.
async fn starter_message(
    http: &Http,
    thread: &GuildChannel,
    parent: &GuildChannel,
) -> anyhow::Result<Option<Message>> {
    let channel_id = if parent.kind == ChannelType::Forum {
        thread.id
    } else {
        parent.id
    };
    let starter_id = MessageId::new(thread.id.get());
    match channel_id.message(http, starter_id).await {
        Ok(starter) => Ok(Some(starter)),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Gets the parent channel's messages from the first message of the unit that started the
/// intake, to the message before the starter message. The earlier messages of the unit stay in
/// the parent channel, because the bot makes the thread from the last message of that unit.
async fn origin_messages(app: &App, intake: &Intake) -> anyhow::Result<Vec<Message>> {
    let origin_id = MessageId::new(intake.origin_message_id);
    let starter_id = MessageId::new(intake.thread_id);
    if origin_id >= starter_id {
        return Ok(Vec::new());
    }
    // One page is sufficient, because a unit is short. If a unit has more than 100 messages,
    // the history does not contain the oldest messages of the unit.
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

/// Maps the messages to the form of the context builder. Adds the display name and the roles of
/// each author in the server `guild_id`. Resolves the attachments for the active model's input
/// modalities.
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

/// Returns the thread that `message` started. Reads the thread from the `thread` object that
/// Discord gives on a starter message.
fn started_marker(message: &Message) -> Option<ThreadMarker> {
    message.thread.as_ref().map(|thread| ThreadMarker::Started {
        name: thread.name.clone(),
        reply_count: reply_count(thread),
    })
}

/// Returns the thread's `message_count`. Discord does not count the starter message. If the
/// thread does not have the field, the thread has no replies.
fn reply_count(thread: &GuildChannel) -> u32 {
    thread.message_count.unwrap_or(0)
}

/// Gets the input modalities that OpenRouter lists at this time for the active model. If no
/// message has an attachment, returns no modalities and sends no request to OpenRouter.
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    use super::*;
    use crate::discord::test_support::{resolved, user_message};
    use crate::test_support::{discord_http, json_response};

    const THREAD_STARTER: &str =
        include_str!("../../tests/fixtures/discord/history_thread_starter.json");
    const CONTEXT_MESSAGE_WITH_ATTACHMENT: &str =
        include_str!("../../tests/fixtures/discord/context_message_with_attachment.json");
    const UNKNOWN_MESSAGE: &str =
        include_str!("../../tests/fixtures/discord/history_unknown_message.json");
    const THREAD: &str = include_str!("../../tests/fixtures/discord/history_thread.json");
    const TEXT_CHANNEL: &str =
        include_str!("../../tests/fixtures/discord/history_text_channel.json");
    const FORUM_POST: &str = include_str!("../../tests/fixtures/discord/history_forum_post.json");
    const FORUM_CHANNEL: &str =
        include_str!("../../tests/fixtures/discord/history_forum_channel.json");
    const FORUM_POST_STARTER: &str =
        include_str!("../../tests/fixtures/discord/history_forum_post_starter.json");

    fn channel(json: &str) -> GuildChannel {
        serde_json::from_str(json).unwrap()
    }

    fn message_with_id(id: u64) -> Message {
        let mut message = Message::default();
        message.id = MessageId::new(id);
        message
    }

    fn ids(messages: &[Message]) -> Vec<u64> {
        messages.iter().map(|message| message.id.get()).collect()
    }

    async fn discord_serving(message_path: &str, status: u16, body: &str) -> MockServer {
        let discord = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(message_path))
            .respond_with(json_response(status, body))
            .mount(&discord)
            .await;
        discord
    }

    #[tokio::test]
    async fn the_starter_of_a_thread_made_from_a_message_is_fetched_from_the_parent_channel() {
        let discord = discord_serving(
            "/api/v10/channels/1200000000000000001/messages/1100000000000000003",
            200,
            THREAD_STARTER,
        )
        .await;

        let starter = starter_message(
            &discord_http(&discord),
            &channel(THREAD),
            &channel(TEXT_CHANNEL),
        )
        .await
        .unwrap();

        assert_eq!(
            starter.map(|message| message.id),
            Some(MessageId::new(1100000000000000003))
        );
    }

    #[tokio::test]
    async fn the_starter_of_a_forum_post_is_fetched_from_the_post() {
        let discord = discord_serving(
            "/api/v10/channels/1100000000000000004/messages/1100000000000000004",
            200,
            FORUM_POST_STARTER,
        )
        .await;

        let starter = starter_message(
            &discord_http(&discord),
            &channel(FORUM_POST),
            &channel(FORUM_CHANNEL),
        )
        .await
        .unwrap();

        assert_eq!(
            starter.map(|message| message.id),
            Some(MessageId::new(1100000000000000004))
        );
    }

    #[tokio::test]
    async fn a_deleted_starter_is_left_out() {
        let discord = discord_serving(
            "/api/v10/channels/1200000000000000001/messages/1100000000000000003",
            404,
            UNKNOWN_MESSAGE,
        )
        .await;

        let starter = starter_message(
            &discord_http(&discord),
            &channel(THREAD),
            &channel(TEXT_CHANNEL),
        )
        .await
        .unwrap();

        assert_eq!(starter.map(|message| message.id), None);
    }

    #[test]
    fn the_starter_goes_before_the_messages_of_the_thread() {
        let messages = with_starter(
            Some(message_with_id(3)),
            vec![message_with_id(7), message_with_id(8)],
        );

        assert_eq!(ids(&messages), vec![3, 7, 8]);
    }

    #[test]
    fn the_starter_goes_after_the_earlier_messages_of_the_unit_in_the_parent_channel() {
        let messages = with_starter(
            Some(message_with_id(3)),
            vec![message_with_id(1), message_with_id(2), message_with_id(7)],
        );

        assert_eq!(ids(&messages), vec![1, 2, 3, 7]);
    }

    #[test]
    fn a_starter_that_is_in_the_messages_is_there_once() {
        let messages = with_starter(
            Some(message_with_id(4)),
            vec![message_with_id(4), message_with_id(5)],
        );

        assert_eq!(ids(&messages), vec![4, 5]);
    }

    #[test]
    fn the_starter_opens_the_thread_when_it_is_the_first_message() {
        let history = vec![
            resolved(user_message(1100000000000000003, "wrong streams"), vec![]),
            resolved(user_message(1100000000000000007, "any news?"), vec![]),
        ];

        let opened = with_opens_marker(history, &channel(THREAD), &channel(TEXT_CHANNEL));

        assert_eq!(
            opened,
            vec![
                ResolvedMessage {
                    thread: Some(ThreadMarker::Opens {
                        name: "nuclear is playing wrong streams, what should I do?".to_string(),
                        parent_channel_name: "general".to_string(),
                        reply_count: 5,
                        has_starter: true,
                    }),
                    ..resolved(user_message(1100000000000000003, "wrong streams"), vec![])
                },
                resolved(user_message(1100000000000000007, "any news?"), vec![]),
            ]
        );
    }

    #[test]
    fn the_starter_opens_the_thread_after_the_earlier_messages_of_the_unit() {
        let history = vec![
            resolved(user_message(1100000000000000001, "hey"), vec![]),
            ResolvedMessage {
                thread: Some(ThreadMarker::Started {
                    name: "nuclear is playing wrong streams, what should I do?".to_string(),
                    reply_count: 5,
                }),
                ..resolved(user_message(1100000000000000003, "wrong streams"), vec![])
            },
            resolved(user_message(1100000000000000007, "any news?"), vec![]),
        ];

        let opened = with_opens_marker(history, &channel(THREAD), &channel(TEXT_CHANNEL));

        assert_eq!(
            opened,
            vec![
                resolved(user_message(1100000000000000001, "hey"), vec![]),
                ResolvedMessage {
                    thread: Some(ThreadMarker::Opens {
                        name: "nuclear is playing wrong streams, what should I do?".to_string(),
                        parent_channel_name: "general".to_string(),
                        reply_count: 5,
                        has_starter: true,
                    }),
                    ..resolved(user_message(1100000000000000003, "wrong streams"), vec![])
                },
                resolved(user_message(1100000000000000007, "any news?"), vec![]),
            ]
        );
    }

    #[test]
    fn a_history_without_the_starter_opens_the_thread_on_the_first_message() {
        let history = vec![resolved(
            user_message(1100000000000000007, "any news?"),
            vec![],
        )];

        let opened = with_opens_marker(history, &channel(THREAD), &channel(TEXT_CHANNEL));

        assert_eq!(
            opened,
            vec![ResolvedMessage {
                thread: Some(ThreadMarker::Opens {
                    name: "nuclear is playing wrong streams, what should I do?".to_string(),
                    parent_channel_name: "general".to_string(),
                    reply_count: 5,
                    has_starter: false,
                }),
                ..resolved(user_message(1100000000000000007, "any news?"), vec![])
            }]
        );
    }

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
        let message: Message = serde_json::from_str(CONTEXT_MESSAGE_WITH_ATTACHMENT).unwrap();

        assert_eq!(started_marker(&message), None);
    }
}
