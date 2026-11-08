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

pub(crate) fn is_thread(channel: &GuildChannel) -> bool {
    matches!(
        channel.kind,
        ChannelType::PublicThread | ChannelType::PrivateThread | ChannelType::NewsThread
    )
}

/// Gets the last `CONTEXT_MESSAGES` messages of the channel: the history before the unit, then
/// the unit.
///
/// If someone made the thread from a message in a channel, the history has these messages:
///
/// - the parent channel's messages before the starter message
/// - the starter message, with the thread header
/// - the thread's messages, with the unit at the end
///
/// The thread's messages have priority. The parent channel's messages fill the remaining places.
/// If the thread has no starter message, the history has only the thread's messages, and the
/// first message has the header. A forum post has no parent channel messages.
pub async fn fetch_unit_context(
    app: &App,
    channel: &GuildChannel,
    unit: &[Message],
) -> anyhow::Result<Vec<ResolvedMessage>> {
    let first = first_message(unit);
    let context_messages = app.config.context_messages;
    let earlier_count = context_messages.saturating_sub(unit.len());
    let earlier = messages_before(app, first.channel_id, first.id, earlier_count).await?;
    let messages: Vec<Message> = earlier.into_iter().chain(unit.iter().cloned()).collect();
    if is_thread(channel) {
        // The starter message takes one place.
        let parent_count = context_messages.saturating_sub(messages.len() + 1);
        in_thread(app, channel, messages, parent_count).await
    } else {
        resolve(app, channel.guild_id, messages).await
    }
}

/// Returns the history of `messages` in `thread`, without system messages. Adds the thread's
/// starter message in its place, with the marker that opens the thread. Before the starter
/// message, adds at most `parent_count` messages of the parent channel.
async fn in_thread(
    app: &App,
    thread: &GuildChannel,
    messages: Vec<Message>,
    parent_count: usize,
) -> anyhow::Result<Vec<ResolvedMessage>> {
    let parent_id = thread
        .parent_id
        .with_context(|| format!("the thread {} has no parent channel", thread.id))?;
    let parent = fetch_channel(app, parent_id).await?;
    let starter = starter_message(&app.http, thread, &parent).await?;
    let before_starter =
        parent_messages_before(app, starter.as_ref(), &parent, parent_count).await?;
    let around_starter: Vec<Message> = before_starter.into_iter().chain(messages).collect();
    let conversation = with_starter(starter, around_starter)
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

/// Gets the `count` messages of `parent` before `starter`, oldest first. If the thread has no
/// starter message, returns no messages. If the starter message is in the thread, for example in
/// a forum post, returns no messages.
async fn parent_messages_before(
    app: &App,
    starter: Option<&Message>,
    parent: &GuildChannel,
    count: usize,
) -> anyhow::Result<Vec<Message>> {
    match starter {
        Some(starter) if starter.channel_id == parent.id => {
            messages_before(app, parent.id, starter.id, count).await
        }
        _ => Ok(Vec::new()),
    }
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
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;
    use crate::discord::test_support::{GUILD_ROLES, resolved, test_app, user_message};
    use crate::test_support::{discord_http, json_response};

    const THREAD_STARTER: &str =
        include_str!("../../tests/fixtures/discord/history_thread_starter.json");
    const CONTEXT_MESSAGE_WITH_ATTACHMENT: &str =
        include_str!("../../tests/fixtures/discord/context_message_with_attachment.json");
    const UNKNOWN_MESSAGE: &str =
        include_str!("../../tests/fixtures/discord/history_unknown_message.json");
    const UNKNOWN_MEMBER: &str = include_str!("../../tests/fixtures/discord/unknown_member.json");
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
        mount_get(&discord, message_path, status, body).await;
        discord
    }

    const GUILD_ID: u64 = 1000000000000000001;
    const TEXT_CHANNEL_ID: u64 = 1200000000000000001;
    const THREAD_ID: u64 = 1100000000000000003;
    const FORUM_CHANNEL_ID: u64 = 1200000000000000005;
    const FORUM_POST_ID: u64 = 1100000000000000004;

    fn message_json(id: u64, channel_id: u64) -> Value {
        json!({
            "id": id.to_string(),
            "channel_id": channel_id.to_string(),
            "author": { "id": "1300000000000000002", "username": "bob", "global_name": "Bob" },
            "content": format!("message {id}"),
            "timestamp": "2026-10-19T12:40:00.000000+00:00",
            "tts": false,
            "mention_everyone": false,
            "mentions": [],
            "mention_roles": [],
            "attachments": [],
            "embeds": [],
            "pinned": false,
            "type": 0
        })
    }

    /// Returns a Discord server with the roles of the server. Its members are unknown, so each
    /// author keeps the name on the message.
    async fn discord_server() -> MockServer {
        let discord = MockServer::start().await;
        mount_get(
            &discord,
            &format!("/api/v10/guilds/{GUILD_ID}/roles"),
            200,
            GUILD_ROLES,
        )
        .await;
        Mock::given(method("GET"))
            .and(path_regex(format!(
                r"^/api/v10/guilds/{GUILD_ID}/members/\d+$"
            )))
            .respond_with(json_response(404, UNKNOWN_MEMBER))
            .mount(&discord)
            .await;
        discord
    }

    async fn mount_get(discord: &MockServer, url_path: &str, status: u16, body: &str) {
        Mock::given(method("GET"))
            .and(path(url_path))
            .respond_with(json_response(status, body))
            .mount(discord)
            .await;
    }

    /// Serves the messages of the channel as Discord does: newest first, before the message
    /// `before`, and `limit` messages at most.
    async fn mount_channel_messages(discord: &MockServer, channel_id: u64, message_ids: &[u64]) {
        let messages: Vec<Value> = message_ids
            .iter()
            .map(|id| message_json(*id, channel_id))
            .collect();
        Mock::given(method("GET"))
            .and(path(format!("/api/v10/channels/{channel_id}/messages")))
            .respond_with(move |request: &Request| {
                let before = query_number(request, "before");
                let limit = query_number(request, "limit");
                let page: Vec<&Value> = messages
                    .iter()
                    .rev()
                    .filter(|message| {
                        message["id"].as_str().unwrap().parse::<u64>().unwrap() < before
                    })
                    .take(usize::try_from(limit).unwrap())
                    .collect();
                ResponseTemplate::new(200).set_body_json(page)
            })
            .mount(discord)
            .await;
    }

    fn query_number(request: &Request, name: &str) -> u64 {
        request
            .url
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.parse().unwrap())
            .unwrap()
    }

    /// Returns the IDs of the history that `fetch_unit_context` gets for a unit of one message,
    /// `unit_id`, in `thread`.
    async fn unit_history_ids(
        discord: &MockServer,
        thread: &str,
        context_messages: usize,
        unit_id: u64,
    ) -> Vec<u64> {
        let repos = TempDir::new().unwrap();
        let mut app = test_app(discord, repos.path()).await;
        app.config.context_messages = context_messages;
        let thread = channel(thread);
        let unit: Message = serde_json::from_value(message_json(unit_id, thread.id.get())).unwrap();

        let history = fetch_unit_context(&app, &thread, &[unit]).await.unwrap();

        history.iter().map(|resolved| resolved.message.id).collect()
    }

    async fn mount_text_channel_thread(discord: &MockServer, starter_status: u16, starter: &str) {
        mount_get(
            discord,
            &format!("/api/v10/channels/{TEXT_CHANNEL_ID}"),
            200,
            TEXT_CHANNEL,
        )
        .await;
        mount_get(
            discord,
            &format!("/api/v10/channels/{TEXT_CHANNEL_ID}/messages/{THREAD_ID}"),
            starter_status,
            starter,
        )
        .await;
    }

    #[tokio::test]
    async fn a_thread_with_few_messages_gets_the_parent_messages_before_the_starter() {
        let discord = discord_server().await;
        mount_text_channel_thread(&discord, 200, THREAD_STARTER).await;
        mount_channel_messages(
            &discord,
            TEXT_CHANNEL_ID,
            &[
                1050000000000000001,
                1050000000000000002,
                1050000000000000003,
                1050000000000000004,
            ],
        )
        .await;
        mount_channel_messages(
            &discord,
            THREAD_ID,
            &[1100000000000000007, 1100000000000000008],
        )
        .await;

        let history = unit_history_ids(&discord, THREAD, 6, 1100000000000000009).await;

        assert_eq!(
            history,
            vec![
                1050000000000000003,
                1050000000000000004,
                THREAD_ID,
                1100000000000000007,
                1100000000000000008,
                1100000000000000009,
            ]
        );
    }

    #[tokio::test]
    async fn a_thread_with_context_messages_or_more_gets_no_parent_messages() {
        let discord = discord_server().await;
        mount_text_channel_thread(&discord, 200, THREAD_STARTER).await;
        mount_channel_messages(
            &discord,
            TEXT_CHANNEL_ID,
            &[1050000000000000001, 1050000000000000002],
        )
        .await;
        mount_channel_messages(
            &discord,
            THREAD_ID,
            &[
                1100000000000000006,
                1100000000000000007,
                1100000000000000008,
            ],
        )
        .await;

        let history = unit_history_ids(&discord, THREAD, 3, 1100000000000000009).await;

        assert_eq!(
            history,
            vec![
                THREAD_ID,
                1100000000000000007,
                1100000000000000008,
                1100000000000000009,
            ]
        );
    }

    #[tokio::test]
    async fn a_thread_without_a_starter_gets_no_parent_messages() {
        let discord = discord_server().await;
        mount_text_channel_thread(&discord, 404, UNKNOWN_MESSAGE).await;
        mount_channel_messages(
            &discord,
            TEXT_CHANNEL_ID,
            &[1050000000000000001, 1050000000000000002],
        )
        .await;
        mount_channel_messages(&discord, THREAD_ID, &[1100000000000000007]).await;

        let history = unit_history_ids(&discord, THREAD, 6, 1100000000000000009).await;

        assert_eq!(history, vec![1100000000000000007, 1100000000000000009]);
    }

    #[tokio::test]
    async fn a_forum_post_gets_no_parent_messages() {
        let discord = discord_server().await;
        mount_get(
            &discord,
            &format!("/api/v10/channels/{FORUM_CHANNEL_ID}"),
            200,
            FORUM_CHANNEL,
        )
        .await;
        mount_get(
            &discord,
            &format!("/api/v10/channels/{FORUM_POST_ID}/messages/{FORUM_POST_ID}"),
            200,
            FORUM_POST_STARTER,
        )
        .await;
        mount_channel_messages(
            &discord,
            FORUM_CHANNEL_ID,
            &[1050000000000000001, 1050000000000000002],
        )
        .await;
        mount_channel_messages(
            &discord,
            FORUM_POST_ID,
            &[FORUM_POST_ID, 1100000000000000007],
        )
        .await;

        let history = unit_history_ids(&discord, FORUM_POST, 6, 1100000000000000009).await;

        assert_eq!(
            history,
            vec![FORUM_POST_ID, 1100000000000000007, 1100000000000000009]
        );
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
    fn the_starter_goes_after_the_messages_of_the_parent_channel() {
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
    fn the_starter_opens_the_thread_after_the_messages_of_the_parent_channel() {
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
