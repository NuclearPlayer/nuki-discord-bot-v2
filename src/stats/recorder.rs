use chrono::{DateTime, Utc};

use crate::stats::event::Event;
use crate::store::events::{EventStore, NewEvent};

/// The IDs that an event is about.
pub struct EventScope {
    pub run_id: Option<String>,
    pub guild_id: Option<u64>,
    pub channel_id: Option<u64>,
    pub user_id: Option<u64>,
}

/// Writes the usage events.
#[derive(Clone)]
pub struct Stats {
    events: EventStore,
}

impl Stats {
    pub fn new(events: EventStore) -> Self {
        Self { events }
    }

    /// Writes `event` at the current time. A failed write does not stop the bot, so it is only logged.
    pub async fn record(&self, scope: EventScope, event: Event) {
        let kind = event.kind();
        let new_event = NewEvent {
            at: Utc::now(),
            kind,
            run_id: scope.run_id,
            guild_id: scope.guild_id,
            channel_id: scope.channel_id,
            user_id: scope.user_id,
            data: event.data(),
        };
        if let Err(error) = self.events.append(new_event).await {
            tracing::warn!(kind, "could not record the usage event: {error}");
        }
    }

    pub fn run(&self, run_id: String, guild_id: u64, channel_id: u64) -> RunStats {
        RunStats {
            stats: self.clone(),
            run_id,
            guild_id,
            channel_id,
        }
    }
}

/// The recorder of one run: all of its events have the same run ID, server, and channel.
#[derive(Clone)]
pub struct RunStats {
    stats: Stats,
    run_id: String,
    guild_id: u64,
    channel_id: u64,
}

impl RunStats {
    pub async fn record(&self, user_id: Option<u64>, event: Event) {
        let scope = EventScope {
            run_id: Some(self.run_id.clone()),
            guild_id: Some(self.guild_id),
            channel_id: Some(self.channel_id),
            user_id,
        };
        self.stats.record(scope, event).await;
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }
}

/// The run ID of a debounced unit.
pub fn run_id_of_unit(last_message_id: u64) -> String {
    format!("unit-{last_message_id}")
}

/// The run ID of a completion of an inactive intake.
pub fn run_id_of_timeout(intake_id: i64, now: DateTime<Utc>) -> String {
    format!("timeout-{intake_id}-{}", now.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::*;
    use crate::stats::event::ReactionAdded;
    use crate::stats::test_support::test_run;
    use crate::store::test_support::{EventRow, all_events};

    #[tokio::test]
    async fn run_stats_record_the_scope_of_the_run() {
        let (store, run) = test_run().await;
        let before = Utc::now();

        run.record(
            Some(30),
            Event::ReactionAdded(ReactionAdded {
                message_id: 7,
                emoji: "👍".to_string(),
            }),
        )
        .await;

        let after = Utc::now();
        let events = all_events(&store).await;
        let at = events[0].at;
        // The time of the event is the current time, so the test can only bound it.
        assert!(before <= at && at <= after);
        assert_eq!(
            events,
            vec![EventRow {
                at,
                kind: "reaction.added".to_string(),
                run_id: Some("unit-1".to_string()),
                guild_id: Some(10),
                channel_id: Some(20),
                user_id: Some(30),
                data: json!({ "message_id": 7, "emoji": "👍" }),
            }]
        );
    }

    #[test]
    fn run_id_of_unit_is_unit_and_the_last_message_id() {
        assert_eq!(run_id_of_unit(1234), "unit-1234");
    }

    #[test]
    fn run_id_of_timeout_is_timeout_the_intake_id_and_the_time_in_ms() {
        let now = Utc.with_ymd_and_hms(2026, 10, 19, 12, 0, 0).unwrap();

        assert_eq!(run_id_of_timeout(5, now), "timeout-5-1792411200000");
    }
}
