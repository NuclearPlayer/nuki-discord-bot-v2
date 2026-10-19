use indexmap::IndexMap;
use reqwest::Response;
use serde::Deserialize;

use super::types::{Answer, DecisionRequest, DecisionResponse, Question};

pub const JEV_MODEL: &str = "~typesafe/jev-latest";

#[derive(Debug, thiserror::Error)]
pub enum DecisionError {
    #[error("the call to Jev failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Jev returned the status {status}: {message}")]
    Api { status: u16, message: String },
}

pub struct JevClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl JevClient {
    pub fn new(api_key: String, base_url: String) -> JevClient {
        JevClient {
            http: reqwest::Client::new(),
            api_key,
            base_url,
        }
    }

    pub async fn decide(
        &self,
        state: serde_json::Value,
        questions: IndexMap<String, Question>,
    ) -> Result<IndexMap<String, Answer>, DecisionError> {
        let request = DecisionRequest {
            model: JEV_MODEL.to_string(),
            state,
            questions,
        };
        let response = self
            .http
            .post(format!("{}/api/alpha/decisions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await?;
        let decision: DecisionResponse = ensure_success(response).await?.json().await?;
        Ok(decision.answers)
    }
}

async fn ensure_success(response: Response) -> Result<Response, DecisionError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await?;
    Err(DecisionError::Api {
        status: status.as_u16(),
        message: error_message(body),
    })
}

#[derive(Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Deserialize)]
struct ErrorDetail {
    message: String,
}

/// Returns `error.message` of the body, or the full body when it has no such field.
fn error_message(body: String) -> String {
    serde_json::from_str::<ErrorBody>(&body)
        .map(|parsed| parsed.error.message)
        .unwrap_or(body)
}

#[cfg(test)]
mod tests {
    use indexmap::indexmap;
    use serde_json::{Value, json};
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const RESPONSE: &str = include_str!("../../tests/fixtures/decisions/response.json");

    fn state() -> Value {
        json!({
            "messages": ["alice: does anyone know how to add a plugin?"],
            "new_messages": ["bob: Nuclear crashes when I open the queue"]
        })
    }

    fn questions() -> IndexMap<String, Question> {
        indexmap! {
            "category".to_string() => Question::Choice {
                instructions: "What do the new messages need from Nuki?".to_string(),
                criteria: indexmap! {
                    "help_request".to_string() => "The author wants help with Nuclear.".to_string(),
                    "bug_report".to_string() => "The author reports behavior of Nuclear that is incorrect.".to_string(),
                },
            },
            "has_version".to_string() => Question::Noul {
                instructions: "Does the conversation state the Nuclear version?".to_string(),
            },
        }
    }

    fn json_response(status: u16, body: &str) -> ResponseTemplate {
        ResponseTemplate::new(status).set_body_raw(body, "application/json")
    }

    async fn server_responding_with(response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/alpha/decisions"))
            .respond_with(response)
            .mount(&server)
            .await;
        server
    }

    async fn decide_against(
        server: &MockServer,
    ) -> Result<IndexMap<String, Answer>, DecisionError> {
        JevClient::new("test-key".to_string(), server.uri())
            .decide(state(), questions())
            .await
    }

    #[tokio::test]
    async fn decide_sends_the_model_the_state_and_the_questions() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/alpha/decisions"))
            .and(header("Authorization", "Bearer test-key"))
            .and(body_json(
                serde_json::from_str::<Value>(include_str!(
                    "../../tests/fixtures/decisions/request.json"
                ))
                .unwrap(),
            ))
            .respond_with(json_response(200, RESPONSE))
            .expect(1)
            .mount(&server)
            .await;

        decide_against(&server).await.unwrap();
    }

    #[tokio::test]
    async fn decide_returns_the_choice_answer_with_all_probabilities() {
        let server = server_responding_with(json_response(200, RESPONSE)).await;

        let answers = decide_against(&server).await.unwrap();

        assert_eq!(
            answers["category"],
            Answer::Choice {
                choice: "bug_report".to_string(),
                probabilities: indexmap! {
                    "help_request".to_string() => 0.14,
                    "bug_report".to_string() => 0.86,
                },
            }
        );
    }

    #[tokio::test]
    async fn decide_returns_the_noul_answer() {
        let server = server_responding_with(json_response(200, RESPONSE)).await;

        let answers = decide_against(&server).await.unwrap();

        assert_eq!(answers["has_version"], Answer::Noul { noul: 0.04 });
    }

    #[tokio::test]
    async fn decide_returns_an_api_error_with_the_status_and_the_message() {
        let server = server_responding_with(json_response(
            400,
            r#"{"error":{"code":400,"message":"bad question"}}"#,
        ))
        .await;

        let error = decide_against(&server).await.unwrap_err();

        assert!(
            matches!(
                &error,
                DecisionError::Api { status: 400, message } if message == "bad question"
            ),
            "unexpected error: {error:?}"
        );
    }

    #[tokio::test]
    async fn decide_returns_the_full_body_as_the_message_when_the_body_has_no_error_message() {
        let server = server_responding_with(
            ResponseTemplate::new(502).set_body_string("upstream unavailable"),
        )
        .await;

        let error = decide_against(&server).await.unwrap_err();

        assert!(
            matches!(
                &error,
                DecisionError::Api { status: 502, message } if message == "upstream unavailable"
            ),
            "unexpected error: {error:?}"
        );
    }
}
