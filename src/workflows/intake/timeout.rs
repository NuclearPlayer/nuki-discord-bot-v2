use chrono::{DateTime, Duration, Utc};

use crate::store::Result;
use crate::store::intakes::{Intake, IntakeStore};

/// Gets the open intakes with no message from the reporter for `timeout` before `now`.
pub async fn due_intakes(
    store: &IntakeStore,
    now: DateTime<Utc>,
    timeout: Duration,
) -> Result<Vec<Intake>> {
    store.open_inactive_since(now - timeout).await
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::store::Store;
    use crate::store::intakes::{IntakeKind, NewIntake};

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 19, hour, 0, 0).unwrap()
    }

    fn new_intake(thread_id: u64, created_at: DateTime<Utc>) -> NewIntake {
        NewIntake {
            kind: IntakeKind::Bug,
            thread_id,
            origin_channel_id: 1200000000000000001,
            origin_message_id: 1200000000000000002,
            reporter_id: 1300000000000000002,
            created_at,
        }
    }

    #[tokio::test]
    async fn due_intakes_returns_open_intakes_older_than_the_timeout() {
        let store = Store::open_in_memory().await.unwrap().intakes();
        let due = store.create(new_intake(10, at(9))).await.unwrap();
        store.create(new_intake(11, at(11))).await.unwrap();
        let touched = store.create(new_intake(13, at(8))).await.unwrap();
        store.touch(touched.id, at(12)).await.unwrap();
        let reported = store.create(new_intake(12, at(8))).await.unwrap();
        store.mark_reported(reported.id).await.unwrap();

        let intakes = due_intakes(&store, at(20), Duration::hours(10))
            .await
            .unwrap();

        assert_eq!(intakes, vec![due]);
    }
}
