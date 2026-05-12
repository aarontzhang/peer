use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{ConnectOptions, SqlitePool};
use tokio::sync::OnceCell;
use uuid::Uuid;

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

        // Backfill: synthesize a v1 'initial' row for every existing recording
        // with a body but no version history. Lets old recordings show up in
        // the new history side-panel without losing their original prompt.
        sqlx::query(
            "INSERT INTO recording_versions
               (id, recording_id, created_at, version_no, source, body, thinking,
                source_message_id, reverted_from_version_id)
             SELECT lower(hex(randomblob(16))), r.id, r.created_at, 1, 'initial',
                    r.body, r.thinking, NULL, NULL
             FROM recordings r
             WHERE COALESCE(r.body, '') <> ''
               AND NOT EXISTS (
                 SELECT 1 FROM recording_versions v WHERE v.recording_id = r.id
               )",
        )
        .execute(&*pool)
        .await?;

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

    /// Update mutable recording fields. **Do not** use this to write a fresh
    /// body coming from the pipeline or chat — that has to go through
    /// `append_version` so the version timeline stays consistent. Callers that
    /// only flip status, summary, transcript, thinking, or error are fine to
    /// use this directly.
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

    /// Atomically writes a new `recording_versions` row AND updates the
    /// recording's `body` (+ `thinking`, when supplied) pointer. Every
    /// codepath that mutates `recordings.body` must go through here so the
    /// timeline never has gaps.
    pub async fn append_version(
        &self,
        recording_id: &str,
        source: VersionSource,
        body: &str,
        thinking: Option<&str>,
        source_message_id: Option<&str>,
        reverted_from_version_id: Option<&str>,
    ) -> Result<RecordingVersion> {
        let pool = self.pool().await?;
        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now();
        let mut tx = pool.begin().await?;

        let next_version_no: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version_no), 0) + 1 FROM recording_versions WHERE recording_id = ?",
        )
        .bind(recording_id)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query(
            r#"INSERT INTO recording_versions
                 (id, recording_id, created_at, version_no, source, body, thinking,
                  source_message_id, reverted_from_version_id)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&id)
        .bind(recording_id)
        .bind(created_at)
        .bind(next_version_no)
        .bind(source.as_str())
        .bind(body)
        .bind(thinking)
        .bind(source_message_id)
        .bind(reverted_from_version_id)
        .execute(&mut *tx)
        .await?;

        // Update body always; only overwrite thinking if a fresh one is
        // supplied (chat edits don't have new thinking, so they leave the
        // initial vision pass intact).
        if let Some(thinking_text) = thinking {
            sqlx::query("UPDATE recordings SET body = ?, thinking = ? WHERE id = ?")
                .bind(body)
                .bind(thinking_text)
                .bind(recording_id)
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query("UPDATE recordings SET body = ? WHERE id = ?")
                .bind(body)
                .bind(recording_id)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        Ok(RecordingVersion {
            id,
            recording_id: recording_id.to_string(),
            created_at,
            version_no: next_version_no,
            source,
            body: body.to_string(),
            thinking: thinking.map(str::to_string),
            source_message_id: source_message_id.map(str::to_string),
            source_message_content: None,
            reverted_from_version_id: reverted_from_version_id.map(str::to_string),
        })
    }

    pub async fn list_versions(&self, recording_id: &str) -> Result<Vec<RecordingVersion>> {
        let pool = self.pool().await?;
        // Newest first. LEFT JOIN pulls in the user message that drove a
        // chat-sourced version so the panel can preview it inline.
        let rows = sqlx::query_as::<_, RecordingVersionRow>(
            r#"SELECT v.id, v.recording_id, v.created_at, v.version_no, v.source,
                      v.body, v.thinking, v.source_message_id, m.content AS source_message_content,
                      v.reverted_from_version_id
               FROM recording_versions v
               LEFT JOIN recording_messages m ON m.id = v.source_message_id
               WHERE v.recording_id = ?
               ORDER BY v.version_no DESC"#,
        )
        .bind(recording_id)
        .fetch_all(&*pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get_version(&self, version_id: &str) -> Result<Option<RecordingVersion>> {
        let pool = self.pool().await?;
        let row = sqlx::query_as::<_, RecordingVersionRow>(
            r#"SELECT v.id, v.recording_id, v.created_at, v.version_no, v.source,
                      v.body, v.thinking, v.source_message_id, m.content AS source_message_content,
                      v.reverted_from_version_id
               FROM recording_versions v
               LEFT JOIN recording_messages m ON m.id = v.source_message_id
               WHERE v.id = ?"#,
        )
        .bind(version_id)
        .fetch_optional(&*pool)
        .await?;
        Ok(row.map(Into::into))
    }

    pub async fn get_chat_thread(&self, recording_id: &str) -> Result<Vec<RecordingMessage>> {
        let pool = self.pool().await?;
        let rows = sqlx::query_as::<_, RecordingMessageRow>(
            r#"SELECT id, recording_id, created_at, turn_index, role, content, produced_version_id
               FROM recording_messages
               WHERE recording_id = ?
               ORDER BY turn_index ASC"#,
        )
        .bind(recording_id)
        .fetch_all(&*pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Append a user-role message to the chat thread for a recording.
    /// Returns the new row; the caller will pair it with an assistant
    /// message and a version once the backend stream completes.
    pub async fn insert_user_message(
        &self,
        recording_id: &str,
        content: &str,
    ) -> Result<RecordingMessage> {
        let pool = self.pool().await?;
        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now();
        let mut tx = pool.begin().await?;
        let turn_index: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(turn_index), -1) + 1 FROM recording_messages WHERE recording_id = ?",
        )
        .bind(recording_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            r#"INSERT INTO recording_messages
                 (id, recording_id, created_at, turn_index, role, content, produced_version_id)
               VALUES (?, ?, ?, ?, 'user', ?, NULL)"#,
        )
        .bind(&id)
        .bind(recording_id)
        .bind(created_at)
        .bind(turn_index)
        .bind(content)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(RecordingMessage {
            id,
            recording_id: recording_id.to_string(),
            created_at,
            turn_index,
            role: ChatRole::User,
            content: content.to_string(),
            produced_version_id: None,
        })
    }

    /// Append an assistant-role message and link it to the version it
    /// produced. Called once the chat stream finishes and the new prompt
    /// body has been versioned.
    pub async fn insert_assistant_message(
        &self,
        recording_id: &str,
        content: &str,
        produced_version_id: Option<&str>,
    ) -> Result<RecordingMessage> {
        let pool = self.pool().await?;
        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now();
        let mut tx = pool.begin().await?;
        let turn_index: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(turn_index), -1) + 1 FROM recording_messages WHERE recording_id = ?",
        )
        .bind(recording_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            r#"INSERT INTO recording_messages
                 (id, recording_id, created_at, turn_index, role, content, produced_version_id)
               VALUES (?, ?, ?, ?, 'assistant', ?, ?)"#,
        )
        .bind(&id)
        .bind(recording_id)
        .bind(created_at)
        .bind(turn_index)
        .bind(content)
        .bind(produced_version_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(RecordingMessage {
            id,
            recording_id: recording_id.to_string(),
            created_at,
            turn_index,
            role: ChatRole::Assistant,
            content: content.to_string(),
            produced_version_id: produced_version_id.map(str::to_string),
        })
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

CREATE TABLE IF NOT EXISTS recording_versions (
    id                        TEXT PRIMARY KEY,
    recording_id              TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    created_at                TEXT NOT NULL,
    version_no                INTEGER NOT NULL,
    source                    TEXT NOT NULL,
    body                      TEXT NOT NULL,
    thinking                  TEXT,
    source_message_id         TEXT,
    reverted_from_version_id  TEXT
);
CREATE INDEX IF NOT EXISTS recording_versions_rec_idx
  ON recording_versions(recording_id, version_no DESC);

CREATE TABLE IF NOT EXISTS recording_messages (
    id                   TEXT PRIMARY KEY,
    recording_id         TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    created_at           TEXT NOT NULL,
    turn_index           INTEGER NOT NULL,
    role                 TEXT NOT NULL,
    content              TEXT NOT NULL,
    produced_version_id  TEXT
);
CREATE INDEX IF NOT EXISTS recording_messages_rec_idx
  ON recording_messages(recording_id, turn_index ASC);
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VersionSource {
    /// First analysis after recording.
    Initial,
    /// Produced by a chat refinement turn.
    Chat,
    /// Re-running the full pipeline on the same video.
    Retry,
    /// Created by reverting to an earlier version (append-only).
    Revert,
}

impl VersionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Chat => "chat",
            Self::Retry => "retry",
            Self::Revert => "revert",
        }
    }
    fn from_str(s: &str) -> Self {
        match s {
            "initial" => Self::Initial,
            "chat" => Self::Chat,
            "retry" => Self::Retry,
            "revert" => Self::Revert,
            _ => Self::Initial,
        }
    }
}

#[derive(sqlx::FromRow)]
struct RecordingVersionRow {
    id: String,
    recording_id: String,
    created_at: DateTime<Utc>,
    version_no: i64,
    source: String,
    body: String,
    thinking: Option<String>,
    source_message_id: Option<String>,
    source_message_content: Option<String>,
    reverted_from_version_id: Option<String>,
}

impl From<RecordingVersionRow> for RecordingVersion {
    fn from(r: RecordingVersionRow) -> Self {
        RecordingVersion {
            id: r.id,
            recording_id: r.recording_id,
            created_at: r.created_at,
            version_no: r.version_no,
            source: VersionSource::from_str(&r.source),
            body: r.body,
            thinking: r.thinking,
            source_message_id: r.source_message_id,
            source_message_content: r.source_message_content,
            reverted_from_version_id: r.reverted_from_version_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingVersion {
    pub id: String,
    pub recording_id: String,
    pub created_at: DateTime<Utc>,
    pub version_no: i64,
    pub source: VersionSource,
    pub body: String,
    pub thinking: Option<String>,
    pub source_message_id: Option<String>,
    /// Content of the producing user message, when this version was produced
    /// by a chat turn. Pulled in via LEFT JOIN so the history panel can
    /// preview the request that led to each refinement without a second
    /// fetch.
    pub source_message_content: Option<String>,
    pub reverted_from_version_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
}

impl ChatRole {
    fn from_str(s: &str) -> Self {
        match s {
            "assistant" => Self::Assistant,
            _ => Self::User,
        }
    }
}

#[derive(sqlx::FromRow)]
struct RecordingMessageRow {
    id: String,
    recording_id: String,
    created_at: DateTime<Utc>,
    turn_index: i64,
    role: String,
    content: String,
    produced_version_id: Option<String>,
}

impl From<RecordingMessageRow> for RecordingMessage {
    fn from(r: RecordingMessageRow) -> Self {
        RecordingMessage {
            id: r.id,
            recording_id: r.recording_id,
            created_at: r.created_at,
            turn_index: r.turn_index,
            role: ChatRole::from_str(&r.role),
            content: r.content,
            produced_version_id: r.produced_version_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingMessage {
    pub id: String,
    pub recording_id: String,
    pub created_at: DateTime<Utc>,
    pub turn_index: i64,
    pub role: ChatRole,
    pub content: String,
    pub produced_version_id: Option<String>,
}
