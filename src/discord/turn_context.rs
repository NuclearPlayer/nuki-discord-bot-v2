//! Builds the turn context: the runtime context, and the stored tool turns of the history.

use std::collections::HashMap;

use chrono::Utc;
use futures::future::try_join_all;
use serenity::model::id::GuildId;

use crate::app::App;
use crate::discord::attachments::ResolvedMessage;
use crate::prompts::system::{CustomEmoji, RuntimeContext};
use crate::stats::recorder::RunStats;
use crate::workflows::TurnContext;

/// Builds the turn context in the server `guild_id`, in the run of `stats`. Adds the stored tool
/// turns of the history messages.
pub async fn turn_context(
    app: &App,
    guild_id: GuildId,
    channel_name: &str,
    history: Vec<ResolvedMessage>,
    unit_len: usize,
    stats: RunStats,
) -> anyhow::Result<TurnContext> {
    let stored_turns = stored_turns(app, &history).await?;
    Ok(TurnContext {
        channel_name: channel_name.to_string(),
        history,
        unit_len,
        stored_turns,
        runtime: runtime_context(app, guild_id, channel_name).await?,
        sandbox: app.sandbox_for(guild_id, stats.clone()),
        stats,
    })
}

async fn runtime_context(
    app: &App,
    guild_id: GuildId,
    channel_name: &str,
) -> anyhow::Result<RuntimeContext> {
    let emoji = guild_id
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

/// Loads the stored tool turns of the history. The store keeps each turn with the channel of
/// its message. An intake's history has messages from two channels: the parent channel and the
/// thread.
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
