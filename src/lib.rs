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

use config::Config;

pub async fn run() -> anyhow::Result<()> {
    config::load_dotenv()?;
    logging::init();
    let _config = Config::from_env()?;
    tracing::info!("configuration loaded");
    Ok(())
}
