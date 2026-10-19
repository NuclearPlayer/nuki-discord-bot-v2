use chrono::{DateTime, Utc};
use tokio_rusqlite::Connection;

use super::{Result, call, timestamp_text};

pub struct NewEvent {
    pub at: DateTime<Utc>,
    pub kind: &'static str,
    pub run_id: Option<String>,
    pub guild_id: Option<u64>,
    pub channel_id: Option<u64>,
    pub user_id: Option<u64>,
    pub data: serde_json::Value,
}

#[derive(Clone)]
pub struct EventStore {
    conn: Connection,
}

impl EventStore {
    pub(super) fn new(conn: Connection) -> Self {
        Self { conn }
    }

    pub async fn append(&self, event: NewEvent) -> Result<()> {
        let at = timestamp_text(event.at);
        let data = event.data.to_string();
        call(&self.conn, move |c| {
            c.execute(
                "INSERT INTO events (at, kind, run_id, guild_id, channel_id, user_id, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                (
                    at,
                    event.kind,
                    event.run_id,
                    event.guild_id,
                    event.channel_id,
                    event.user_id,
                    data,
                ),
            )?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::NewEvent;
    use crate::store::Store;
    use crate::store::test_support::{EventRow, all_events};

    #[tokio::test]
    async fn append_writes_all_columns() {
        let store = Store::open_in_memory().await.unwrap();
        let at = Utc.with_ymd_and_hms(2026, 10, 19, 12, 30, 45).unwrap();

        store
            .events()
            .append(NewEvent {
                at,
                kind: "reaction.added",
                run_id: Some("unit-7".to_string()),
                guild_id: Some(10),
                channel_id: Some(20),
                user_id: Some(30),
                data: json!({ "message_id": 7, "emoji": "👍" }),
            })
            .await
            .unwrap();

        assert_eq!(
            all_events(&store).await,
            vec![EventRow {
                at,
                kind: "reaction.added".to_string(),
                run_id: Some("unit-7".to_string()),
                guild_id: Some(10),
                channel_id: Some(20),
                user_id: Some(30),
                data: json!({ "message_id": 7, "emoji": "👍" }),
            }]
        );
    }

    #[tokio::test]
    async fn append_writes_null_for_absent_ids() {
        let store = Store::open_in_memory().await.unwrap();
        let at = Utc.with_ymd_and_hms(2026, 10, 19, 8, 0, 0).unwrap();

        store
            .events()
            .append(NewEvent {
                at,
                kind: "model.changed",
                run_id: None,
                guild_id: None,
                channel_id: None,
                user_id: None,
                data: json!({ "previous": "a/b", "new": "c/d" }),
            })
            .await
            .unwrap();

        assert_eq!(
            all_events(&store).await,
            vec![EventRow {
                at,
                kind: "model.changed".to_string(),
                run_id: None,
                guild_id: None,
                channel_id: None,
                user_id: None,
                data: json!({ "previous": "a/b", "new": "c/d" }),
            }]
        );
    }
}
