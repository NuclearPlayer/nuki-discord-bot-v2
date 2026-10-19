//! The start of the services when the bot knows its server.

use std::sync::Arc;
use std::time::Duration;

use serenity::model::channel::Message;
use serenity::model::id::GuildId;
use tokio::sync::mpsc;

use crate::app::App;
use crate::config::Config;
use crate::discord::pipeline::spawn_unit_reader;
use crate::routing::debounce::Debouncer;
use crate::tasks::{spawn_intake_timeout, spawn_repo_fetch};

/// The `App`, and the debouncer that sends the units of messages to the pipeline.
pub struct Services {
    pub app: Arc<App>,
    pub debouncer: Debouncer<Message>,
}

impl Services {
    /// Builds the `App`, and starts the debouncer, the unit reader, and the background tasks.
    pub async fn start(config: Config, guild_id: GuildId) -> anyhow::Result<Services> {
        let quiet = Duration::from_secs(config.debounce_seconds);
        let fetch_period = Duration::from_secs(config.repo_fetch_interval_minutes * 60);

        let app = App::build(config, guild_id).await?;
        tracing::info!(guild_id = guild_id.get(), "services ready");

        let (units_sender, units) = mpsc::unbounded_channel();
        let debouncer = Debouncer::new(quiet, units_sender);
        spawn_unit_reader(Arc::clone(&app), units);
        spawn_repo_fetch(Arc::clone(&app.repos), fetch_period);
        spawn_intake_timeout(Arc::clone(&app));

        Ok(Services { app, debouncer })
    }
}
