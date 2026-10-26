use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use reqwest::Url;
use serenity::http::Http;
use serenity::model::id::GuildId;

use crate::admin::log::AdminLog;
use crate::config::Config;
use crate::decisions::client::JevClient;
use crate::github::client::GitHubClient;
use crate::github::types::OrgRepo;
use crate::llm::agent::{LlmClient, OPENROUTER_BASE_URL};
use crate::llm::models_list::ModelsClient;
use crate::llm::selection::{ActiveModel, DEFAULT_MODEL_ID};
use crate::repos::catalog::{CatalogEntry, NUCLEAR_REPO, RepoCatalog, RepoId};
use crate::repos::manager::RepoManager;
use crate::routing::classify::Classifier;
use crate::routing::cooldown::Cooldowns;
use crate::routing::dispatch::Thresholds;
use crate::sandbox::ops::discord::DiscordOps;
use crate::sandbox::ops::full_table;
use crate::sandbox::ops::web::WebOps;
use crate::sandbox::runtime::{Sandbox, SandboxLimits};
use crate::stats::recorder::{RunStats, Stats};
use crate::store::Store;
use crate::web::client::TavilyClient;
use crate::workflows::WorkflowDeps;
use crate::workflows::chat::ChatWorkflow;
use crate::workflows::help::HelpWorkflow;
use crate::workflows::intake::IntakeWorkflow;

/// The base of the Jev and the models APIs. The LLM uses `OPENROUTER_BASE_URL`.
const OPENROUTER_URL: &str = "https://openrouter.ai";
const DISCORD_API_BASE: &str = "https://discord.com/api/v10";
const GITHUB_API_BASE: &str = "https://api.github.com";
const GITHUB_GRAPHQL_URL: &str = "https://api.github.com/graphql";
const GITHUB_ORG: &str = "NuclearPlayer";
const TAVILY_API_BASE: &str = "https://api.tavily.com";

/// The Discord user of the bot.
pub struct BotIdentity {
    pub user_id: u64,
    pub name: String,
}

/// The services that the handler and the background tasks share.
pub struct App {
    pub config: Config,
    pub http: Arc<Http>,
    pub nuki: BotIdentity,
    pub store: Store,
    pub stats: Stats,
    pub active_model: Arc<ActiveModel>,
    pub models: ModelsClient,
    pub classifier: Classifier,
    pub thresholds: Thresholds,
    pub cooldowns: Cooldowns,
    pub chat: ChatWorkflow,
    pub help: HelpWorkflow,
    pub intake: IntakeWorkflow,
    pub repos: Arc<RepoManager>,
    pub github: Arc<GitHubClient>,
    pub tavily: Arc<TavilyClient>,
    pub admin_log: AdminLog,
    pub rest: reqwest::Client,
}

impl App {
    pub async fn build(config: Config) -> anyhow::Result<Arc<App>> {
        let store = open_store(&config).await?;

        let api_base = Url::parse(GITHUB_API_BASE)
            .with_context(|| format!("could not parse the GitHub API base {GITHUB_API_BASE}"))?;
        let graphql_url = Url::parse(GITHUB_GRAPHQL_URL).with_context(|| {
            format!("could not parse the GitHub GraphQL URL {GITHUB_GRAPHQL_URL}")
        })?;
        let github = Arc::new(GitHubClient::new(
            config.github_token.clone(),
            api_base,
            graphql_url,
        ));
        let org_repos = github
            .list_org_repos(GITHUB_ORG)
            .await
            .with_context(|| format!("could not list the repositories of {GITHUB_ORG}"))?;
        let repos = Arc::new(RepoManager::new(
            config.data_dir.join("repos"),
            catalog(org_repos)?,
        ));
        repos
            .ensure_cloned(&RepoId::nuclear())
            .await
            .with_context(|| format!("could not clone {NUCLEAR_REPO}"))?;

        let tavily_api_base = Url::parse(TAVILY_API_BASE)
            .with_context(|| format!("could not parse the Tavily API base {TAVILY_API_BASE}"))?;
        let tavily = Arc::new(TavilyClient::new(tavily_api_base));

        let http = Arc::new(Http::new(&config.discord_token));
        let current_user = http
            .get_current_user()
            .await
            .context("could not get the Discord user of the bot")?;
        let nuki = BotIdentity {
            user_id: current_user.id.get(),
            name: current_user.display_name().to_string(),
        };

        let llm = Arc::new(LlmClient::new(
            &config.openrouter_api_key,
            OPENROUTER_BASE_URL,
        ));
        let models = ModelsClient::new(OPENROUTER_URL.to_string());
        let active_model = Arc::new(ActiveModel::load(store.settings(), DEFAULT_MODEL_ID).await?);

        let jev = Arc::new(JevClient::new(
            config.openrouter_api_key.clone(),
            OPENROUTER_URL.to_string(),
        ));
        let deps = WorkflowDeps {
            llm,
            jev: Arc::clone(&jev),
            model: Arc::clone(&active_model),
            help_quality_threshold: config.threshold_help_quality,
        };

        let stats = Stats::new(store.events());

        Ok(Arc::new(App {
            nuki,
            store,
            active_model,
            models,
            classifier: Classifier::new(jev),
            thresholds: Thresholds::from(&config),
            cooldowns: Cooldowns::new(Duration::from_secs(config.chat_cooldown_minutes * 60)),
            chat: ChatWorkflow { deps: deps.clone() },
            help: HelpWorkflow { deps: deps.clone() },
            intake: IntakeWorkflow { deps },
            repos,
            github,
            tavily,
            admin_log: AdminLog {
                http: Arc::clone(&http),
                channel_id: config.admin_log_channel_id,
                stats: stats.clone(),
            },
            stats,
            http,
            rest: reqwest::Client::new(),
            config,
        }))
    }

    /// The sandbox of the `run_script` tool, with the Discord operations of `guild_id`, which
    /// record their events in the run of `stats`.
    pub fn sandbox_for(&self, guild_id: GuildId, stats: RunStats) -> Arc<Sandbox> {
        let discord_ops = DiscordOps {
            http: Arc::clone(&self.http),
            rest: self.rest.clone(),
            api_base: DISCORD_API_BASE.to_string(),
            bot_token: self.config.discord_token.clone(),
            guild_id: guild_id.get(),
            stats,
        };
        let table = full_table(
            discord_ops,
            Arc::clone(&self.repos),
            Arc::clone(&self.github),
            WebOps::new(Arc::clone(&self.tavily)),
        );
        Arc::new(Sandbox::new(Arc::new(table), SandboxLimits::production()))
    }
}

/// Opens `DATA_DIR/nuki.sqlite`, and makes `DATA_DIR` if it does not exist.
async fn open_store(config: &Config) -> anyhow::Result<Store> {
    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("could not make {}", config.data_dir.display()))?;
    let path = config.data_dir.join("nuki.sqlite");
    Store::open(&path)
        .await
        .with_context(|| format!("could not open {}", path.display()))
}

/// `nukeop/nuclear` first, then the repositories of the organization.
fn catalog(org_repos: Vec<OrgRepo>) -> anyhow::Result<RepoCatalog> {
    let nuclear = CatalogEntry {
        id: RepoId::nuclear(),
        description: None,
        clone_url: format!("https://github.com/{NUCLEAR_REPO}.git"),
    };
    let org_entries = org_repos
        .into_iter()
        .map(|repo| {
            Ok(CatalogEntry {
                id: RepoId::parse(&repo.full_name)?,
                description: repo.description,
                clone_url: repo.clone_url,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(RepoCatalog::new(
        std::iter::once(nuclear).chain(org_entries).collect(),
    ))
}
