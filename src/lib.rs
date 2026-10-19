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
pub mod services;
pub mod store;
pub mod tasks;
#[cfg(test)]
pub(crate) mod test_support;
pub mod workflows;

use serenity::Client;
use serenity::model::gateway::GatewayIntents;

use config::Config;
use discord::handler::Handler;

/// Starts the Discord client. The handler starts the services when the bot knows its server.
pub async fn run() -> anyhow::Result<()> {
    config::load_dotenv()?;
    logging::init();
    let config = Config::from_env()?;
    let token = config.discord_token.clone();

    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILD_MEMBERS;
    // `ClientBuilder` takes an owned `Http`, and `Http` is not `Clone`, so the gateway client has
    // its own `Http` with the same token. The pipeline uses `app.http`.
    let mut client = Client::builder(&token, intents)
        .event_handler(Handler::new(config))
        .await?;
    client.start().await?;
    Ok(())
}
