use anyhow::bail;
use indexmap::IndexMap;
use serde_json::json;

use crate::decisions::client::JevClient;
use crate::decisions::types::{Answer, Question};
use crate::prompts::questions::{bug_fields, feature_fields};
use crate::stats::event::JevPurpose;
use crate::stats::recorder::RunStats;
use crate::store::intakes::IntakeKind;

/// A field is present only if Jev's probability of yes is above this value.
const PRESENT_ABOVE: f64 = 0.5;

/// Gets the names of the checklist fields that the conversation does not give, in the file's
/// sequence.
pub async fn missing_fields(
    jev: &JevClient,
    kind: IntakeKind,
    conversation: Vec<String>,
    stats: &RunStats,
) -> anyhow::Result<Vec<String>> {
    let fields = checklist(kind);
    let answers = jev
        .decide(
            json!({ "conversation": conversation }),
            noul_questions(fields),
            stats,
            JevPurpose::IntakeFields,
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
    use chrono::Utc;
    use serde_json::{Map, Value};
    use wiremock::MockServer;

    use super::*;
    use crate::decisions::client::JEV_MODEL;
    use crate::decisions::test_support::{
        jev_request_bodies, mount_jev_answers, without_instructions,
    };
    use crate::stats::test_support::{events_between, run_event, test_run, unrecorded_run};
    use crate::workflows::test_support::{
        bug_field_names, bug_fields_missing, missing_bug_field_names,
    };

    fn conversation() -> Vec<String> {
        vec![
            "bob: playback stops after one track".to_string(),
            "nuki: Which Nuclear version do you use?".to_string(),
        ]
    }

    async fn bug_fields_against(server: &MockServer, stats: &RunStats) -> Vec<String> {
        let jev = JevClient::new("jev-key".to_string(), server.uri());
        missing_fields(&jev, IntakeKind::Bug, conversation(), stats)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn missing_fields_returns_the_fields_at_or_below_one_half() {
        let server = MockServer::start().await;
        mount_jev_answers(&server, bug_fields_missing()).await;
        let (store, stats) = test_run().await;
        let before = Utc::now();

        let missing = bug_fields_against(&server, &stats).await;

        let after = Utc::now();
        assert_eq!(missing, missing_bug_field_names());
        assert_eq!(
            events_between(&store, before, after).await,
            vec![run_event(
                "jev.call",
                json!({
                    "purpose": "intake_fields",
                    "questions": bug_field_names(),
                    "answers": bug_fields_missing(),
                    "model": "typesafe/jev-1.14-20261014",
                    "input_tokens": null,
                    "output_tokens": null,
                    "cost_usd": null,
                    "error": null,
                })
            )]
        );
    }

    #[tokio::test]
    async fn missing_fields_sends_one_noul_question_for_each_field() {
        let server = MockServer::start().await;
        mount_jev_answers(&server, bug_fields_missing()).await;

        bug_fields_against(&server, &unrecorded_run().await).await;

        let questions: Map<String, Value> = bug_field_names()
            .into_iter()
            .map(|name| (name, json!({ "type": "noul" })))
            .collect();
        let requests: Vec<Value> = jev_request_bodies(&server)
            .await
            .into_iter()
            .map(without_instructions)
            .collect();
        assert_eq!(
            requests,
            [json!({
                "model": JEV_MODEL,
                "state": { "conversation": conversation() },
                "questions": questions
            })]
        );
    }
}
