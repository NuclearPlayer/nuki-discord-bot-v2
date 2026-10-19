//! The `workflow.finished` event of a workflow run.

use std::future::Future;
use std::time::Instant;

use super::Outcome;
use crate::stats::duration_ms;
use crate::stats::event::{Event, WorkflowFinished, WorkflowOutcome};
use crate::stats::recorder::RunStats;

/// Runs the workflow `name` and records its `workflow.finished` event in the run of `stats`. The
/// `outcome` of the event is what the workflow returned; `reply.sent` records what Discord got.
/// A failed run records the outcome `error` with the error chain, and then returns the error.
pub async fn timed(
    stats: &RunStats,
    name: &str,
    run: impl Future<Output = anyhow::Result<Outcome>>,
) -> anyhow::Result<Outcome> {
    let started = Instant::now();
    let result = run.await;
    let elapsed = started.elapsed();
    let (outcome, error) = match &result {
        Ok(outcome) => (WorkflowOutcome::from(outcome), None),
        Err(error) => (WorkflowOutcome::Error, Some(format!("{error:#}"))),
    };
    let finished = WorkflowFinished {
        workflow: name.to_string(),
        outcome,
        duration_ms: duration_ms(elapsed),
        error,
    };
    stats.record(None, Event::WorkflowFinished(finished)).await;
    result
}

impl From<&Outcome> for WorkflowOutcome {
    fn from(outcome: &Outcome) -> WorkflowOutcome {
        match outcome {
            Outcome::Reply(_) => WorkflowOutcome::Reply,
            Outcome::Nothing => WorkflowOutcome::Nothing,
            Outcome::Intake { .. } => WorkflowOutcome::Intake,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use anyhow::{Context, anyhow};
    use chrono::Utc;
    use serde_json::json;

    use super::*;
    use crate::stats::test_support::{duration_of, events_between, run_event, test_run};
    use crate::workflows::Reply;

    fn reply() -> Outcome {
        Outcome::Reply(Reply {
            text: "Open Settings.".to_string(),
            turn_messages: vec![],
        })
    }

    #[tokio::test]
    async fn workflow_records_the_outcome() {
        let (store, run) = test_run().await;
        let before = Utc::now();

        let outcome = timed(&run, "chat", async { Ok(reply()) }).await.unwrap();

        let after = Utc::now();
        assert_eq!(outcome, reply());
        assert_eq!(
            events_between(&store, before, after).await,
            vec![run_event(
                "workflow.finished",
                json!({ "workflow": "chat", "outcome": "reply", "error": null }),
            )]
        );
    }

    #[tokio::test]
    async fn workflow_records_the_duration_of_the_run() {
        let (store, run) = test_run().await;

        timed(&run, "chat", async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(reply())
        })
        .await
        .unwrap();

        assert!(duration_of(&store, "workflow.finished").await >= 50);
    }

    #[tokio::test]
    async fn workflow_records_an_error_outcome() {
        let (store, run) = test_run().await;
        let before = Utc::now();

        let error = timed(&run, "help_request", async {
            Err(anyhow!("connection refused")).context("the Jev request failed")
        })
        .await
        .unwrap_err();

        let after = Utc::now();
        assert_eq!(
            format!("{error:#}"),
            "the Jev request failed: connection refused"
        );
        assert_eq!(
            events_between(&store, before, after).await,
            vec![run_event(
                "workflow.finished",
                json!({
                    "workflow": "help_request",
                    "outcome": "error",
                    "error": "the Jev request failed: connection refused",
                }),
            )]
        );
    }
}
