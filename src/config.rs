use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, PartialEq, Deserialize)]
pub struct Config {
    pub discord_token: String,
    pub openrouter_api_key: String,
    pub github_token: String,
    pub admin_log_channel_id: u64,
    #[serde(default = "default_context_messages")]
    pub context_messages: usize,
    #[serde(default = "default_chat_cooldown_minutes")]
    pub chat_cooldown_minutes: u64,
    #[serde(default = "default_threshold")]
    pub threshold_help: f64,
    #[serde(default = "default_threshold")]
    pub threshold_bug: f64,
    #[serde(default = "default_threshold")]
    pub threshold_feature: f64,
    #[serde(default = "default_threshold")]
    pub threshold_chat: f64,
    #[serde(default = "default_threshold")]
    pub threshold_help_quality: f64,
    #[serde(default = "default_intake_timeout_hours")]
    pub intake_timeout_hours: u64,
    #[serde(default = "default_repo_fetch_interval_minutes")]
    pub repo_fetch_interval_minutes: u64,
    #[serde(default = "default_debounce_seconds")]
    pub debounce_seconds: u64,
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
}

fn default_context_messages() -> usize {
    15
}

fn default_chat_cooldown_minutes() -> u64 {
    15
}

fn default_threshold() -> f64 {
    0.7
}

fn default_intake_timeout_hours() -> u64 {
    24
}

fn default_repo_fetch_interval_minutes() -> u64 {
    15
}

fn default_debounce_seconds() -> u64 {
    3
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("/data")
}

impl Config {
    /// Reads the process environment. Call `load_dotenv` first to include `.env`.
    pub fn from_env() -> envy::Result<Config> {
        Config::from_pairs(std::env::vars())
    }

    pub fn from_pairs<K, V>(pairs: impl IntoIterator<Item = (K, V)>) -> envy::Result<Config>
    where
        K: Into<String>,
        V: Into<String>,
    {
        envy::from_iter(
            pairs
                .into_iter()
                .map(|(name, value)| (name.into(), value.into())),
        )
    }
}

/// If `.env` exists in the working directory, loads the file into the environment.
pub fn load_dotenv() -> Result<(), dotenvy::Error> {
    match dotenvy::from_path(".env") {
        Err(error) if error.not_found() => Ok(()),
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIRED: [(&str, &str); 4] = [
        ("DISCORD_TOKEN", "discord-token"),
        ("OPENROUTER_API_KEY", "openrouter-key"),
        ("GITHUB_TOKEN", "github-token"),
        ("ADMIN_LOG_CHANNEL_ID", "3003"),
    ];

    #[test]
    fn from_pairs_reads_all_required_values_and_applies_the_defaults() {
        let config = Config::from_pairs(REQUIRED).unwrap();

        assert_eq!(
            config,
            Config {
                discord_token: "discord-token".to_string(),
                openrouter_api_key: "openrouter-key".to_string(),
                github_token: "github-token".to_string(),
                admin_log_channel_id: 3003,
                context_messages: 15,
                chat_cooldown_minutes: 15,
                threshold_help: 0.7,
                threshold_bug: 0.7,
                threshold_feature: 0.7,
                threshold_chat: 0.7,
                threshold_help_quality: 0.7,
                intake_timeout_hours: 24,
                repo_fetch_interval_minutes: 15,
                debounce_seconds: 3,
                data_dir: PathBuf::from("/data"),
            }
        );
    }

    #[test]
    fn from_pairs_reads_values_that_override_the_defaults() {
        let overrides = [
            ("CONTEXT_MESSAGES", "30"),
            ("CHAT_COOLDOWN_MINUTES", "5"),
            ("THRESHOLD_HELP", "0.1"),
            ("THRESHOLD_BUG", "0.2"),
            ("THRESHOLD_FEATURE", "0.3"),
            ("THRESHOLD_CHAT", "0.4"),
            ("THRESHOLD_HELP_QUALITY", "0.5"),
            ("INTAKE_TIMEOUT_HOURS", "48"),
            ("REPO_FETCH_INTERVAL_MINUTES", "60"),
            ("DEBOUNCE_SECONDS", "10"),
            ("DATA_DIR", "/tmp/nuki"),
        ];

        let config = Config::from_pairs(REQUIRED.into_iter().chain(overrides)).unwrap();

        assert_eq!(
            config,
            Config {
                discord_token: "discord-token".to_string(),
                openrouter_api_key: "openrouter-key".to_string(),
                github_token: "github-token".to_string(),
                admin_log_channel_id: 3003,
                context_messages: 30,
                chat_cooldown_minutes: 5,
                threshold_help: 0.1,
                threshold_bug: 0.2,
                threshold_feature: 0.3,
                threshold_chat: 0.4,
                threshold_help_quality: 0.5,
                intake_timeout_hours: 48,
                repo_fetch_interval_minutes: 60,
                debounce_seconds: 10,
                data_dir: PathBuf::from("/tmp/nuki"),
            }
        );
    }

    #[test]
    fn from_pairs_fails_when_a_required_variable_is_missing() {
        let without_token = REQUIRED
            .into_iter()
            .filter(|(name, _)| *name != "DISCORD_TOKEN");

        let error = Config::from_pairs(without_token).unwrap_err();

        assert_eq!(error.to_string(), "missing value for field discord_token");
    }
}
