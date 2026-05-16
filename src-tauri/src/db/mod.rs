use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{ConnectOptions, SqlitePool};
use tokio::sync::OnceCell;

#[derive(Clone)]
pub struct Db {
    path: PathBuf,
    pool: Arc<OnceCell<SqlitePool>>,
}

impl Db {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            pool: Arc::new(OnceCell::new()),
        }
    }

    pub async fn init(&self) -> Result<()> {
        let pool = self.pool().await?;
        sqlx::query(SCHEMA).execute(&*pool).await?;
        // Idempotent migration: add `thinking` column if the table predates it.
        // SQLite has no IF NOT EXISTS for ADD COLUMN, so we swallow the
        // duplicate-column error.
        let _ = sqlx::query("ALTER TABLE recordings ADD COLUMN thinking TEXT")
            .execute(&*pool)
            .await;

        // Sweep dead rows from prior sessions:
        //  - 'recording' or 'stopped' have no useful content and just
        //    clutter the sidebar. ('stopped' = capture finished but the
        //    user closed the app without sending; treat as discarded.)
        //  - 'canceled' rows DO persist across sessions on purpose: the
        //    user explicitly discarded those from the pill, but they
        //    asked to keep them visible in history.
        //  - 'processing' rows that never finished similarly aren't
        //    recoverable; if they have a body, downgrade to 'failed'
        //    so the user still sees what was generated; otherwise drop.
        sqlx::query(
            "DELETE FROM recordings
             WHERE status IN ('recording', 'stopped')
                OR (status = 'processing' AND COALESCE(body, '') = '')",
        )
        .execute(&*pool)
        .await?;
        sqlx::query(
            "UPDATE recordings SET status = 'failed', error = COALESCE(error, 'Interrupted')
             WHERE status = 'processing'",
        )
        .execute(&*pool)
        .await?;
        Ok(())
    }

    async fn pool(&self) -> Result<Arc<SqlitePool>> {
        let pool = self
            .pool
            .get_or_try_init(|| async {
                if let Some(parent) = self.path.parent() {
                    tokio::fs::create_dir_all(parent).await.ok();
                }
                let opts = SqliteConnectOptions::new()
                    .filename(&self.path)
                    .create_if_missing(true)
                    .foreign_keys(true)
                    .log_statements(tracing::log::LevelFilter::Trace);
                let pool = SqlitePoolOptions::new()
                    .max_connections(4)
                    .connect_with(opts)
                    .await?;
                Result::<_, anyhow::Error>::Ok(pool)
            })
            .await?;
        Ok(Arc::new(pool.clone()))
    }

    pub async fn insert_recording(&self, rec: &Recording) -> Result<()> {
        let pool = self.pool().await?;
        sqlx::query(
            r#"INSERT INTO recordings (id, created_at, duration_ms, video_path, status, summary, body, transcript, thinking, error)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&rec.id)
        .bind(rec.created_at)
        .bind(rec.duration_ms as i64)
        .bind(&rec.video_path)
        .bind(rec.status.as_str())
        .bind(&rec.summary)
        .bind(&rec.body)
        .bind(&rec.transcript)
        .bind(&rec.thinking)
        .bind(&rec.error)
        .execute(&*pool)
        .await?;
        Ok(())
    }

    pub async fn update_recording(&self, rec: &Recording) -> Result<()> {
        let pool = self.pool().await?;
        sqlx::query(
            r#"UPDATE recordings
               SET duration_ms = ?, status = ?, summary = ?, body = ?, transcript = ?, thinking = ?, error = ?
               WHERE id = ?"#,
        )
        .bind(rec.duration_ms as i64)
        .bind(rec.status.as_str())
        .bind(&rec.summary)
        .bind(&rec.body)
        .bind(&rec.transcript)
        .bind(&rec.thinking)
        .bind(&rec.error)
        .bind(&rec.id)
        .execute(&*pool)
        .await?;
        Ok(())
    }

    pub async fn list_recordings(&self, limit: i64) -> Result<Vec<Recording>> {
        let pool = self.pool().await?;
        let rows = sqlx::query_as::<_, RecordingRow>(
            r#"SELECT id, created_at, duration_ms, video_path, status, summary, body, transcript, thinking, error
               FROM recordings
               ORDER BY created_at DESC
               LIMIT ?"#,
        )
        .bind(limit)
        .fetch_all(&*pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get_recording(&self, id: &str) -> Result<Option<Recording>> {
        let pool = self.pool().await?;
        let row = sqlx::query_as::<_, RecordingRow>(
            r#"SELECT id, created_at, duration_ms, video_path, status, summary, body, transcript, thinking, error
               FROM recordings WHERE id = ?"#,
        )
        .bind(id)
        .fetch_optional(&*pool)
        .await?;
        Ok(row.map(Into::into))
    }

    pub async fn delete_recording(&self, id: &str) -> Result<()> {
        let pool = self.pool().await?;
        sqlx::query("DELETE FROM recordings WHERE id = ?")
            .bind(id)
            .execute(&*pool)
            .await?;
        Ok(())
    }
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS recordings (
    id           TEXT PRIMARY KEY,
    created_at   TEXT NOT NULL,
    duration_ms  INTEGER NOT NULL DEFAULT 0,
    video_path   TEXT NOT NULL,
    status       TEXT NOT NULL,
    summary      TEXT,
    body         TEXT,
    transcript   TEXT,
    thinking     TEXT,
    error        TEXT
);
CREATE INDEX IF NOT EXISTS recordings_created_idx ON recordings(created_at DESC);
"#;

#[derive(sqlx::FromRow)]
struct RecordingRow {
    id: String,
    created_at: DateTime<Utc>,
    duration_ms: i64,
    video_path: String,
    status: String,
    summary: Option<String>,
    body: Option<String>,
    transcript: Option<String>,
    thinking: Option<String>,
    error: Option<String>,
}

impl From<RecordingRow> for Recording {
    fn from(r: RecordingRow) -> Self {
        Recording {
            id: r.id,
            created_at: r.created_at,
            duration_ms: r.duration_ms.max(0) as u64,
            video_path: r.video_path,
            status: RecordingStatus::from_str(&r.status),
            summary: r.summary,
            body: r.body,
            transcript: r.transcript,
            thinking: r.thinking,
            error: r.error,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recording {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub video_path: String,
    pub status: RecordingStatus,
    pub summary: Option<String>,
    pub body: Option<String>,
    pub transcript: Option<String>,
    /// Markdown rendering of the per-window observations (what the model
    /// "saw" in each segment) — populated by the analyzer for the
    /// "Show thinking" panel. None for older rows.
    pub thinking: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RecordingStatus {
    Recording,
    /// Capture has stopped; the user hasn't yet pressed send or cancel on
    /// the pill. The pipeline has not run.
    Stopped,
    Processing,
    Done,
    Failed,
    Canceled,
}

impl RecordingStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Stopped => "stopped",
            Self::Processing => "processing",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s {
            "recording" => Self::Recording,
            "stopped" => Self::Stopped,
            "processing" => Self::Processing,
            "done" => Self::Done,
            "canceled" => Self::Canceled,
            _ => Self::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Db, Recording, RecordingStatus};
    use chrono::Utc;
    use tempfile::tempdir;

    fn rec(id: &str, status: RecordingStatus) -> Recording {
        Recording {
            id: id.to_string(),
            created_at: Utc::now(),
            duration_ms: 1234,
            video_path: format!("/tmp/{id}.mp4"),
            status,
            summary: Some(format!("{id} summary")),
            body: None,
            transcript: None,
            thinking: None,
            error: None,
        }
    }

    #[tokio::test]
    async fn insert_list_get_update_and_delete_recordings() {
        let dir = tempdir().unwrap();
        let db = Db::new(dir.path().join("peer.db"));
        db.init().await.unwrap();

        let mut row = rec("one", RecordingStatus::Done);
        db.insert_recording(&row).await.unwrap();

        let listed = db.list_recordings(10).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "one");

        row.body = Some("Generated prompt".into());
        row.thinking = Some("Observed details".into());
        db.update_recording(&row).await.unwrap();

        let fetched = db.get_recording("one").await.unwrap().unwrap();
        assert_eq!(fetched.body.as_deref(), Some("Generated prompt"));
        assert_eq!(fetched.thinking.as_deref(), Some("Observed details"));

        db.delete_recording("one").await.unwrap();
        assert!(db.get_recording("one").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn init_sweeps_unrecoverable_in_flight_rows() {
        let dir = tempdir().unwrap();
        let db = Db::new(dir.path().join("peer.db"));
        db.init().await.unwrap();

        db.insert_recording(&rec("recording", RecordingStatus::Recording))
            .await
            .unwrap();
        db.insert_recording(&rec("stopped", RecordingStatus::Stopped))
            .await
            .unwrap();
        db.insert_recording(&rec("empty-processing", RecordingStatus::Processing))
            .await
            .unwrap();
        let mut partial = rec("partial-processing", RecordingStatus::Processing);
        partial.body = Some("Partial prompt".into());
        db.insert_recording(&partial).await.unwrap();
        db.insert_recording(&rec("canceled", RecordingStatus::Canceled))
            .await
            .unwrap();

        db.init().await.unwrap();

        assert!(db.get_recording("recording").await.unwrap().is_none());
        assert!(db.get_recording("stopped").await.unwrap().is_none());
        assert!(db
            .get_recording("empty-processing")
            .await
            .unwrap()
            .is_none());

        let partial = db
            .get_recording("partial-processing")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(partial.status, RecordingStatus::Failed);
        assert_eq!(partial.error.as_deref(), Some("Interrupted"));

        let canceled = db.get_recording("canceled").await.unwrap().unwrap();
        assert_eq!(canceled.status, RecordingStatus::Canceled);
    }
}
