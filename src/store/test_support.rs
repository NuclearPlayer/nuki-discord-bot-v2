use chrono::{DateTime, Utc};
use rusqlite::Row;
use rusqlite::types::Type;

use crate::store::{Store, call};

#[derive(Debug, PartialEq)]
pub(crate) struct EventRow {
    pub at: DateTime<Utc>,
    pub kind: String,
    pub run_id: Option<String>,
    pub guild_id: Option<u64>,
    pub channel_id: Option<u64>,
    pub user_id: Option<u64>,
    pub data: serde_json::Value,
}

/// Returns all rows of `events`, sorted by `id`.
pub(crate) async fn all_events(store: &Store) -> Vec<EventRow> {
    let rows = call(&store.conn, |c| {
        let mut statement = c.prepare(
            "SELECT at, kind, run_id, guild_id, channel_id, user_id, data
             FROM events ORDER BY id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    timestamp_column(row, 0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .await
    .unwrap();
    rows.into_iter()
        .map(
            |(at, kind, run_id, guild_id, channel_id, user_id, data)| EventRow {
                at,
                kind,
                run_id,
                guild_id,
                channel_id,
                user_id,
                data: serde_json::from_str(&data).unwrap(),
            },
        )
        .collect()
}

fn timestamp_column(row: &Row, index: usize) -> rusqlite::Result<DateTime<Utc>> {
    let text: String = row.get(index)?;
    DateTime::parse_from_rfc3339(&text)
        .map(|at| at.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
        })
}
