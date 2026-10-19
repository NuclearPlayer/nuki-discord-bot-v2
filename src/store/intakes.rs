use chrono::{DateTime, Utc};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef};
use rusqlite::{OptionalExtension, Row, ToSql};
use tokio_rusqlite::Connection;

use super::{Result, call, timestamp_column, timestamp_text};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IntakeKind {
    Bug,
    Feature,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IntakeStatus {
    Open,
    Reported,
}

#[derive(Clone, PartialEq, Debug)]
pub struct NewIntake {
    pub kind: IntakeKind,
    pub thread_id: u64,
    pub origin_channel_id: u64,
    pub origin_message_id: u64,
    pub reporter_id: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Intake {
    pub id: i64,
    pub kind: IntakeKind,
    pub thread_id: u64,
    pub origin_channel_id: u64,
    pub origin_message_id: u64,
    pub reporter_id: u64,
    pub status: IntakeStatus,
    pub last_activity_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct IntakeStore {
    conn: Connection,
}

impl IntakeStore {
    pub(super) fn new(conn: Connection) -> Self {
        Self { conn }
    }

    pub async fn create(&self, new: NewIntake) -> Result<Intake> {
        call(&self.conn, move |c| {
            c.execute(
                "INSERT INTO intakes (kind, thread_id, origin_channel_id, origin_message_id,
                     reporter_id, status, last_activity_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                (
                    new.kind,
                    new.thread_id,
                    new.origin_channel_id,
                    new.origin_message_id,
                    new.reporter_id,
                    IntakeStatus::Open,
                    timestamp_text(new.created_at),
                ),
            )?;
            Ok(Intake {
                id: c.last_insert_rowid(),
                kind: new.kind,
                thread_id: new.thread_id,
                origin_channel_id: new.origin_channel_id,
                origin_message_id: new.origin_message_id,
                reporter_id: new.reporter_id,
                status: IntakeStatus::Open,
                last_activity_at: new.created_at,
                created_at: new.created_at,
            })
        })
        .await
    }

    pub async fn find_open_by_thread(&self, thread_id: u64) -> Result<Option<Intake>> {
        call(&self.conn, move |c| {
            let intake = c
                .query_row(
                    &format!("SELECT {COLUMNS} FROM intakes WHERE thread_id = ?1 AND status = ?2"),
                    (thread_id, IntakeStatus::Open),
                    intake_from_row,
                )
                .optional()?;
            Ok(intake)
        })
        .await
    }

    pub async fn touch(&self, id: i64, at: DateTime<Utc>) -> Result<()> {
        call(&self.conn, move |c| {
            c.execute(
                "UPDATE intakes SET last_activity_at = ?2 WHERE id = ?1",
                (id, timestamp_text(at)),
            )?;
            Ok(())
        })
        .await
    }

    pub async fn mark_reported(&self, id: i64) -> Result<()> {
        call(&self.conn, move |c| {
            c.execute(
                "UPDATE intakes SET status = ?2 WHERE id = ?1",
                (id, IntakeStatus::Reported),
            )?;
            Ok(())
        })
        .await
    }

    pub async fn open_inactive_since(&self, cutoff: DateTime<Utc>) -> Result<Vec<Intake>> {
        call(&self.conn, move |c| {
            let intakes = c
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM intakes
                     WHERE status = ?1 AND last_activity_at < ?2
                     ORDER BY id"
                ))?
                .query_map(
                    (IntakeStatus::Open, timestamp_text(cutoff)),
                    intake_from_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(intakes)
        })
        .await
    }
}

const COLUMNS: &str = "id, kind, thread_id, origin_channel_id, origin_message_id, reporter_id, \
                       status, last_activity_at, created_at";

fn intake_from_row(row: &Row) -> rusqlite::Result<Intake> {
    Ok(Intake {
        id: row.get(0)?,
        kind: row.get(1)?,
        thread_id: row.get(2)?,
        origin_channel_id: row.get(3)?,
        origin_message_id: row.get(4)?,
        reporter_id: row.get(5)?,
        status: row.get(6)?,
        last_activity_at: timestamp_column(row, 7)?,
        created_at: timestamp_column(row, 8)?,
    })
}

impl IntakeKind {
    fn as_str(self) -> &'static str {
        match self {
            IntakeKind::Bug => "bug",
            IntakeKind::Feature => "feature",
        }
    }
}

impl ToSql for IntakeKind {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl FromSql for IntakeKind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "bug" => Ok(IntakeKind::Bug),
            "feature" => Ok(IntakeKind::Feature),
            other => Err(FromSqlError::Other(
                format!("unknown intake kind: {other}").into(),
            )),
        }
    }
}

impl IntakeStatus {
    fn as_str(self) -> &'static str {
        match self {
            IntakeStatus::Open => "open",
            IntakeStatus::Reported => "reported",
        }
    }
}

impl ToSql for IntakeStatus {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl FromSql for IntakeStatus {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "open" => Ok(IntakeStatus::Open),
            "reported" => Ok(IntakeStatus::Reported),
            other => Err(FromSqlError::Other(
                format!("unknown intake status: {other}").into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::store::Store;

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 19, hour, 0, 0).unwrap()
    }

    fn new_intake(kind: IntakeKind, thread_id: u64, created_at: DateTime<Utc>) -> NewIntake {
        NewIntake {
            kind,
            thread_id,
            origin_channel_id: 1_200_000_000_000_000_001,
            origin_message_id: 1_200_000_000_000_000_002,
            reporter_id: 1_200_000_000_000_000_003,
            created_at,
        }
    }

    async fn intakes() -> IntakeStore {
        Store::open_in_memory().await.unwrap().intakes()
    }

    #[tokio::test]
    async fn create_returns_an_open_intake_with_the_creation_time_as_last_activity() {
        let intakes = intakes().await;

        let intake = intakes
            .create(new_intake(
                IntakeKind::Bug,
                1_300_000_000_000_000_000,
                at(9),
            ))
            .await
            .unwrap();

        assert_eq!(
            intake,
            Intake {
                id: 1,
                kind: IntakeKind::Bug,
                thread_id: 1_300_000_000_000_000_000,
                origin_channel_id: 1_200_000_000_000_000_001,
                origin_message_id: 1_200_000_000_000_000_002,
                reporter_id: 1_200_000_000_000_000_003,
                status: IntakeStatus::Open,
                last_activity_at: at(9),
                created_at: at(9),
            }
        );
    }

    #[tokio::test]
    async fn find_open_by_thread_returns_the_open_intake_of_the_thread() {
        let intakes = intakes().await;
        intakes
            .create(new_intake(IntakeKind::Bug, 10, at(8)))
            .await
            .unwrap();
        let created = intakes
            .create(new_intake(IntakeKind::Feature, 11, at(9)))
            .await
            .unwrap();

        assert_eq!(
            intakes.find_open_by_thread(11).await.unwrap(),
            Some(created)
        );
    }

    #[tokio::test]
    async fn find_open_by_thread_returns_none_after_the_intake_is_reported() {
        let intakes = intakes().await;
        let reported = intakes
            .create(new_intake(IntakeKind::Bug, 10, at(8)))
            .await
            .unwrap();
        let still_open = intakes
            .create(new_intake(IntakeKind::Feature, 11, at(9)))
            .await
            .unwrap();

        intakes.mark_reported(reported.id).await.unwrap();

        assert_eq!(
            (
                intakes.find_open_by_thread(10).await.unwrap(),
                intakes.find_open_by_thread(11).await.unwrap(),
            ),
            (None, Some(still_open))
        );
    }

    #[tokio::test]
    async fn touch_sets_the_last_activity() {
        let intakes = intakes().await;
        let created = intakes
            .create(new_intake(IntakeKind::Bug, 10, at(8)))
            .await
            .unwrap();

        intakes.touch(created.id, at(11)).await.unwrap();

        assert_eq!(
            intakes.find_open_by_thread(10).await.unwrap(),
            Some(Intake {
                last_activity_at: at(11),
                ..created
            })
        );
    }

    #[tokio::test]
    async fn open_inactive_since_returns_only_open_intakes_older_than_the_cutoff() {
        let intakes = intakes().await;
        let first_old = intakes
            .create(new_intake(IntakeKind::Bug, 10, at(8)))
            .await
            .unwrap();
        let reported = intakes
            .create(new_intake(IntakeKind::Bug, 11, at(8)))
            .await
            .unwrap();
        intakes
            .create(new_intake(IntakeKind::Feature, 12, at(12)))
            .await
            .unwrap();
        let second_old = intakes
            .create(new_intake(IntakeKind::Feature, 13, at(7)))
            .await
            .unwrap();
        intakes.mark_reported(reported.id).await.unwrap();

        assert_eq!(
            intakes.open_inactive_since(at(10)).await.unwrap(),
            vec![first_old, second_old]
        );
    }
}
