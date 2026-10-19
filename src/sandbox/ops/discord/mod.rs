mod client;
mod views;

use std::sync::Arc;

pub use client::DiscordOps;
use client::{EmojiParams, ListParams, MessageParams, ReactionParams, SearchParams};

use crate::sandbox::operation::{Operation, OperationTable, Param};

const NAMESPACE: &str = "discord";

/// The TypeScript type of `MessageView`, as a literal so that `concat!` can use it.
macro_rules! message_view_type {
    () => {
        "{ id: string; channelId: string; author: { id: string; name: string }; content: string; timestamp: string; attachments: { id: string; filename: string; contentType: string | null; size: number }[]; replyTo: string | null }"
    };
}

/// The operations that read the server, add reactions, and add custom emoji.
pub fn discord_table(ops: DiscordOps) -> OperationTable {
    let ops = Arc::new(ops);
    OperationTable::new(vec![
        search_guild_messages(&ops),
        get_channel_message(&ops),
        list_channel_messages(&ops),
        create_reaction(&ops),
        create_guild_emoji(&ops),
    ])
}

fn search_guild_messages(ops: &Arc<DiscordOps>) -> Operation {
    Operation::with_handle(
        ops,
        NAMESPACE,
        "searchGuildMessages",
        "Search the messages in the server.",
        vec![
            Param {
                name: "content",
                ts_type: "string",
                optional: true,
                description: "Text that the messages contain.",
            },
            Param {
                name: "authorId",
                ts_type: "string",
                optional: true,
                description: "Find only the messages of this user.",
            },
            Param {
                name: "channelId",
                ts_type: "string",
                optional: true,
                description: "Find only the messages in this channel.",
            },
            Param {
                name: "limit",
                ts_type: "number",
                optional: true,
                description: "The maximum number of messages, at most 25. The default is 25.",
            },
            Param {
                name: "offset",
                ts_type: "number",
                optional: true,
                description: "The number of results to skip, to get the next page.",
            },
        ],
        concat!(
            "{ totalResults: number; messages: ",
            message_view_type!(),
            "[] }"
        ),
        |ops, params: SearchParams| async move { ops.search_guild_messages(params).await },
    )
}

fn get_channel_message(ops: &Arc<DiscordOps>) -> Operation {
    Operation::with_handle(
        ops,
        NAMESPACE,
        "getChannelMessage",
        "Get one message.",
        vec![channel_id_param(), message_id_param()],
        message_view_type!(),
        |ops, params: MessageParams| async move { ops.get_channel_message(params).await },
    )
}

fn list_channel_messages(ops: &Arc<DiscordOps>) -> Operation {
    Operation::with_handle(
        ops,
        NAMESPACE,
        "listChannelMessages",
        "Get a list of the messages in a channel, with a `before`, `after`, or `around` cursor.",
        vec![
            channel_id_param(),
            cursor_param("before", "Get the messages before this message ID."),
            cursor_param("after", "Get the messages after this message ID."),
            cursor_param("around", "Get the messages around this message ID."),
            Param {
                name: "limit",
                ts_type: "number",
                optional: true,
                description: "The maximum number of messages, at most 100. The default is 50.",
            },
        ],
        concat!(message_view_type!(), "[]"),
        |ops, params: ListParams| async move { ops.list_channel_messages(params).await },
    )
}

fn create_reaction(ops: &Arc<DiscordOps>) -> Operation {
    Operation::with_handle(
        ops,
        NAMESPACE,
        "createReaction",
        "Add a reaction to a message.",
        vec![
            channel_id_param(),
            message_id_param(),
            Param {
                name: "emoji",
                ts_type: "string",
                optional: false,
                description: "A Unicode emoji, or a custom emoji as `<:name:id>`.",
            },
        ],
        "{ ok: true }",
        |ops, params: ReactionParams| async move { ops.create_reaction(params).await },
    )
}

fn create_guild_emoji(ops: &Arc<DiscordOps>) -> Operation {
    Operation::with_handle(
        ops,
        NAMESPACE,
        "createGuildEmoji",
        "Add a custom emoji from an attachment.",
        vec![
            Param {
                name: "name",
                ts_type: "string",
                optional: false,
                description: "The name of the new emoji.",
            },
            channel_id_param(),
            message_id_param(),
            Param {
                name: "attachmentId",
                ts_type: "string",
                optional: false,
                description: "The ID of the image attachment of the message.",
            },
        ],
        "{ id: string; name: string; text: string }",
        |ops, params: EmojiParams| async move { ops.create_guild_emoji(params).await },
    )
}

fn channel_id_param() -> Param {
    Param {
        name: "channelId",
        ts_type: "string",
        optional: false,
        description: "The ID of the channel.",
    }
}

fn message_id_param() -> Param {
    Param {
        name: "messageId",
        ts_type: "string",
        optional: false,
        description: "The ID of the message.",
    }
}

fn cursor_param(name: &'static str, description: &'static str) -> Param {
    Param {
        name,
        ts_type: "string",
        optional: true,
        description,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use serenity::http::HttpBuilder;
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;
    use crate::sandbox::runtime::{Sandbox, SandboxLimits, ScriptOutcome};
    use crate::sandbox::test_support::value;
    use crate::test_support::json_response;

    const REPLY_WITH_GIF: &str =
        include_str!("../../../../tests/fixtures/discord/ops_reply_with_gif.json");
    const SEARCH_RESULTS: &str =
        include_str!("../../../../tests/fixtures/discord/ops_search_results.json");
    const CHANNEL_MESSAGES: &str =
        include_str!("../../../../tests/fixtures/discord/ops_channel_messages.json");
    const CREATED_EMOJI: &str =
        include_str!("../../../../tests/fixtures/discord/ops_created_emoji.json");

    const BOT_TOKEN: &str = "test-token";
    const GUILD_ID: u64 = 1000000000000000001;

    async fn run(server: &MockServer, code: &str) -> ScriptOutcome {
        let table = discord_table(ops_for(server));
        Sandbox::new(Arc::new(table), SandboxLimits::production())
            .run(code)
            .await
    }

    fn ops_for(server: &MockServer) -> DiscordOps {
        let http = HttpBuilder::new(BOT_TOKEN)
            .proxy(server.uri())
            .ratelimiter_disabled(true)
            .build();
        DiscordOps {
            http: Arc::new(http),
            rest: reqwest::Client::new(),
            api_base: format!("{}/api/v10", server.uri()),
            bot_token: BOT_TOKEN.to_owned(),
            guild_id: GUILD_ID,
        }
    }

    async fn mount_reply_with_gif(server: &MockServer) {
        let body = REPLY_WITH_GIF.replace("MOCK_SERVER", &server.uri());
        Mock::given(method("GET"))
            .and(path(
                "/api/v10/channels/1200000000000000001/messages/1100000000000000002",
            ))
            .respond_with(json_response(200, &body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn search_guild_messages_sends_the_filters_and_returns_the_messages() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v10/guilds/{GUILD_ID}/messages/search")))
            .and(query_param("content", "playlist"))
            .and(query_param("author_id", "1300000000000000001"))
            .and(query_param("channel_id", "1200000000000000001"))
            .and(query_param("limit", "10"))
            .and(query_param("offset", "20"))
            .and(header("Authorization", "Bot test-token"))
            .respond_with(json_response(200, SEARCH_RESULTS))
            .mount(&server)
            .await;

        let outcome = run(
            &server,
            "return await discord.searchGuildMessages({ content: 'playlist', authorId: '1300000000000000001', channelId: '1200000000000000001', limit: 10, offset: 20 });",
        )
        .await;

        assert_eq!(
            outcome,
            value(json!({
                "totalResults": 37,
                "messages": [
                    {
                        "id": "1100000000000000012",
                        "channelId": "1200000000000000001",
                        "author": { "id": "1300000000000000001", "name": "alice" },
                        "content": "the playlist import works again in 1.51",
                        "timestamp": "2026-10-18T09:00:05Z",
                        "attachments": [],
                        "replyTo": null,
                    },
                    {
                        "id": "1100000000000000011",
                        "channelId": "1200000000000000001",
                        "author": { "id": "1300000000000000001", "name": "alice" },
                        "content": "my playlist does not load, log attached",
                        "timestamp": "2026-10-17T21:14:30Z",
                        "attachments": [
                            {
                                "id": "1400000000000000011",
                                "filename": "nuclear.log",
                                "contentType": "text/plain; charset=utf-8",
                                "size": 2048,
                            },
                        ],
                        "replyTo": null,
                    },
                ],
            }))
        );
    }

    #[tokio::test]
    async fn search_guild_messages_sends_only_the_given_filters() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v10/guilds/{GUILD_ID}/messages/search")))
            .and(|request: &Request| request.url.query() == Some("content=playlist&limit=25"))
            .respond_with(json_response(200, SEARCH_RESULTS))
            .expect(1)
            .mount(&server)
            .await;

        let outcome = run(
            &server,
            "const result = await discord.searchGuildMessages({ content: 'playlist' }); return result.totalResults;",
        )
        .await;

        assert_eq!(outcome, value(json!(37)));
    }

    #[tokio::test]
    async fn discord_errors_carry_the_message_of_discord() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v10/guilds/{GUILD_ID}/messages/search")))
            .respond_with(json_response(
                403,
                r#"{"message": "Missing Access", "code": 50001}"#,
            ))
            .mount(&server)
            .await;

        let outcome = run(
            &server,
            "try { await discord.searchGuildMessages({ content: 'playlist' }); } catch (e) { return e.message; }",
        )
        .await;

        assert_eq!(
            outcome,
            value(json!(
                r#"HTTP request failed with 403 Forbidden: {"message": "Missing Access", "code": 50001}"#
            ))
        );
    }

    #[tokio::test]
    async fn get_channel_message_returns_the_message_view() {
        let server = MockServer::start().await;
        mount_reply_with_gif(&server).await;

        let outcome = run(
            &server,
            "return await discord.getChannelMessage({ channelId: '1200000000000000001', messageId: '1100000000000000002' });",
        )
        .await;

        assert_eq!(
            outcome,
            value(json!({
                "id": "1100000000000000002",
                "channelId": "1200000000000000001",
                "author": { "id": "1300000000000000002", "name": "bob" },
                "content": "can we have this as an emoji?",
                "timestamp": "2026-10-19T12:35:00Z",
                "attachments": [
                    {
                        "id": "1400000000000000001",
                        "filename": "dance.gif",
                        "contentType": "image/gif",
                        "size": 6,
                    },
                ],
                "replyTo": "1100000000000000001",
            }))
        );
    }

    #[tokio::test]
    async fn list_channel_messages_sends_the_cursor_and_returns_the_messages_oldest_first() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v10/channels/1200000000000000001/messages"))
            .and(query_param("before", "1100000000000000030"))
            .and(query_param("limit", "50"))
            .respond_with(json_response(200, CHANNEL_MESSAGES))
            .mount(&server)
            .await;

        let outcome = run(
            &server,
            "return await discord.listChannelMessages({ channelId: '1200000000000000001', before: '1100000000000000030' });",
        )
        .await;

        assert_eq!(
            outcome,
            value(json!([
                {
                    "id": "1100000000000000021",
                    "channelId": "1200000000000000001",
                    "author": { "id": "1300000000000000001", "name": "alice" },
                    "content": "try clearing the cache in settings",
                    "timestamp": "2026-10-19T08:00:00Z",
                    "attachments": [],
                    "replyTo": null,
                },
                {
                    "id": "1100000000000000022",
                    "channelId": "1200000000000000001",
                    "author": { "id": "1300000000000000002", "name": "bob" },
                    "content": "thanks, that fixed it",
                    "timestamp": "2026-10-19T08:01:10Z",
                    "attachments": [],
                    "replyTo": null,
                },
            ]))
        );
    }

    #[tokio::test]
    async fn create_reaction_sends_the_encoded_custom_emoji() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(
                "/api/v10/channels/1200000000000000001/messages/1100000000000000002/reactions/dance:1600000000000000001/@me",
            ))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let outcome = run(
            &server,
            "return await discord.createReaction({ channelId: '1200000000000000001', messageId: '1100000000000000002', emoji: '<:dance:1600000000000000001>' });",
        )
        .await;

        assert_eq!(outcome, value(json!({ "ok": true })));
    }

    #[tokio::test]
    async fn create_reaction_sends_the_percent_encoded_unicode_emoji() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(
                "/api/v10/channels/1200000000000000001/messages/1100000000000000002/reactions/%F0%9F%98%80/@me",
            ))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let outcome = run(
            &server,
            "return await discord.createReaction({ channelId: '1200000000000000001', messageId: '1100000000000000002', emoji: '😀' });",
        )
        .await;

        assert_eq!(outcome, value(json!({ "ok": true })));
    }

    #[tokio::test]
    async fn create_guild_emoji_uploads_the_attachment_with_its_content_type() {
        let server = MockServer::start().await;
        mount_reply_with_gif(&server).await;
        Mock::given(method("GET"))
            .and(path(
                "/attachments/1200000000000000001/1400000000000000001/dance.gif",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_raw(b"GIF89a", "image/gif"))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v10/guilds/{GUILD_ID}/emojis")))
            .and(body_json(json!({
                "name": "dance",
                "image": "data:image/gif;base64,R0lGODlh",
            })))
            .respond_with(json_response(200, CREATED_EMOJI))
            .mount(&server)
            .await;

        let outcome = run(
            &server,
            "return await discord.createGuildEmoji({ name: 'dance', channelId: '1200000000000000001', messageId: '1100000000000000002', attachmentId: '1400000000000000001' });",
        )
        .await;

        assert_eq!(
            outcome,
            value(json!({
                "id": "1600000000000000001",
                "name": "dance",
                "text": "<a:dance:1600000000000000001>",
            }))
        );
    }

    #[tokio::test]
    async fn create_guild_emoji_fails_for_an_unknown_attachment() {
        let server = MockServer::start().await;
        mount_reply_with_gif(&server).await;

        let outcome = run(
            &server,
            "try { await discord.createGuildEmoji({ name: 'dance', channelId: '1200000000000000001', messageId: '1100000000000000002', attachmentId: '1400000000000000099' }); } catch (e) { return e.message; }",
        )
        .await;

        assert_eq!(
            outcome,
            value(json!("attachment not found: 1400000000000000099"))
        );
    }

    #[tokio::test]
    async fn an_invalid_id_is_an_operation_error() {
        let server = MockServer::start().await;

        let outcome = run(
            &server,
            "return await Promise.all(['general', '0'].map((channelId) =>
                discord.getChannelMessage({ channelId, messageId: '1100000000000000002' }).catch((e) => e.message)));",
        )
        .await;

        assert_eq!(
            outcome,
            value(json!(["invalid id: general", "invalid id: 0"]))
        );
    }
}
