use serde::Deserialize;
use thiserror::Error;

/// Discord allows at most 25 autocomplete choices with names of at most 100 characters.
const MAX_CHOICES: usize = 25;
const MAX_NAME_CHARS: usize = 100;

#[derive(Debug, Error)]
pub enum ModelsError {
    #[error("the call to the OpenRouter models API failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("the OpenRouter models API returned an invalid price: {0}")]
    InvalidPrice(#[from] std::num::ParseFloatError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub prompt_usd_per_m: f64,
    pub completion_usd_per_m: f64,
    pub input_modalities: Vec<String>,
}

/// One autocomplete choice of `/change-model`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelChoice {
    pub name: String,
    pub value: String,
}

/// Lists the OpenRouter models that support tools.
pub struct ModelsClient {
    http: reqwest::Client,
    base_url: String,
}

impl ModelsClient {
    pub fn new(base_url: String) -> ModelsClient {
        ModelsClient {
            http: reqwest::Client::new(),
            base_url,
        }
    }

    /// Gets the models in the sequence of their use in the last week.
    pub async fn list(&self) -> Result<Vec<ModelInfo>, ModelsError> {
        let response: ModelsResponse = self
            .http
            .get(format!("{}/api/v1/models", self.base_url))
            .query(&[("sort", "top-weekly"), ("supported_parameters", "tools")])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        response.data.into_iter().map(ModelInfo::try_from).collect()
    }
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ApiModel>,
}

#[derive(Deserialize)]
struct ApiModel {
    id: String,
    name: String,
    pricing: ApiPricing,
    architecture: ApiArchitecture,
}

/// Prices in USD for each token, as decimal strings.
#[derive(Deserialize)]
struct ApiPricing {
    prompt: String,
    completion: String,
}

#[derive(Deserialize)]
struct ApiArchitecture {
    input_modalities: Vec<String>,
}

impl TryFrom<ApiModel> for ModelInfo {
    type Error = ModelsError;

    fn try_from(model: ApiModel) -> Result<ModelInfo, ModelsError> {
        Ok(ModelInfo {
            id: model.id,
            name: model.name,
            prompt_usd_per_m: usd_per_m(&model.pricing.prompt)?,
            completion_usd_per_m: usd_per_m(&model.pricing.completion)?,
            input_modalities: model.architecture.input_modalities,
        })
    }
}

fn usd_per_m(usd_per_token: &str) -> Result<f64, ModelsError> {
    Ok(usd_per_token.parse::<f64>()? * 1_000_000.0)
}

/// Writes a price as `$<value>/M`, rounded to 3 decimals, without zeros at the end.
pub fn format_usd_per_m(value: f64) -> String {
    let fixed = format!("{value:.3}");
    let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
    format!("${trimmed}/M")
}

/// Makes the autocomplete choices for the models whose ID or name contains `query`, without case.
pub fn choices(models: &[ModelInfo], query: &str) -> Vec<ModelChoice> {
    let query = query.to_lowercase();
    models
        .iter()
        .filter(|model| matches_query(model, &query))
        .take(MAX_CHOICES)
        .map(|model| ModelChoice {
            name: choice_name(model),
            value: model.id.clone(),
        })
        .collect()
}

fn matches_query(model: &ModelInfo, lowercase_query: &str) -> bool {
    model.id.to_lowercase().contains(lowercase_query)
        || model.name.to_lowercase().contains(lowercase_query)
}

fn choice_name(model: &ModelInfo) -> String {
    let name = format!(
        "{} · {} in · {} out",
        model.name,
        format_usd_per_m(model.prompt_usd_per_m),
        format_usd_per_m(model.completion_usd_per_m)
    );
    truncate(name, MAX_NAME_CHARS)
}

/// Keeps `max_chars - 1` characters and adds `…` when the text is longer than `max_chars`.
fn truncate(text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text;
    }
    let kept: String = text.chars().take(max_chars - 1).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer};

    use super::*;
    use crate::llm::test_support::MODELS;
    use crate::test_support::json_response;

    fn model(id: &str, name: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            name: name.to_string(),
            prompt_usd_per_m: 0.3,
            completion_usd_per_m: 2.5,
            input_modalities: vec!["text".to_string()],
        }
    }

    fn choice(name: &str, value: &str) -> ModelChoice {
        ModelChoice {
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn format_usd_per_m_keeps_one_decimal() {
        assert_eq!(format_usd_per_m(0.3), "$0.3/M");
    }

    #[test]
    fn format_usd_per_m_removes_zeros_at_the_end() {
        assert_eq!(format_usd_per_m(2.5), "$2.5/M");
    }

    #[test]
    fn format_usd_per_m_rounds_to_3_decimals() {
        assert_eq!(format_usd_per_m(0.12345), "$0.123/M");
    }

    #[test]
    fn format_usd_per_m_writes_an_integer_without_a_point() {
        assert_eq!(format_usd_per_m(3.0), "$3/M");
    }

    #[test]
    fn choices_filter_by_id_and_name_without_case() {
        let models = [
            model("google/gemini-3.8-flash", "Google: Gemini 3.8 Flash"),
            model("anthropic/claude-sonnet-4", "Anthropic: Claude Sonnet 4"),
            model("openai/gpt-5", "OpenAI: GPT-5 by Google"),
        ];

        assert_eq!(
            choices(&models, "GOOGLE"),
            vec![
                choice(
                    "Google: Gemini 3.8 Flash · $0.3/M in · $2.5/M out",
                    "google/gemini-3.8-flash"
                ),
                choice(
                    "OpenAI: GPT-5 by Google · $0.3/M in · $2.5/M out",
                    "openai/gpt-5"
                ),
            ]
        );
    }

    #[test]
    fn choices_keep_25_models() {
        let models: Vec<ModelInfo> = (1..=30)
            .map(|n| model(&format!("vendor/model-{n}"), &format!("Model {n}")))
            .collect();

        let values: Vec<String> = choices(&models, "")
            .into_iter()
            .map(|choice| choice.value)
            .collect();

        let expected: Vec<String> = (1..=25).map(|n| format!("vendor/model-{n}")).collect();
        assert_eq!(values, expected);
    }

    #[test]
    fn choice_names_are_truncated_to_100_characters() {
        let long_name = "é".repeat(120);
        let models = [model("vendor/long", &long_name)];

        assert_eq!(
            choices(&models, "long"),
            vec![choice(&format!("{}…", "é".repeat(99)), "vendor/long")]
        );
    }

    #[test]
    fn a_choice_name_of_100_characters_is_not_truncated() {
        let name = "x".repeat(100 - " · $0.3/M in · $2.5/M out".chars().count());
        let models = [model("vendor/exact", &name)];

        assert_eq!(
            choices(&models, "exact"),
            vec![choice(
                &format!("{name} · $0.3/M in · $2.5/M out"),
                "vendor/exact"
            )]
        );
    }

    #[tokio::test]
    async fn list_converts_the_prices_to_usd_per_million() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/models"))
            .and(query_param("sort", "top-weekly"))
            .and(query_param("supported_parameters", "tools"))
            .respond_with(json_response(200, MODELS))
            .mount(&server)
            .await;

        let models = ModelsClient::new(server.uri()).list().await.unwrap();

        assert_eq!(
            models,
            vec![
                ModelInfo {
                    id: "google/gemini-3.8-flash".to_string(),
                    name: "Google: Gemini 3.8 Flash".to_string(),
                    prompt_usd_per_m: 0.3,
                    completion_usd_per_m: 2.5,
                    input_modalities: vec![
                        "text".to_string(),
                        "image".to_string(),
                        "file".to_string(),
                        "audio".to_string()
                    ],
                },
                ModelInfo {
                    id: "anthropic/claude-sonnet-4".to_string(),
                    name: "Anthropic: Claude Sonnet 4".to_string(),
                    prompt_usd_per_m: 3.0,
                    completion_usd_per_m: 15.0,
                    input_modalities: vec!["image".to_string(), "text".to_string()],
                },
            ]
        );
    }
}
