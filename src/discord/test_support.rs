pub(crate) mod intake_thread_scene;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use reqwest::Url;
use wiremock::MockServer;

use crate::admin::log::AdminLog;
use crate::app::{App, BotIdentity};
use crate::config::Config;
use crate::discord::attachments::{ResolvedAttachment, ResolvedMessage};
use crate::discord::chat_message::{AttachmentMeta, ChatMessage};
use crate::github::client::GitHubClient;
use crate::llm::models_list::ModelsClient;
use crate::repos::catalog::RepoCatalog;
use crate::repos::manager::RepoManager;
use crate::routing::classify::Classifier;
use crate::routing::cooldown::Cooldowns;
use crate::routing::dispatch::Thresholds;
use crate::stats::recorder::Stats;
use crate::store::Store;
use crate::test_support::discord_http;
use crate::web::client::TavilyClient;
use crate::workflows::chat::ChatWorkflow;
use crate::workflows::intake::IntakeWorkflow;
use crate::workflows::test_support::deps;

pub(crate) const NUKI_USER_ID: u64 = 1700000000000000001;
const ADMIN_LOG_CHANNEL_ID: u64 = 1200000000000000009;
pub(crate) const GUILD_ROLES: &str =
    include_str!("../../tests/fixtures/discord/pipeline_guild_roles.json");

pub(crate) async fn test_app(server: &MockServer, repos_dir: &Path) -> App {
    let config = Config::from_pairs([
        ("DISCORD_TOKEN", "test-token".to_string()),
        ("OPENROUTER_API_KEY", "test-key".to_string()),
        ("GITHUB_TOKEN", "test-token".to_string()),
        ("ADMIN_LOG_CHANNEL_ID", ADMIN_LOG_CHANNEL_ID.to_string()),
    ])
    .unwrap();
    let store = Store::open_in_memory().await.unwrap();
    let stats = Stats::new(store.events());
    let http = Arc::new(discord_http(server));
    let deps = deps(server).await;
    let server_url = Url::parse(&server.uri()).unwrap();
    App {
        nuki: BotIdentity {
            user_id: NUKI_USER_ID,
            name: "Nuki".to_string(),
        },
        active_model: Arc::clone(&deps.model),
        models: ModelsClient::new(server.uri()),
        classifier: Classifier::new(Arc::clone(&deps.jev)),
        thresholds: Thresholds::from(&config),
        cooldowns: Cooldowns::new(Duration::from_secs(config.chat_cooldown_minutes * 60)),
        chat: ChatWorkflow { deps: deps.clone() },
        intake: IntakeWorkflow { deps },
        repos: Arc::new(RepoManager::new(
            repos_dir.to_path_buf(),
            RepoCatalog::new(vec![]),
        )),
        github: Arc::new(GitHubClient::new(
            "test-token".to_string(),
            server_url.clone(),
            server_url.clone(),
        )),
        tavily: Arc::new(TavilyClient::new(server_url)),
        admin_log: AdminLog {
            http: Arc::clone(&http),
            channel_id: config.admin_log_channel_id,
            stats: stats.clone(),
        },
        store,
        stats,
        http,
        rest: reqwest::Client::new(),
        config,
    }
}

pub(crate) fn attachment(filename: &str, content_type: Option<&str>) -> AttachmentMeta {
    AttachmentMeta {
        id: 1400000000000000001,
        filename: filename.to_string(),
        content_type: content_type.map(str::to_string),
        size: 2048,
        url: format!("https://cdn.discordapp.com/attachments/1/2/{filename}"),
    }
}

pub(crate) fn user_message(id: u64, content: &str) -> ChatMessage {
    ChatMessage {
        id,
        channel_id: 1200000000000000001,
        author_id: 1300000000000000002,
        author_name: "Bob".to_string(),
        author_roles: vec!["Linux".to_string(), "supporter".to_string()],
        is_nuki: false,
        timestamp: Utc.with_ymd_and_hms(2026, 10, 19, 12, 35, 0).unwrap()
            + chrono::Duration::milliseconds(123),
        content: content.to_string(),
        attachments: vec![],
    }
}

pub(crate) fn nuki_message(id: u64, content: &str) -> ChatMessage {
    ChatMessage {
        author_id: NUKI_USER_ID,
        author_name: "Nuki".to_string(),
        author_roles: vec![],
        is_nuki: true,
        ..user_message(id, content)
    }
}

pub(crate) fn resolved(
    message: ChatMessage,
    attachments: Vec<ResolvedAttachment>,
) -> ResolvedMessage {
    ResolvedMessage {
        message,
        attachments,
        thread: None,
    }
}
