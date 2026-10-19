use anyhow::bail;
use indexmap::IndexMap;
use serde_json::json;

use crate::decisions::client::JevClient;
use crate::decisions::types::{Answer, Question};
use crate::prompts::questions::{bug_fields, feature_fields};
use crate::store::intakes::IntakeKind;

/// A field is present only if Jev gives a probability of yes above this value.
const PRESENT_ABOVE: f64 = 0.5;

/// The names of the checklist fields that the conversation does not give, in the sequence of the file.
pub async fn missing_fields(
    jev: &JevClient,
    kind: IntakeKind,
    conversation: Vec<String>,
) -> anyhow::Result<Vec<String>> {
    let fields = checklist(kind);
    let answers = jev
        .decide(
            json!({ "conversation": conversation }),
            noul_questions(fields),
        )
        .await?;
    let probabilities = fields
        .keys()
        .map(|name| Ok((name, probability_of_yes(&answers, name)?)))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(probabilities
        .into_iter()
        .filter(|(_, probability)| *probability <= PRESENT_ABOVE)
        .map(|(name, _)| name.clone())
        .collect())
}

fn checklist(kind: IntakeKind) -> &'static IndexMap<String, String> {
    match kind {
        IntakeKind::Bug => bug_fields(),
        IntakeKind::Feature => feature_fields(),
    }
}

fn noul_questions(fields: &IndexMap<String, String>) -> IndexMap<String, Question> {
    fields
        .iter()
        .map(|(name, text)| {
            let question = Question::Noul {
                instructions: text.clone(),
            };
            (name.clone(), question)
        })
        .collect()
}

fn probability_of_yes(answers: &IndexMap<String, Answer>, field: &str) -> anyhow::Result<f64> {
    match answers.get(field) {
        Some(Answer::Noul { noul }) => Ok(*noul),
        Some(other) => bail!("Jev gave a non-noul answer for the field {field}: {other:?}"),
        None => bail!("Jev gave no answer for the field {field}"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use wiremock::MockServer;

    use super::*;
    use crate::decisions::test_support::{jev_request_bodies, mount_jev_answers};
    use crate::workflows::test_support::bug_fields_missing;

    fn conversation() -> Vec<String> {
        vec![
            "bob: playback stops after one track".to_string(),
            "nuki: Which Nuclear version do you use?".to_string(),
        ]
    }

    async fn bug_fields_against(server: &MockServer) -> Vec<String> {
        let jev = JevClient::new("jev-key".to_string(), server.uri());
        missing_fields(&jev, IntakeKind::Bug, conversation())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn missing_fields_returns_the_fields_at_or_below_one_half() {
        let server = MockServer::start().await;
        mount_jev_answers(&server, bug_fields_missing()).await;

        let missing = bug_fields_against(&server).await;

        assert_eq!(missing, ["platform", "plugins"]);
    }

    #[tokio::test]
    async fn missing_fields_sends_one_noul_question_for_each_field() {
        let server = MockServer::start().await;
        mount_jev_answers(&server, bug_fields_missing()).await;

        bug_fields_against(&server).await;

        let expected: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/workflows/intake_bug_fields_request.json"
        ))
        .unwrap();
        assert_eq!(jev_request_bodies(&server).await, [expected]);
    }
}
