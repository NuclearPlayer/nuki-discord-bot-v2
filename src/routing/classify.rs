use std::sync::Arc;

use indexmap::{IndexMap, indexmap};
use serde_json::json;

use super::category::Category;
use super::trigger::ClassifiedTrigger;
use crate::decisions::client::{DecisionError, JevClient};
use crate::decisions::types::{Answer, Question};
use crate::prompts::questions::category;

const CATEGORY_QUESTION: &str = "category";

#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    pub probabilities: IndexMap<Category, f64>,
}

impl Classification {
    pub fn top(&self) -> (Category, f64) {
        self.probabilities
            .iter()
            .map(|(category, probability)| (*category, *probability))
            .max_by(|(_, left), (_, right)| left.total_cmp(right))
            .expect("a classification has at least one probability")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClassifyError {
    #[error(transparent)]
    Decision(#[from] DecisionError),
    #[error("Jev returned no answer to the category question")]
    MissingAnswer,
    #[error("Jev returned an answer to the category question that is not a choice")]
    UnexpectedAnswer,
    #[error("Jev returned a choice answer to the category question with no probabilities")]
    NoProbabilities,
    #[error("Jev returned the unknown category {0}")]
    UnknownCategory(String),
}

pub struct Classifier {
    jev: Arc<JevClient>,
}

impl Classifier {
    pub fn new(jev: Arc<JevClient>) -> Classifier {
        Classifier { jev }
    }

    pub async fn classify(
        &self,
        trigger: ClassifiedTrigger,
        history_lines: &[String],
        new_lines: &[String],
    ) -> Result<Classification, ClassifyError> {
        let state = json!({ "messages": history_lines, "new_messages": new_lines });
        let questions = indexmap! { CATEGORY_QUESTION.to_string() => category_question(trigger) };
        let mut answers = self.jev.decide(state, questions).await?;
        match answers.shift_remove(CATEGORY_QUESTION) {
            Some(Answer::Choice { probabilities, .. }) => classification_of(probabilities),
            Some(_) => Err(ClassifyError::UnexpectedAnswer),
            None => Err(ClassifyError::MissingAnswer),
        }
    }
}

/// The `category` question. A direct message always gets a reply, so its question has no `ignore` option.
fn category_question(trigger: ClassifiedTrigger) -> Question {
    let question = category();
    let criteria = question
        .options
        .iter()
        .filter(|(name, _)| offers(trigger, name))
        .map(|(name, text)| (name.clone(), text.clone()))
        .collect();
    Question::Choice {
        instructions: question.instructions.clone(),
        criteria,
    }
}

fn offers(trigger: ClassifiedTrigger, option_name: &str) -> bool {
    match trigger {
        ClassifiedTrigger::Direct => option_name != Category::Ignore.name(),
        ClassifiedTrigger::Unsolicited => true,
    }
}

fn classification_of(
    probabilities: IndexMap<String, f64>,
) -> Result<Classification, ClassifyError> {
    if probabilities.is_empty() {
        return Err(ClassifyError::NoProbabilities);
    }
    let probabilities = probabilities
        .into_iter()
        .map(|(name, probability)| match Category::from_name(&name) {
            Some(category) => Ok((category, probability)),
            None => Err(ClassifyError::UnknownCategory(name)),
        })
        .collect::<Result<_, _>>()?;
    Ok(Classification { probabilities })
}

#[cfg(test)]
mod tests {
    use indexmap::indexmap;
    use serde_json::{Value, json};
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer};

    use super::*;
    use crate::decisions::client::JEV_MODEL;
    use crate::decisions::test_support::{JEV_PATH, jev_response, mount_jev_answers};

    fn history() -> Vec<String> {
        vec!["alice: does anyone know how to add a plugin?".to_string()]
    }

    fn new_lines() -> Vec<String> {
        vec![
            "bob: hey nuki".to_string(),
            "bob: Nuclear crashes when I open the queue".to_string(),
        ]
    }

    fn request_with_options(option_names: &[&str]) -> Value {
        let question = category();
        let criteria: serde_json::Map<String, Value> = option_names
            .iter()
            .map(|name| (name.to_string(), json!(question.options[*name])))
            .collect();
        json!({
            "model": JEV_MODEL,
            "state": { "messages": history(), "new_messages": new_lines() },
            "questions": {
                "category": {
                    "type": "choice",
                    "instructions": question.instructions,
                    "criteria": criteria,
                }
            }
        })
    }

    /// A `category` choice answer whose choice and confidence are those of the top probability.
    fn answer_with(probabilities: &[(&str, f64)]) -> Value {
        let (choice, confidence) = probabilities
            .iter()
            .copied()
            .max_by(|(_, left), (_, right)| left.total_cmp(right))
            .unwrap();
        let probabilities: serde_json::Map<String, Value> = probabilities
            .iter()
            .map(|(name, probability)| (name.to_string(), json!(probability)))
            .collect();
        json!({
            "category": {
                "type": "choice",
                "choice": choice,
                "confidence": confidence,
                "probabilities": probabilities,
            }
        })
    }

    async fn jev_expecting(request: Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(JEV_PATH))
            .and(body_json(request))
            .respond_with(jev_response(answer_with(&[("bug_report", 1.0)])))
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    async fn jev_answering(answers: Value) -> MockServer {
        let server = MockServer::start().await;
        mount_jev_answers(&server, answers).await;
        server
    }

    async fn classify_against(
        server: &MockServer,
        trigger: ClassifiedTrigger,
    ) -> Result<Classification, ClassifyError> {
        Classifier::new(Arc::new(JevClient::new(
            "test-key".to_string(),
            server.uri(),
        )))
        .classify(trigger, &history(), &new_lines())
        .await
    }

    #[tokio::test]
    async fn direct_requests_omit_the_ignore_option() {
        let server = jev_expecting(request_with_options(&[
            "help_request",
            "bug_report",
            "feature_request",
            "chat",
        ]))
        .await;

        classify_against(&server, ClassifiedTrigger::Direct)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn unsolicited_requests_contain_all_options() {
        let server = jev_expecting(request_with_options(&[
            "help_request",
            "bug_report",
            "feature_request",
            "chat",
            "ignore",
        ]))
        .await;

        classify_against(&server, ClassifiedTrigger::Unsolicited)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn classify_maps_the_probabilities_to_categories() {
        let server = jev_answering(answer_with(&[
            ("help_request", 0.78),
            ("bug_report", 0.10),
            ("feature_request", 0.02),
            ("chat", 0.07),
            ("ignore", 0.03),
        ]))
        .await;

        let classification = classify_against(&server, ClassifiedTrigger::Unsolicited)
            .await
            .unwrap();

        assert_eq!(
            classification,
            Classification {
                probabilities: indexmap! {
                    Category::HelpRequest => 0.78,
                    Category::BugReport => 0.10,
                    Category::FeatureRequest => 0.02,
                    Category::Chat => 0.07,
                    Category::Ignore => 0.03,
                },
            }
        );
    }

    #[tokio::test]
    async fn classify_fails_on_an_unknown_category() {
        let server = jev_answering(answer_with(&[("help_request", 0.6), ("spam", 0.4)])).await;

        let error = classify_against(&server, ClassifiedTrigger::Unsolicited)
            .await
            .unwrap_err();

        assert!(
            matches!(&error, ClassifyError::UnknownCategory(name) if name == "spam"),
            "unexpected error: {error:?}"
        );
    }

    #[tokio::test]
    async fn classify_fails_without_probabilities() {
        let server = jev_answering(json!({
            "category": {
                "type": "choice",
                "choice": "help_request",
                "confidence": 0.0,
                "probabilities": {},
            }
        }))
        .await;

        let error = classify_against(&server, ClassifiedTrigger::Unsolicited)
            .await
            .unwrap_err();

        assert!(
            matches!(error, ClassifyError::NoProbabilities),
            "unexpected error: {error:?}"
        );
    }

    #[tokio::test]
    async fn classify_fails_without_a_category_answer() {
        let server = jev_answering(json!({})).await;

        let error = classify_against(&server, ClassifiedTrigger::Unsolicited)
            .await
            .unwrap_err();

        assert!(
            matches!(error, ClassifyError::MissingAnswer),
            "unexpected error: {error:?}"
        );
    }

    #[tokio::test]
    async fn classify_fails_on_a_category_answer_that_is_not_a_choice() {
        let server = jev_answering(json!({ "category": { "type": "noul", "noul": 0.9 } })).await;

        let error = classify_against(&server, ClassifiedTrigger::Unsolicited)
            .await
            .unwrap_err();

        assert!(
            matches!(error, ClassifyError::UnexpectedAnswer),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn top_returns_the_category_with_the_highest_probability() {
        let classification = Classification {
            probabilities: indexmap! {
                Category::HelpRequest => 0.10,
                Category::BugReport => 0.86,
                Category::Chat => 0.04,
            },
        };

        assert_eq!(classification.top(), (Category::BugReport, 0.86));
    }
}
