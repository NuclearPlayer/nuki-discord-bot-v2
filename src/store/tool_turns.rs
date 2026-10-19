use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio_rusqlite::Connection;

use super::{Result, call, timestamp_text};

#[derive(Clone)]
pub struct ToolTurnStore {
    conn: Connection,
}

impl ToolTurnStore {
    pub(super) fn new(conn: Connection) -> Self {
        Self { conn }
    }

    pub async fn save(
        &self,
        channel_id: u64,
        message_id: u64,
        messages: &[Value],
        created_at: DateTime<Utc>,
    ) -> Result<()> {
        let json = serde_json::to_string(messages)?;
        call(&self.conn, move |c| {
            c.execute(
                "INSERT INTO tool_turns (channel_id, message_id, messages, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                (channel_id, message_id, json, timestamp_text(created_at)),
            )?;
            Ok(())
        })
        .await
    }

    pub async fn load(
        &self,
        channel_id: u64,
        message_ids: &[u64],
    ) -> Result<HashMap<u64, Vec<Value>>> {
        // The IDs go in as one JSON array parameter, so the query text does not depend on their count.
        let ids_json = serde_json::to_string(message_ids)?;
        call(&self.conn, move |c| {
            c.prepare(
                "SELECT message_id, messages FROM tool_turns
                 WHERE channel_id = ?1 AND message_id IN (SELECT value FROM json_each(?2))",
            )?
            .query_map((channel_id, ids_json), |row| {
                Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?))
            })?
            .map(|row| {
                let (message_id, json) = row?;
                Ok((message_id, serde_json::from_str(&json)?))
            })
            .collect()
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;
    use crate::store::Store;

    const CHANNEL: u64 = 1_100_000_000_000_000_000;

    fn created_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 19, 12, 0, 0).unwrap()
    }

    fn turn(call_id: &str) -> Vec<Value> {
        vec![
            json!({ "role": "assistant", "tool_calls": [{ "id": call_id, "name": "run_script" }] }),
            json!({ "role": "tool", "tool_call_id": call_id, "content": "{\"value\":42}" }),
        ]
    }

    async fn tool_turns() -> ToolTurnStore {
        Store::open_in_memory().await.unwrap().tool_turns()
    }

    #[tokio::test]
    async fn load_returns_the_saved_messages_of_each_requested_message() {
        let tool_turns = tool_turns().await;
        tool_turns
            .save(CHANNEL, 1, &turn("call-1"), created_at())
            .await
            .unwrap();
        tool_turns
            .save(CHANNEL, 2, &turn("call-2"), created_at())
            .await
            .unwrap();
        tool_turns
            .save(CHANNEL, 3, &turn("call-3"), created_at())
            .await
            .unwrap();

        assert_eq!(
            tool_turns.load(CHANNEL, &[1, 3]).await.unwrap(),
            HashMap::from([(1, turn("call-1")), (3, turn("call-3"))])
        );
    }

    #[tokio::test]
    async fn load_returns_no_entry_for_a_message_without_a_turn() {
        let tool_turns = tool_turns().await;
        tool_turns
            .save(CHANNEL, 1, &turn("call-1"), created_at())
            .await
            .unwrap();

        assert_eq!(
            tool_turns.load(CHANNEL, &[1, 2]).await.unwrap(),
            HashMap::from([(1, turn("call-1"))])
        );
    }

    #[tokio::test]
    async fn load_returns_no_entry_for_a_turn_of_a_different_channel() {
        let tool_turns = tool_turns().await;
        tool_turns
            .save(CHANNEL + 1, 1, &turn("call-1"), created_at())
            .await
            .unwrap();
        tool_turns
            .save(CHANNEL, 2, &turn("call-2"), created_at())
            .await
            .unwrap();

        assert_eq!(
            tool_turns.load(CHANNEL, &[1, 2]).await.unwrap(),
            HashMap::from([(2, turn("call-2"))])
        );
    }
}
