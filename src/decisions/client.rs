use std::time::{Duration, Instant};

use indexmap::IndexMap;
use reqwest::Response;
use serde::Deserialize;

use super::types::{Answer, DecisionRequest, DecisionResponse, Question};
use crate::stats::duration_ms;
use crate::stats::event::{Event, JevCall, JevPurpose};
use crate::stats::recorder::RunStats;

pub const JEV_MODEL: &str = "liquid/d1-ultra";

/// The path of the Decisions API, after the base URL.
pub(crate) const DECISIONS_PATH: &str = "/api/alpha/decisions";

#[derive(Debug, thiserror::Error)]
pub enum DecisionError {
    #[error("the call to Jev failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Jev returned the status {status}: {message}")]
    Api { status: u16, message: String },
    #[error("the response of Jev does not parse: {0}")]
    Json(#[from] serde_json::Error),
}

/// Jev's response, with its answers in the raw JSON that Jev sent.
struct Decided {
    response: DecisionResponse,
    raw_answers: serde_json::Value,
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
        stats: &RunStats,
        purpose: JevPurpose,
    ) -> Result<IndexMap<String, Answer>, DecisionError> {
        let question_ids = questions.keys().cloned().collect();
        let started = Instant::now();
        let result = self.request(state, questions).await;
        let call = jev_call(purpose, question_ids, started.elapsed(), &result);
        stats.record(None, Event::JevCall(call)).await;
        result.map(|decided| decided.response.answers)
    }

    async fn request(
        &self,
        state: serde_json::Value,
        questions: IndexMap<String, Question>,
    ) -> Result<Decided, DecisionError> {
        let request = DecisionRequest {
            model: JEV_MODEL.to_string(),
            state,
            questions,
        };
        let response = self
            .http
            .post(format!("{}{DECISIONS_PATH}", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await?;
        let body: serde_json::Value = ensure_success(response).await?.json().await?;
        let raw_answers = body["answers"].clone();
        let response = serde_json::from_value(body)?;
        Ok(Decided {
            response,
            raw_answers,
        })
    }
}

/// If the call succeeds, the event has the answers, the model, and the usage. If the call fails,
/// the event has the error.
fn jev_call(
    purpose: JevPurpose,
    questions: Vec<String>,
    elapsed: Duration,
    result: &Result<Decided, DecisionError>,
) -> JevCall {
    let without_result = JevCall {
        purpose,
        questions,
        answers: serde_json::Value::Null,
        model: None,
        duration_ms: duration_ms(elapsed),
        input_tokens: None,
        output_tokens: None,
        cost_usd: None,
        error: None,
    };
    match result {
        Ok(Decided {
            response,
            raw_answers,
        }) => {
            let usage = response.usage.as_ref();
            JevCall {
                answers: raw_answers.clone(),
                model: response.model.clone(),
                input_tokens: usage.and_then(|usage| usage.input_tokens),
                output_tokens: usage.and_then(|usage| usage.output_tokens),
                cost_usd: usage.and_then(|usage| usage.cost),
                ..without_result
            }
        }
        Err(error) => JevCall {
            error: Some(error.to_string()),
            ..without_result
        },
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

/// Returns the `error.message` field of the body. If the body does not have this field,
/// returns the full body.
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

    use chrono::Utc;

    use super::*;
    use crate::stats::test_support::{
        duration_of, events_between, run_event, test_run, unrecorded_run,
    };
    use crate::test_support::json_response;

    const RESPONSE: &str = include_str!("../../tests/fixtures/decisions/response.json");
    const ERROR: &str = include_str!("../../tests/fixtures/decisions/error.json");

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

    async fn server_responding_with(response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(DECISIONS_PATH))
            .respond_with(response)
            .mount(&server)
            .await;
        server
    }

    async fn decide_with_stats(
        server: &MockServer,
        stats: &RunStats,
    ) -> Result<IndexMap<String, Answer>, DecisionError> {
        JevClient::new("test-key".to_string(), server.uri())
            .decide(state(), questions(), stats, JevPurpose::IntakeFields)
            .await
    }

    #[tokio::test]
    async fn decide_records_the_jev_call_with_the_usage() {
        let server = server_responding_with(json_response(200, RESPONSE)).await;
        let (store, stats) = test_run().await;
        let before = Utc::now();

        decide_with_stats(&server, &stats).await.unwrap();

        let after = Utc::now();
        assert_eq!(
            events_between(&store, before, after).await,
            vec![run_event(
                "jev.call",
                json!({
                    "purpose": "intake_fields",
                    "questions": ["category", "has_version"],
                    "answers": {
                        "category": {
                            "type": "choice",
                            "choice": "bug_report",
                            "confidence": 0.67,
                            "probabilities": { "help_request": 0.14, "bug_report": 0.86 }
                        },
                        "has_version": { "type": "noul", "noul": 0.04 }
                    },
                    "model": "typesafe/jev-1.14-20261014",
                    "input_tokens": 476,
                    "output_tokens": 70,
                    "cost_usd": 0.000019992,
                    "error": null,
                })
            )]
        );
    }

    #[tokio::test]
    async fn a_jev_call_records_the_duration_of_the_request() {
        let server = server_responding_with(
            json_response(200, RESPONSE).set_delay(Duration::from_millis(50)),
        )
        .await;
        let (store, stats) = test_run().await;

        decide_with_stats(&server, &stats).await.unwrap();

        assert!(duration_of(&store, "jev.call").await >= 50);
    }

    #[tokio::test]
    async fn a_failed_decision_records_the_error() {
        let server = server_responding_with(json_response(400, ERROR)).await;
        let (store, stats) = test_run().await;
        let before = Utc::now();

        decide_with_stats(&server, &stats).await.unwrap_err();

        let after = Utc::now();
        assert_eq!(
            events_between(&store, before, after).await,
            vec![run_event(
                "jev.call",
                json!({
                    "purpose": "intake_fields",
                    "questions": ["category", "has_version"],
                    "answers": null,
                    "model": null,
                    "input_tokens": null,
                    "output_tokens": null,
                    "cost_usd": null,
                    "error": "Jev returned the status 400: bad question",
                })
            )]
        );
    }

    #[tokio::test]
    async fn decide_sends_the_model_the_state_and_the_questions() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(DECISIONS_PATH))
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

        decide_with_stats(&server, &unrecorded_run().await)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn decide_returns_the_choice_answer_with_all_probabilities() {
        let server = server_responding_with(json_response(200, RESPONSE)).await;

        let answers = decide_with_stats(&server, &unrecorded_run().await)
            .await
            .unwrap();

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

        let answers = decide_with_stats(&server, &unrecorded_run().await)
            .await
            .unwrap();

        assert_eq!(answers["has_version"], Answer::Noul { noul: 0.04 });
    }

    #[tokio::test]
    async fn decide_returns_an_api_error_with_the_status_and_the_message() {
        let server = server_responding_with(json_response(400, ERROR)).await;

        let error = decide_with_stats(&server, &unrecorded_run().await)
            .await
            .unwrap_err();

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

        let error = decide_with_stats(&server, &unrecorded_run().await)
            .await
            .unwrap_err();

        assert!(
            matches!(
                &error,
                DecisionError::Api { status: 502, message } if message == "upstream unavailable"
            ),
            "unexpected error: {error:?}"
        );
    }
}
