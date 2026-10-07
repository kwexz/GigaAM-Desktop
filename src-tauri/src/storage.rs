use std::{path::Path, str::FromStr};

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::{
    SegmentState, SourceTrack, TranscriptSegment, Transcription, TranscriptionStatus,
};

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let connection = Connection::open(path)?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn memory() -> rusqlite::Result<Self> {
        let connection = Connection::open_in_memory()?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> rusqlite::Result<()> {
        self.connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS transcriptions (
               id TEXT PRIMARY KEY,
               kind TEXT NOT NULL CHECK(kind IN ('file','live')),
               source_filename TEXT NOT NULL,
               source_path TEXT NOT NULL,
               source_size_bytes INTEGER NOT NULL CHECK(source_size_bytes >= 0),
               source_modified_ms INTEGER NOT NULL,
               created_ms INTEGER NOT NULL,
               started_ms INTEGER,
               completed_ms INTEGER,
               status TEXT NOT NULL CHECK(status IN ('queued','preparing','transcribing','completed','cancelled','failed','interrupted')),
               duration_seconds REAL CHECK(duration_seconds IS NULL OR duration_seconds >= 0),
               model_version TEXT NOT NULL,
               error TEXT,
               audio_path TEXT,
               start_requested INTEGER NOT NULL DEFAULT 0 CHECK(start_requested IN (0,1)),
               completed_chunks INTEGER NOT NULL DEFAULT 0 CHECK(completed_chunks >= 0),
               total_chunks INTEGER CHECK(total_chunks IS NULL OR total_chunks >= 0),
               cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0,1)),
               queue_visible INTEGER NOT NULL DEFAULT 1 CHECK(queue_visible IN (0,1))
             );
             CREATE TABLE IF NOT EXISTS transcript_segments (
               id TEXT PRIMARY KEY,
               transcription_id TEXT NOT NULL REFERENCES transcriptions(id) ON DELETE CASCADE,
               segment_index INTEGER NOT NULL CHECK(segment_index >= 0),
               start_seconds REAL NOT NULL CHECK(start_seconds >= 0),
               end_seconds REAL NOT NULL CHECK(end_seconds > start_seconds),
               text TEXT NOT NULL,
               source_track TEXT NOT NULL CHECK(source_track IN ('file','microphone','system')),
               speaker_id TEXT,
               state TEXT NOT NULL CHECK(state IN ('provisional','final')),
               UNIQUE(transcription_id, segment_index)
             );
             CREATE INDEX IF NOT EXISTS idx_transcriptions_queue ON transcriptions(queue_visible, created_ms);
             CREATE INDEX IF NOT EXISTS idx_transcriptions_filename ON transcriptions(source_filename);
             CREATE INDEX IF NOT EXISTS idx_segments_transcription ON transcript_segments(transcription_id, segment_index);",
        )?;
        self.connection.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_single_active_transcription
             ON transcriptions((1)) WHERE status IN ('preparing','transcribing');",
        )?;
        let columns: Vec<String> = self
            .connection
            .prepare("PRAGMA table_info(transcriptions)")?
            .query_map([], |row| row.get(1))?
            .collect::<rusqlite::Result<_>>()?;
        if !columns.iter().any(|column| column == "completed_chunks") {
            self.connection.execute_batch(
                "ALTER TABLE transcriptions ADD COLUMN completed_chunks INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE transcriptions ADD COLUMN total_chunks INTEGER;",
            )?;
        }
        if !columns.iter().any(|column| column == "cancel_requested") {
            self.connection.execute_batch(
                "ALTER TABLE transcriptions ADD COLUMN cancel_requested INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        if !columns.iter().any(|column| column == "start_requested") {
            self.connection.execute_batch(
                "ALTER TABLE transcriptions ADD COLUMN start_requested INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        if !columns.iter().any(|column| column == "audio_path") {
            self.connection
                .execute_batch("ALTER TABLE transcriptions ADD COLUMN audio_path TEXT;")?;
        }
        if !columns.iter().any(|column| column == "audio_path") {
            self.connection
                .execute_batch("ALTER TABLE transcriptions ADD COLUMN audio_path TEXT;")?;
        }
        Ok(())
    }

    pub fn enqueue(&mut self, transcription: &Transcription) -> rusqlite::Result<()> {
        insert_transcription(&self.connection, transcription)
    }

    pub fn enqueue_batch(&mut self, transcriptions: &[Transcription]) -> rusqlite::Result<()> {
        let transaction = self.connection.transaction()?;
        for transcription in transcriptions {
            insert_transcription(&transaction, transcription)?;
        }
        transaction.commit()
    }

    pub fn insert_live_session(&mut self, transcription: &Transcription) -> rusqlite::Result<()> {
        insert_transcription(&self.connection, transcription)
    }

    pub fn claim_next(&mut self, now_ms: i64) -> rusqlite::Result<Option<Transcription>> {
        let transaction = self.connection.transaction()?;
        let active: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM transcriptions WHERE status IN ('preparing','transcribing'))",
            [],
            |row| row.get(0),
        )?;
        if active {
            return Ok(None);
        }
        let id: Option<String> = transaction
            .query_row(
                "SELECT id FROM transcriptions WHERE queue_visible = 1 AND kind = 'file' AND status = 'queued' AND start_requested = 1
                 ORDER BY created_ms, rowid LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let Some(id) = id else { return Ok(None) };
        transaction.execute(
            "UPDATE transcriptions SET status = 'preparing', started_ms = ?2,
                    completed_ms = NULL, error = NULL WHERE id = ?1",
            params![id, now_ms],
        )?;
        let claimed = transaction.query_row(
            "SELECT * FROM transcriptions WHERE id = ?1",
            [&id],
            map_transcription,
        )?;
        transaction.commit()?;
        Ok(Some(claimed))
    }

    pub fn is_cancelled(&self, id: &str) -> rusqlite::Result<bool> {
        self.connection.query_row(
            "SELECT status = 'cancelled' OR cancel_requested = 1 FROM transcriptions WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
    }

    pub fn request_cancel(&mut self, id: &str, now_ms: i64) -> rusqlite::Result<()> {
        let changed = self.connection.execute(
            "UPDATE transcriptions SET
               cancel_requested = CASE WHEN status IN ('preparing','transcribing') THEN 1 ELSE cancel_requested END,
               completed_ms = CASE WHEN status = 'queued' THEN ?2 ELSE completed_ms END,
               status = CASE WHEN status = 'queued' THEN 'cancelled' ELSE status END
             WHERE id = ?1 AND status IN ('queued','preparing','transcribing')",
            params![id, now_ms],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    pub fn request_start(&mut self, id: &str) -> rusqlite::Result<()> {
        let changed = self.connection.execute(
            "UPDATE transcriptions SET start_requested = 1 WHERE id = ?1 AND status = 'queued'",
            [id],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    pub fn acknowledge_cancel(&mut self, id: &str, now_ms: i64) -> rusqlite::Result<()> {
        let changed = self.connection.execute(
            "UPDATE transcriptions SET status = 'cancelled', completed_ms = ?2,
                    cancel_requested = 0 WHERE id = ?1 AND cancel_requested = 1
                    AND status IN ('preparing','transcribing')",
            params![id, now_ms],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    pub fn complete_if_not_cancelled(
        &mut self,
        id: &str,
        duration_seconds: f64,
        now_ms: i64,
    ) -> rusqlite::Result<bool> {
        let changed = self.connection.execute(
            "UPDATE transcriptions SET status = 'completed', duration_seconds = ?2,
                    completed_ms = ?3, error = NULL
             WHERE id = ?1 AND status = 'transcribing' AND cancel_requested = 0",
            params![id, duration_seconds, now_ms],
        )?;
        Ok(changed == 1)
    }

    pub fn fail_if_not_cancelled(
        &mut self,
        id: &str,
        error: &str,
        now_ms: i64,
    ) -> rusqlite::Result<bool> {
        let changed = self.connection.execute(
            "UPDATE transcriptions SET status = 'failed', completed_ms = ?3, error = ?2
             WHERE id = ?1 AND status = 'transcribing' AND cancel_requested = 0",
            params![id, error, now_ms],
        )?;
        Ok(changed == 1)
    }

    pub fn reset_for_retry(&mut self, id: &str) -> rusqlite::Result<()> {
        let transaction = self.connection.transaction()?;
        let (status, kind, audio_path): (String, String, Option<String>) =
            transaction.query_row(
                "SELECT status, kind, audio_path FROM transcriptions WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        if !matches!(
            status.as_str(),
            "cancelled" | "failed" | "interrupted" | "completed"
        ) {
            return Err(rusqlite::Error::InvalidQuery);
        }
        // A live session cannot be re-transcribed live afterwards; instead it
        // becomes a regular file job over its archived FLAC so the existing
        // queue/worker/export flow picks it up unchanged.
        if kind == "live" {
            let audio = audio_path
                .filter(|path| !path.is_empty())
                .ok_or(rusqlite::Error::InvalidQuery)?;
            let size = std::fs::metadata(&audio)
                .map(|meta| meta.len() as i64)
                .ok();
            transaction.execute(
                "UPDATE transcriptions SET kind = 'file', source_path = ?2,
                        source_size_bytes = COALESCE(?3, source_size_bytes)
                 WHERE id = ?1",
                rusqlite::params![id, audio, size],
            )?;
        }
        transaction.execute(
            "DELETE FROM transcript_segments WHERE transcription_id = ?1",
            [id],
        )?;
        transaction.execute(
            "UPDATE transcriptions SET status = 'queued', started_ms = NULL, completed_ms = NULL,
                    duration_seconds = NULL, error = NULL, completed_chunks = 0, total_chunks = NULL,
                    cancel_requested = 0, start_requested = 0, queue_visible = 1
             WHERE id = ?1",
            [id],
        )?;
        transaction.commit()
    }

    pub fn set_duration(&mut self, id: &str, duration_seconds: f64) -> rusqlite::Result<()> {
        self.connection.execute(
            "UPDATE transcriptions SET duration_seconds = ?2 WHERE id = ?1",
            params![id, duration_seconds],
        )?;
        Ok(())
    }

    pub fn set_audio_path(&mut self, id: &str, audio_path: Option<&str>) -> rusqlite::Result<()> {
        self.connection.execute(
            "UPDATE transcriptions SET audio_path = ?2 WHERE id = ?1",
            params![id, audio_path],
        )?;
        Ok(())
    }

    pub fn update_segment_text(
        &mut self,
        transcription_id: &str,
        index: usize,
        text: &str,
    ) -> rusqlite::Result<()> {
        let changed = self.connection.execute(
            "UPDATE transcript_segments SET text = ?3
             WHERE transcription_id = ?1 AND segment_index = ?2",
            params![transcription_id, index as i64, text],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    pub fn set_total_chunks(&mut self, id: &str, total_chunks: usize) -> rusqlite::Result<()> {
        self.connection.execute(
            "UPDATE transcriptions SET total_chunks = ?2 WHERE id = ?1",
            params![id, total_chunks],
        )?;
        Ok(())
    }
}

fn insert_transcription(
    connection: &Connection,
    transcription: &Transcription,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO transcriptions (
               id, kind, source_filename, source_path, source_size_bytes, source_modified_ms,
               created_ms, started_ms, completed_ms, status, duration_seconds, model_version,
                error, audio_path, queue_visible
              ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            transcription.id,
            transcription.kind.to_string(),
            transcription.source_filename,
            transcription.source_path,
            transcription.source_size_bytes,
            transcription.source_modified_ms,
            transcription.created_ms,
            transcription.started_ms,
            transcription.completed_ms,
            transcription.status.to_string(),
            transcription.duration_seconds,
            transcription.model_version,
            transcription.error,
            transcription.audio_path,
            transcription.queue_visible,
        ],
    )?;
    Ok(())
}

impl Store {
    pub fn transition(
        &mut self,
        id: &str,
        next: TranscriptionStatus,
        now_ms: i64,
        error: Option<&str>,
    ) -> rusqlite::Result<()> {
        let current: String = self.connection.query_row(
            "SELECT status FROM transcriptions WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        let current =
            TranscriptionStatus::from_str(&current).map_err(|_| rusqlite::Error::InvalidQuery)?;
        if !can_transition(current, next) {
            return Err(rusqlite::Error::InvalidQuery);
        }
        match next {
            TranscriptionStatus::Queued => {
                self.connection.execute(
                    "UPDATE transcriptions SET status = 'queued', started_ms = NULL,
                            completed_ms = NULL, error = NULL, cancel_requested = 0,
                            queue_visible = 1 WHERE id = ?1",
                    [id],
                )?;
            }
            TranscriptionStatus::Preparing => {
                self.connection.execute(
                    "UPDATE transcriptions SET status = 'preparing', started_ms = ?2,
                            completed_ms = NULL, error = NULL, cancel_requested = 0 WHERE id = ?1",
                    params![id, now_ms],
                )?;
            }
            TranscriptionStatus::Transcribing => {
                self.connection.execute(
                    "UPDATE transcriptions SET status = 'transcribing', error = NULL WHERE id = ?1",
                    [id],
                )?;
            }
            TranscriptionStatus::Completed
            | TranscriptionStatus::Cancelled
            | TranscriptionStatus::Failed
            | TranscriptionStatus::Interrupted => {
                self.connection.execute(
                    "UPDATE transcriptions SET status = ?2, completed_ms = ?3, error = ?4 WHERE id = ?1",
                    params![id, next.to_string(), now_ms, error],
                )?;
            }
        }
        Ok(())
    }

    pub fn list_history(&self) -> rusqlite::Result<Vec<Transcription>> {
        let mut statement = self
            .connection
            .prepare("SELECT * FROM transcriptions ORDER BY created_ms DESC")?;
        statement.query_map([], map_transcription)?.collect()
    }

    pub fn list_queue(&self) -> rusqlite::Result<Vec<Transcription>> {
        let mut statement = self.connection.prepare(
            "SELECT * FROM transcriptions WHERE queue_visible = 1 ORDER BY created_ms, rowid",
        )?;
        statement.query_map([], map_transcription)?.collect()
    }

    pub fn has_active(&self) -> rusqlite::Result<bool> {
        self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM transcriptions WHERE status IN ('preparing','transcribing'))",
            [],
            |row| row.get(0),
        )
    }

    pub fn append_segment(&mut self, segment: &TranscriptSegment) -> rusqlite::Result<()> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO transcript_segments (
               id, transcription_id, segment_index, start_seconds, end_seconds, text,
               source_track, speaker_id, state
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(transcription_id, segment_index) DO UPDATE SET
               id = excluded.id,
               start_seconds = excluded.start_seconds,
               end_seconds = excluded.end_seconds,
               text = excluded.text,
               source_track = excluded.source_track,
               speaker_id = excluded.speaker_id,
               state = excluded.state",
            params![
                segment.id,
                segment.transcription_id,
                segment.index,
                segment.start_seconds,
                segment.end_seconds,
                segment.text,
                source_track_value(segment.source_track),
                segment.speaker_id,
                segment_state_value(segment.state),
            ],
        )?;
        transaction.execute(
            "UPDATE transcriptions SET completed_chunks = MAX(completed_chunks, ?2)
             WHERE id = ?1",
            params![segment.transcription_id, segment.index + 1],
        )?;
        transaction.commit()
    }

    pub fn segments(&self, transcription_id: &str) -> rusqlite::Result<Vec<TranscriptSegment>> {
        let mut statement = self.connection.prepare(
            "SELECT id, transcription_id, segment_index, start_seconds, end_seconds, text,
                    source_track, speaker_id, state
             FROM transcript_segments WHERE transcription_id = ?1 ORDER BY start_seconds, segment_index",
        )?;
        statement
            .query_map([transcription_id], map_segment)?
            .collect()
    }

    pub fn get(&self, id: &str) -> rusqlite::Result<Option<Transcription>> {
        self.connection
            .query_row(
                "SELECT * FROM transcriptions WHERE id = ?1",
                [id],
                map_transcription,
            )
            .optional()
    }

    pub fn search(&self, query: &str) -> rusqlite::Result<Vec<Transcription>> {
        let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
        let mut statement = self.connection.prepare(
            "SELECT DISTINCT t.* FROM transcriptions t
             LEFT JOIN transcript_segments s ON s.transcription_id = t.id
             WHERE t.source_filename LIKE ?1 ESCAPE '\\' OR s.text LIKE ?1 ESCAPE '\\'
             ORDER BY t.created_ms DESC",
        )?;
        statement.query_map([pattern], map_transcription)?.collect()
    }

    pub fn recover_interrupted(&mut self, now_ms: i64) -> rusqlite::Result<usize> {
        self.connection.execute(
            "UPDATE transcriptions SET status = 'interrupted', completed_ms = ?1,
                    error = 'Application stopped during transcription'
             WHERE status IN ('preparing','transcribing')",
            [now_ms],
        )
    }

    pub fn remove_from_queue(&mut self, id: &str, now_ms: i64) -> rusqlite::Result<()> {
        let changed = self.connection.execute(
            "UPDATE transcriptions SET
               queue_visible = 0,
               completed_ms = CASE WHEN status = 'queued' THEN ?2 ELSE completed_ms END,
               status = CASE WHEN status = 'queued' THEN 'cancelled' ELSE status END
             WHERE id = ?1 AND status IN ('queued','completed','cancelled','failed','interrupted')",
            params![id, now_ms],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    pub fn delete_history(&mut self, id: &str) -> rusqlite::Result<()> {
        let changed = self.connection.execute(
            "DELETE FROM transcriptions WHERE id = ?1
             AND status IN ('completed','cancelled','failed','interrupted')",
            [id],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }
}

pub fn can_transition(from: TranscriptionStatus, to: TranscriptionStatus) -> bool {
    use TranscriptionStatus::*;
    matches!(
        (from, to),
        (Queued, Preparing)
            | (Queued, Cancelled)
            | (Preparing, Transcribing)
            | (Preparing, Cancelled | Failed | Interrupted)
            | (Transcribing, Completed | Cancelled | Failed | Interrupted)
            | (Cancelled | Failed | Interrupted, Queued)
    )
}

fn source_track_value(value: SourceTrack) -> &'static str {
    match value {
        SourceTrack::File => "file",
        SourceTrack::Microphone => "microphone",
        SourceTrack::System => "system",
    }
}

fn segment_state_value(value: SegmentState) -> &'static str {
    match value {
        SegmentState::Provisional => "provisional",
        SegmentState::Final => "final",
    }
}

fn map_transcription(row: &rusqlite::Row<'_>) -> rusqlite::Result<Transcription> {
    let kind: String = row.get("kind")?;
    let status: String = row.get("status")?;
    Ok(Transcription {
        id: row.get("id")?,
        kind: kind.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
        source_filename: row.get("source_filename")?,
        source_path: row.get("source_path")?,
        source_size_bytes: row.get("source_size_bytes")?,
        source_modified_ms: row.get("source_modified_ms")?,
        created_ms: row.get("created_ms")?,
        started_ms: row.get("started_ms")?,
        completed_ms: row.get("completed_ms")?,
        status: status.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
        duration_seconds: row.get("duration_seconds")?,
        model_version: row.get("model_version")?,
        error: row.get("error")?,
        audio_path: row.get("audio_path")?,
        completed_chunks: row.get("completed_chunks")?,
        total_chunks: row.get("total_chunks")?,
        queue_visible: row.get("queue_visible")?,
    })
}

fn map_segment(row: &rusqlite::Row<'_>) -> rusqlite::Result<TranscriptSegment> {
    let track: String = row.get("source_track")?;
    let state: String = row.get("state")?;
    Ok(TranscriptSegment {
        id: row.get("id")?,
        transcription_id: row.get("transcription_id")?,
        index: row.get("segment_index")?,
        start_seconds: row.get("start_seconds")?,
        end_seconds: row.get("end_seconds")?,
        text: row.get("text")?,
        source_track: match track.as_str() {
            "file" => SourceTrack::File,
            "microphone" => SourceTrack::Microphone,
            "system" => SourceTrack::System,
            _ => return Err(rusqlite::Error::InvalidQuery),
        },
        speaker_id: row.get("speaker_id")?,
        state: match state.as_str() {
            "provisional" => SegmentState::Provisional,
            "final" => SegmentState::Final,
            _ => return Err(rusqlite::Error::InvalidQuery),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TranscriptionKind;

    fn transcription(id: &str, status: TranscriptionStatus) -> Transcription {
        Transcription {
            id: id.into(),
            kind: TranscriptionKind::File,
            source_filename: "recording.wav".into(),
            source_path: "C:\\missing\\recording.wav".into(),
            source_size_bytes: 100,
            source_modified_ms: 1,
            created_ms: 2,
            started_ms: None,
            completed_ms: None,
            status,
            duration_seconds: None,
            model_version: "v3_e2e_ctc.int8".into(),
            error: None,
            audio_path: None,
            completed_chunks: 0,
            total_chunks: None,
            queue_visible: true,
        }
    }

    #[test]
    fn persists_segments_and_searches_without_source_file() {
        let mut store = Store::memory().unwrap();
        store
            .enqueue(&transcription("tx", TranscriptionStatus::Queued))
            .unwrap();
        let segment = TranscriptSegment::new(
            "tx",
            0,
            1.0,
            2.0,
            "важный текст".into(),
            SourceTrack::File,
            SegmentState::Final,
        )
        .unwrap();
        store.append_segment(&segment).unwrap();
        assert_eq!(store.segments("tx").unwrap(), [segment]);
        assert_eq!(store.search("важный").unwrap()[0].id, "tx");
    }

    #[test]
    fn segment_text_updates_in_place() {
        let mut store = Store::memory().unwrap();
        store
            .enqueue(&transcription("tx", TranscriptionStatus::Queued))
            .unwrap();
        let segment = TranscriptSegment::new(
            "tx",
            0,
            1.0,
            2.0,
            "до".into(),
            SourceTrack::File,
            SegmentState::Final,
        )
        .unwrap();
        store.append_segment(&segment).unwrap();
        store.update_segment_text("tx", 0, "после").unwrap();
        assert_eq!(store.segments("tx").unwrap()[0].text, "после");
        assert_eq!(store.search("после").unwrap()[0].id, "tx");
        assert!(store.update_segment_text("tx", 7, "мимо").is_err());
        assert!(store.update_segment_text("нет", 0, "мимо").is_err());
    }

    #[test]
    fn recovers_interrupted_work_and_allows_retry() {
        let mut store = Store::memory().unwrap();
        store
            .enqueue(&transcription("tx", TranscriptionStatus::Queued))
            .unwrap();
        store
            .transition("tx", TranscriptionStatus::Preparing, 3, None)
            .unwrap();
        store
            .transition("tx", TranscriptionStatus::Transcribing, 4, None)
            .unwrap();
        assert_eq!(store.recover_interrupted(5).unwrap(), 1);
        assert_eq!(
            store.get("tx").unwrap().unwrap().status,
            TranscriptionStatus::Interrupted
        );
        store
            .transition("tx", TranscriptionStatus::Queued, 6, None)
            .unwrap();
        assert_eq!(
            store.get("tx").unwrap().unwrap().status,
            TranscriptionStatus::Queued
        );
    }

    #[test]
    fn retry_from_completed_clears_segments_and_requeues() {
        let mut store = Store::memory().unwrap();
        store
            .enqueue(&transcription("tx", TranscriptionStatus::Queued))
            .unwrap();
        store.request_start("tx").unwrap();
        let job = store.claim_next(3).unwrap().unwrap();
        assert_eq!(job.status, TranscriptionStatus::Preparing);
        store
            .transition(&job.id, TranscriptionStatus::Transcribing, 4, None)
            .unwrap();
        assert!(store.complete_if_not_cancelled(&job.id, 2.0, 5).unwrap());
        store.reset_for_retry(&job.id).unwrap();
        let retried = store.get(&job.id).unwrap().unwrap();
        assert_eq!(retried.status, TranscriptionStatus::Queued);
        assert_eq!(retried.completed_chunks, 0);
        store.request_start(&job.id).unwrap();
        assert_eq!(store.claim_next(6).unwrap().unwrap().id, job.id);
    }

    #[test]
    fn retry_converts_live_session_into_file_job() {
        let mut store = Store::memory().unwrap();
        let dir =
            std::env::temp_dir().join(format!("gigaam-retry-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let audio = dir.join("microphone.flac");
        std::fs::write(&audio, b"fake-flac").unwrap();
        let mut tx = transcription("live1", TranscriptionStatus::Failed);
        tx.kind = TranscriptionKind::Live;
        tx.audio_path = Some(audio.to_string_lossy().into_owned());
        store.enqueue(&tx).unwrap();
        store.reset_for_retry("live1").unwrap();
        let retried = store.get("live1").unwrap().unwrap();
        assert_eq!(retried.kind, TranscriptionKind::File);
        assert_eq!(retried.source_path, audio.to_string_lossy());
        assert_eq!(retried.status, TranscriptionStatus::Queued);
        store.request_start("live1").unwrap();
        assert_eq!(store.claim_next(9).unwrap().unwrap().id, "live1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retry_live_without_audio_is_rejected() {
        let mut store = Store::memory().unwrap();
        let mut tx = transcription("live2", TranscriptionStatus::Failed);
        tx.kind = TranscriptionKind::Live;
        tx.audio_path = None;
        store.enqueue(&tx).unwrap();
        assert!(store.reset_for_retry("live2").is_err());
    }

    #[test]
    fn queue_worker_never_claims_live_rows() {
        let mut store = Store::memory().unwrap();
        let mut tx = transcription("live3", TranscriptionStatus::Queued);
        tx.kind = TranscriptionKind::Live;
        store.enqueue(&tx).unwrap();
        store.request_start("live3").unwrap();
        assert!(store.claim_next(9).unwrap().is_none());
    }

    #[test]
    fn rejects_invalid_queue_transition() {
        let mut store = Store::memory().unwrap();
        store
            .enqueue(&transcription("tx", TranscriptionStatus::Queued))
            .unwrap();
        assert!(
            store
                .transition("tx", TranscriptionStatus::Completed, 3, None)
                .is_err()
        );
    }

    #[test]
    fn queue_is_fifo_and_retry_resets_attempt_fields() {
        let mut store = Store::memory().unwrap();
        let mut first = transcription("first", TranscriptionStatus::Queued);
        first.created_ms = 1;
        let mut second = transcription("second", TranscriptionStatus::Queued);
        second.created_ms = 2;
        store.enqueue(&second).unwrap();
        store.enqueue(&first).unwrap();
        store.request_start("first").unwrap();
        assert_eq!(store.claim_next(3).unwrap().unwrap().id, "first");
        store
            .transition(
                "first",
                TranscriptionStatus::Cancelled,
                4,
                Some("cancelled"),
            )
            .unwrap();
        store
            .transition("first", TranscriptionStatus::Queued, 5, None)
            .unwrap();
        let retried = store.get("first").unwrap().unwrap();
        assert_eq!(retried.status, TranscriptionStatus::Queued);
        assert_eq!(retried.started_ms, None);
        assert_eq!(retried.completed_ms, None);
        assert_eq!(retried.error, None);
    }

    #[test]
    fn clearing_queue_preserves_history_and_segments() {
        let mut store = Store::memory().unwrap();
        store
            .enqueue(&transcription("tx", TranscriptionStatus::Queued))
            .unwrap();
        let segment = TranscriptSegment::new(
            "tx",
            0,
            0.0,
            1.0,
            "text".into(),
            SourceTrack::File,
            SegmentState::Final,
        )
        .unwrap();
        store.append_segment(&segment).unwrap();
        store
            .transition("tx", TranscriptionStatus::Cancelled, 3, None)
            .unwrap();
        store.remove_from_queue("tx", 4).unwrap();
        assert!(store.list_queue().unwrap().is_empty());
        assert_eq!(store.list_history().unwrap().len(), 1);
        assert_eq!(store.segments("tx").unwrap(), [segment]);
    }

    #[test]
    fn deleting_history_cascades_segments() {
        let mut store = Store::memory().unwrap();
        store
            .enqueue(&transcription("tx", TranscriptionStatus::Queued))
            .unwrap();
        let segment = TranscriptSegment::new(
            "tx",
            0,
            0.0,
            1.0,
            "text".into(),
            SourceTrack::File,
            SegmentState::Final,
        )
        .unwrap();
        store.append_segment(&segment).unwrap();
        store
            .transition("tx", TranscriptionStatus::Cancelled, 3, None)
            .unwrap();
        store.delete_history("tx").unwrap();
        assert!(store.segments("tx").unwrap().is_empty());
    }
}
