pub mod admin;
pub mod app;
pub mod config;
pub mod decisions;
pub mod discord;
pub mod github;
pub mod llm;
pub mod logging;
pub mod prompts;
pub mod repos;
pub mod routing;
pub mod sandbox;
pub mod store;
pub mod tasks;
#[cfg(test)]
pub(crate) mod test_support;
pub mod workflows;

use std::sync::Arc;
use std::time::Duration;

use serenity::Client;
use serenity::model::gateway::GatewayIntents;
use tokio::sync::mpsc;

use app::App;
use config::Config;
use discord::handler::Handler;
use discord::pipeline::spawn_unit_reader;
use routing::debounce::Debouncer;

pub async fn run() -> anyhow::Result<()> {
    config::load_dotenv()?;
    logging::init();
    let config = Config::from_env()?;
    let token = config.discord_token.clone();
    let quiet = Duration::from_secs(config.debounce_seconds);

    let app = App::build(config).await?;
    tracing::info!("services ready");

    let (units_sender, units) = mpsc::unbounded_channel();
    let debouncer = Debouncer::new(quiet, units_sender);
    spawn_unit_reader(Arc::clone(&app), units);

    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILD_MEMBERS;
    // `ClientBuilder` takes an owned `Http`, and `Http` is not `Clone`, so the gateway client has
    // its own `Http` with the same token. The pipeline uses `app.http`.
    let mut client = Client::builder(&token, intents)
        .event_handler(Handler { app, debouncer })
        .await?;
    client.start().await?;
    Ok(())
}
