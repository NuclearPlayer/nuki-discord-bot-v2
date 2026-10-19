use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::stats::recorder::{RunStats, Stats};
use crate::store::Store;
use crate::store::test_support::all_events;

const TEST_RUN_ID: &str = "unit-1";
const TEST_GUILD_ID: u64 = 10;
const TEST_CHANNEL_ID: u64 = 20;

/// A `RunStats` on an in-memory store, with the run `unit-1`, the server 10, and the channel 20.
pub(crate) async fn test_run() -> (Store, RunStats) {
    let store = Store::open_in_memory().await.unwrap();
    let run =
        Stats::new(store.events()).run(TEST_RUN_ID.to_string(), TEST_GUILD_ID, TEST_CHANNEL_ID);
    (store, run)
}

/// A `RunStats` for a test that does not read the events.
pub(crate) async fn unrecorded_run() -> RunStats {
    test_run().await.1
}

/// An event row without its time and its `duration_ms`, which a test cannot know.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RunEvent {
    pub kind: String,
    pub run_id: Option<String>,
    pub guild_id: Option<u64>,
    pub channel_id: Option<u64>,
    pub user_id: Option<u64>,
    pub data: Value,
}

/// The event of `test_run` without a user, with `kind` and `data`.
pub(crate) fn run_event(kind: &str, data: Value) -> RunEvent {
    RunEvent {
        kind: kind.to_string(),
        run_id: Some(TEST_RUN_ID.to_string()),
        guild_id: Some(TEST_GUILD_ID),
        channel_id: Some(TEST_CHANNEL_ID),
        user_id: None,
        data,
    }
}

/// All events of `store`, sorted by ID. Asserts that each event is from between `before` and
/// `after`, and leaves out the time and the `duration_ms`.
pub(crate) async fn events_between(
    store: &Store,
    before: DateTime<Utc>,
    after: DateTime<Utc>,
) -> Vec<RunEvent> {
    all_events(store)
        .await
        .into_iter()
        .map(|row| {
            assert!(
                before <= row.at && row.at <= after,
                "the event is at {}, not between {before} and {after}",
                row.at
            );
            RunEvent {
                kind: row.kind,
                run_id: row.run_id,
                guild_id: row.guild_id,
                channel_id: row.channel_id,
                user_id: row.user_id,
                data: without_duration(row.data),
            }
        })
        .collect()
}

/// The events of `kind` in `events`.
pub(crate) fn of_kind(events: Vec<RunEvent>, kind: &str) -> Vec<RunEvent> {
    events
        .into_iter()
        .filter(|event| event.kind == kind)
        .collect()
}

/// The `duration_ms` of the only event of `kind` in `store`.
pub(crate) async fn duration_of(store: &Store, kind: &str) -> u64 {
    let rows: Vec<_> = all_events(store)
        .await
        .into_iter()
        .filter(|row| row.kind == kind)
        .collect();
    let [row] = rows.as_slice() else {
        panic!("expected one {kind} event, got {rows:?}");
    };
    row.data["duration_ms"].as_u64().unwrap()
}

fn without_duration(data: Value) -> Value {
    match data {
        Value::Object(mut fields) => {
            fields.remove("duration_ms");
            Value::Object(fields)
        }
        other => other,
    }
}
