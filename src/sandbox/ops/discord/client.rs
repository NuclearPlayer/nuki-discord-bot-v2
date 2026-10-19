use std::num::NonZeroU64;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::header::AUTHORIZATION;
use reqwest::{Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use serenity::builder::GetMessages;
use serenity::http::Http;
use serenity::model::channel::{Attachment, Message, ReactionType};
use serenity::model::id::{ChannelId, GuildId, MessageId};
use thiserror::Error;

use super::views::{EmojiView, MessageView, SearchResult};
use crate::stats::event::{EmojiCreated, Event, ReactionAdded};
use crate::stats::recorder::RunStats;

const DEFAULT_SEARCH_LIMIT: u32 = 25;
const DEFAULT_LIST_LIMIT: u32 = 50;
const MAX_LIST_LIMIT: u32 = 100;

/// What the `discord.*` operations need to call Discord.
pub struct DiscordOps {
    pub http: Arc<Http>,
    pub rest: reqwest::Client,
    /// `https://discord.com/api/v10` in production.
    pub api_base: String,
    pub bot_token: String,
    pub guild_id: u64,
    /// The recorder of the run whose scripts call the operations.
    pub stats: RunStats,
}

#[derive(Debug, Error)]
pub(super) enum DiscordOpError {
    #[error("invalid id: {0}")]
    InvalidId(String),
    #[error("invalid emoji: {0}")]
    InvalidEmoji(String),
    #[error("attachment not found: {0}")]
    AttachmentNotFound(String),
    #[error("attachment has no content type: {0}")]
    MissingContentType(String),
    /// Boxed, because `serenity::Error` is large.
    #[error("Discord request failed: {0}")]
    Discord(Box<serenity::Error>),
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("HTTP request failed with {status}: {body}")]
    Status { status: StatusCode, body: String },
}

impl From<serenity::Error> for DiscordOpError {
    fn from(error: serenity::Error) -> Self {
        Self::Discord(Box::new(error))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SearchParams {
    content: Option<String>,
    author_id: Option<String>,
    channel_id: Option<String>,
    #[serde(default = "default_search_limit")]
    limit: u32,
    offset: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MessageParams {
    channel_id: String,
    message_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ListParams {
    channel_id: String,
    before: Option<String>,
    after: Option<String>,
    around: Option<String>,
    #[serde(default = "default_list_limit")]
    limit: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReactionParams {
    channel_id: String,
    message_id: String,
    emoji: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EmojiParams {
    name: String,
    channel_id: String,
    message_id: String,
    attachment_id: String,
}

fn default_search_limit() -> u32 {
    DEFAULT_SEARCH_LIMIT
}

fn default_list_limit() -> u32 {
    DEFAULT_LIST_LIMIT
}

/// The query string of `GET /guilds/{guild.id}/messages/search`.
#[derive(Debug, Serialize)]
struct SearchQuery<'a> {
    content: Option<&'a str>,
    author_id: Option<u64>,
    channel_id: Option<u64>,
    limit: u32,
    offset: Option<u32>,
}

impl<'a> SearchQuery<'a> {
    fn new(params: &'a SearchParams) -> Result<Self, DiscordOpError> {
        Ok(Self {
            content: params.content.as_deref(),
            author_id: optional_id(params.author_id.as_deref())?,
            channel_id: optional_id(params.channel_id.as_deref())?,
            limit: params.limit,
            offset: params.offset,
        })
    }
}

/// The body of `GET /guilds/{guild.id}/messages/search`.
#[derive(Debug, Deserialize)]
struct SearchResponse {
    total_results: u64,
    /// Each inner list holds one matching message. Discord no longer sends context messages in it.
    messages: Vec<Vec<Message>>,
}

impl DiscordOps {
    pub(super) async fn search_guild_messages(
        &self,
        params: SearchParams,
    ) -> Result<SearchResult, DiscordOpError> {
        let url = format!("{}/guilds/{}/messages/search", self.api_base, self.guild_id);
        let response = self
            .rest
            .get(url)
            .header(AUTHORIZATION, format!("Bot {}", self.bot_token))
            .query(&SearchQuery::new(&params)?)
            .send()
            .await?;
        let response: SearchResponse = successful(response).await?.json().await?;
        Ok(SearchResult {
            total_results: response.total_results,
            messages: response
                .messages
                .iter()
                .flatten()
                .map(MessageView::from)
                .collect(),
        })
    }

    pub(super) async fn get_channel_message(
        &self,
        params: MessageParams,
    ) -> Result<MessageView, DiscordOpError> {
        let message = self
            .fetch_message(&params.channel_id, &params.message_id)
            .await?;
        Ok(MessageView::from(&message))
    }

    pub(super) async fn list_channel_messages(
        &self,
        params: ListParams,
    ) -> Result<Vec<MessageView>, DiscordOpError> {
        let channel = ChannelId::new(parse_id(&params.channel_id)?);
        let messages = channel.messages(&self.http, get_messages(&params)?).await?;
        // Discord sends the newest message first.
        Ok(messages.iter().rev().map(MessageView::from).collect())
    }

    pub(super) async fn create_reaction(
        &self,
        params: ReactionParams,
    ) -> Result<Value, DiscordOpError> {
        let channel = ChannelId::new(parse_id(&params.channel_id)?);
        let message = MessageId::new(parse_id(&params.message_id)?);
        let emoji = ReactionType::try_from(params.emoji.as_str())
            .map_err(|_| DiscordOpError::InvalidEmoji(params.emoji.clone()))?;
        channel.create_reaction(&self.http, message, emoji).await?;
        let added = ReactionAdded {
            message_id: message.get(),
            emoji: params.emoji,
        };
        self.stats.record(None, Event::ReactionAdded(added)).await;
        Ok(json!({ "ok": true }))
    }

    pub(super) async fn create_guild_emoji(
        &self,
        params: EmojiParams,
    ) -> Result<EmojiView, DiscordOpError> {
        let message = self
            .fetch_message(&params.channel_id, &params.message_id)
            .await?;
        let attachment = find_attachment(&message, &params.attachment_id)?;
        let image = self.data_uri(attachment).await?;
        let emoji = GuildId::new(self.guild_id)
            .create_emoji(&self.http, &params.name, &image)
            .await?;
        let created = EmojiCreated {
            emoji_id: emoji.id.get(),
            name: emoji.name.clone(),
        };
        self.stats.record(None, Event::EmojiCreated(created)).await;
        Ok(EmojiView::from(&emoji))
    }

    async fn fetch_message(
        &self,
        channel_id: &str,
        message_id: &str,
    ) -> Result<Message, DiscordOpError> {
        let channel = ChannelId::new(parse_id(channel_id)?);
        let message = MessageId::new(parse_id(message_id)?);
        Ok(channel.message(&self.http, message).await?)
    }

    /// Downloads the attachment and makes the `data:` URI that Discord takes as an emoji image.
    async fn data_uri(&self, attachment: &Attachment) -> Result<String, DiscordOpError> {
        let content_type = attachment
            .content_type
            .as_deref()
            .ok_or_else(|| DiscordOpError::MissingContentType(attachment.id.to_string()))?;
        let response = self.rest.get(&attachment.url).send().await?;
        let data = successful(response).await?.bytes().await?;
        Ok(format!(
            "data:{content_type};base64,{}",
            STANDARD.encode(data)
        ))
    }
}

/// Passes a success response through. Any other status is an error with the body,
/// so that the script sees the message of Discord, for example `Missing Access`.
async fn successful(response: Response) -> Result<Response, DiscordOpError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await?;
    Err(DiscordOpError::Status { status, body })
}

/// Discord takes one cursor. If a script gives more than one, `around` wins over `after`, and `after` over `before`.
fn get_messages(params: &ListParams) -> Result<GetMessages, DiscordOpError> {
    let limit = u8::try_from(params.limit.min(MAX_LIST_LIMIT)).expect("the limit is at most 100");
    let builder = GetMessages::new().limit(limit);
    let builder = optional_id(params.before.as_deref())?
        .map_or(builder, |id| builder.before(MessageId::new(id)));
    let builder = optional_id(params.after.as_deref())?
        .map_or(builder, |id| builder.after(MessageId::new(id)));
    let builder = optional_id(params.around.as_deref())?
        .map_or(builder, |id| builder.around(MessageId::new(id)));
    Ok(builder)
}

fn find_attachment<'a>(
    message: &'a Message,
    attachment_id: &str,
) -> Result<&'a Attachment, DiscordOpError> {
    let id = parse_id(attachment_id)?;
    message
        .attachments
        .iter()
        .find(|attachment| attachment.id.get() == id)
        .ok_or_else(|| DiscordOpError::AttachmentNotFound(attachment_id.to_owned()))
}

/// Parses a Discord ID. Zero is not an ID: Serenity panics on it.
fn parse_id(value: &str) -> Result<u64, DiscordOpError> {
    value
        .parse::<NonZeroU64>()
        .map(NonZeroU64::get)
        .map_err(|_| DiscordOpError::InvalidId(value.to_owned()))
}

fn optional_id(value: Option<&str>) -> Result<Option<u64>, DiscordOpError> {
    value.map(parse_id).transpose()
}
