use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    domain::{SourceTrack, TranscriptSegment, Transcription, TranscriptionStatus},
    storage::Store,
};

pub trait FileProcessor {
    fn set_phrase_recognition(&mut self, _enabled: bool) {}
    fn process(
        &mut self,
        job: &Transcription,
        set_total: &mut dyn FnMut(usize) -> Result<(), String>,
        emit: &mut dyn FnMut(TranscriptSegment) -> Result<(), String>,
        is_cancelled: &dyn Fn() -> Result<bool, String>,
    ) -> Result<f64, String>;
}

#[derive(serde::Deserialize)]
struct TrackOffset {
    file: String,
    #[serde(default)]
    offset_seconds: f64,
}

/// Archived tracks of a converted dual live session: the row points at
/// a generated `mixed.*`, but per-track transcription keeps microphone/system labels.
/// Returns (path, track, offset_seconds), or None for ordinary file jobs.
fn live_tracks_for(source_path: &str) -> Option<Vec<(std::path::PathBuf, SourceTrack, f64)>> {
    use std::path::Path;
    let source = Path::new(source_path);
    if !source.file_stem()?.to_str()?.eq_ignore_ascii_case("mixed") {
        return None;
    }
    let directory = source.parent()?;
    let listing = std::fs::read_to_string(directory.join("tracks.json")).ok()?;
    let offsets: Vec<TrackOffset> = serde_json::from_str(&listing).ok()?;
    let mut tracks = Vec::new();
    for entry in offsets {
        if entry.file.starts_with("mixed.") {
            continue;
        }
        let path = directory.join(&entry.file);
        if !path.exists() {
            continue;
        }
        let track = if entry.file.starts_with("microphone") {
            SourceTrack::Microphone
        } else if entry.file.contains("system") {
            SourceTrack::System
        } else {
            SourceTrack::File
        };
        tracks.push((path, track, entry.offset_seconds.max(0.0)));
    }
    if tracks.len() < 2 {
        return None;
    }
    tracks.sort_by(|a, b| a.0.cmp(&b.0));
    Some(tracks)
}

pub fn process_next(
    store: &Arc<Mutex<Store>>,
    processor: &mut impl FileProcessor,
    now_ms: i64,
) -> Result<bool, String> {
    let terminal_time = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    };
    let Some(job) = store
        .lock()
        .map_err(|_| "database lock is poisoned")?
        .claim_next(now_ms)
        .map_err(|error| error.to_string())?
    else {
        return Ok(false);
    };
    let phrase_recognition = store.lock().map_err(|_| "database lock is poisoned")?.phrase_recognition().map_err(|error| error.to_string())?;
    processor.set_phrase_recognition(phrase_recognition);
    store
        .lock()
        .map_err(|_| "database lock is poisoned")?
        .transition(&job.id, TranscriptionStatus::Transcribing, now_ms, None)
        .map_err(|error| error.to_string())?;

    let is_cancelled = || {
        store
            .lock()
            .map_err(|_| "database lock is poisoned".to_string())?
            .is_cancelled(&job.id)
            .map_err(|error| error.to_string())
    };
    let mut previous: Option<TranscriptSegment> = None;
    let mut emit = |mut segment: TranscriptSegment| {
        if is_cancelled()? {
            return Err("transcription cancelled".into());
        }
        let mut db = store.lock().map_err(|_| "database lock is poisoned".to_string())?;
        if let Some(last) = previous.as_ref() {
            let unfinished = !last.text.trim_end().trim_end_matches(['"', '\'', '»', ')', ']']).ends_with(['.', '!', '?', '…']);
            if phrase_recognition && unfinished && last.source_track == segment.source_track && last.speaker_id == segment.speaker_id
                && segment.start_seconds >= last.end_seconds && segment.start_seconds - last.end_seconds <= 1.0
                && last.text.len() + segment.text.len() <= 8192
                && db.extend_segment_if_unchanged(last, &segment).map_err(|error| error.to_string())? {
                let mut merged = last.clone();
                merged.text = format!("{} {}", last.text.trim_end(), segment.text.trim_start());
                merged.end_seconds = segment.end_seconds;
                previous = Some(merged);
                return Ok(());
            }
        }
        // Keep processor indices unique even when an earlier fragment was merged.
        segment.id = format!("{}:{}", segment.transcription_id, segment.index);
        db.append_segment(&segment).map_err(|error| error.to_string())?;
        previous = Some(segment);
        Ok(())
    };
    let mut set_total = |total: usize| {
        store
            .lock()
            .map_err(|_| "database lock is poisoned".to_string())?
            .set_total_chunks(&job.id, total)
            .map_err(|error| error.to_string())
    };

    let result = if let Some(tracks) = live_tracks_for(&job.source_path) {
        // Converted dual live session: transcribe each archived track so
        // segments keep their microphone/system labels, then merge by time
        // (the store orders segments by start_seconds on read).
        let mut index_base = 0usize;
        let mut base_total = 0usize;
        let mut duration = 0.0f64;
        let mut cancelled = false;
        let mut failure: Option<String> = None;
        for (path, track, offset) in &tracks {
            if is_cancelled()? {
                cancelled = true;
                break;
            }
            let mut sub_job = job.clone();
            sub_job.source_path = path.to_string_lossy().into_owned();
            let mut track_chunks = 0usize;
            let mut emitted = 0usize;
            let track_result = processor.process(
                &sub_job,
                &mut |total| {
                    track_chunks = total;
                    set_total(base_total + total)
                },
                &mut |mut segment: TranscriptSegment| {
                    segment.index = index_base + emitted;
                    segment.id = format!("{}:{}", segment.transcription_id, segment.index);
                    segment.source_track = *track;
                    segment.start_seconds += *offset;
                    segment.end_seconds += *offset;
                    emitted += 1;
                    emit(segment)
                },
                &is_cancelled,
            );
            match track_result {
                Ok(track_duration) => {
                    duration = duration.max(track_duration);
                    base_total += track_chunks;
                    index_base += emitted;
                }
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        if cancelled {
            Err("transcription cancelled".into())
        } else {
            failure.map_or(Ok(duration), Err)
        }
    } else {
        processor.process(&job, &mut set_total, &mut emit, &is_cancelled)
    };
    if is_cancelled()? {
        store
            .lock()
            .map_err(|_| "database lock is poisoned")?
            .acknowledge_cancel(&job.id, terminal_time())
            .map_err(|error| error.to_string())?;
        return Ok(true);
    }
    match result {
        Ok(duration_seconds) => {
            let mut store = store.lock().map_err(|_| "database lock is poisoned")?;
            if !store
                .complete_if_not_cancelled(&job.id, duration_seconds, terminal_time())
                .map_err(|error| error.to_string())?
            {
                store
                    .acknowledge_cancel(&job.id, terminal_time())
                    .map_err(|error| error.to_string())?;
            }
        }
        Err(error) => {
            let mut store = store.lock().map_err(|_| "database lock is poisoned")?;
            if !store
                .fail_if_not_cancelled(&job.id, &error, terminal_time())
                .map_err(|storage_error| storage_error.to_string())?
            {
                store
                    .acknowledge_cancel(&job.id, terminal_time())
                    .map_err(|storage_error| storage_error.to_string())?;
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{SegmentState, SourceTrack, TranscriptionKind};

    struct FakeProcessor {
        fail: bool,
    }

    struct PhraseProcessor { enabled: bool }
    impl FileProcessor for PhraseProcessor {
        fn set_phrase_recognition(&mut self, enabled: bool) { self.enabled = enabled; }
        fn process(&mut self, job: &Transcription, total: &mut dyn FnMut(usize) -> Result<(), String>, emit: &mut dyn FnMut(TranscriptSegment) -> Result<(), String>, _: &dyn Fn() -> Result<bool, String>) -> Result<f64, String> {
            total(2)?;
            for (index, text) in ["начало", "конец."].into_iter().enumerate() {
                emit(TranscriptSegment::new(&job.id, index, index as f64, (index + 1) as f64, text.into(), SourceTrack::File, SegmentState::Final).unwrap())?;
            }
            Ok(2.0)
        }
    }

    #[test]
    fn phrase_setting_controls_durable_cross_window_joining() {
        for enabled in [false, true] {
            let store = Arc::new(Mutex::new(Store::memory().unwrap()));
            store.lock().unwrap().set_phrase_recognition(enabled).unwrap();
            store.lock().unwrap().enqueue(&job("phrase", 1)).unwrap();
            store.lock().unwrap().request_start("phrase").unwrap();
            let mut processor = PhraseProcessor { enabled: !enabled };
            process_next(&store, &mut processor, 10).unwrap();
            assert_eq!(processor.enabled, enabled);
            let segments = store.lock().unwrap().segments("phrase").unwrap();
            if enabled {
                assert_eq!(segments.len(), 1);
                assert_eq!(segments[0].text, "начало конец.");
                assert_eq!(segments[0].end_seconds, 2.0);
            } else {
                assert_eq!(segments.len(), 2);
            }
        }
    }

    impl FileProcessor for FakeProcessor {
        fn process(
            &mut self,
            job: &Transcription,
            set_total: &mut dyn FnMut(usize) -> Result<(), String>,
            emit: &mut dyn FnMut(TranscriptSegment) -> Result<(), String>,
            is_cancelled: &dyn Fn() -> Result<bool, String>,
        ) -> Result<f64, String> {
            if self.fail {
                return Err("decode failed".into());
            }
            assert!(!is_cancelled()?);
            set_total(1)?;
            emit(
                TranscriptSegment::new(
                    &job.id,
                    0,
                    0.0,
                    1.0,
                    format!("text-{}", job.id),
                    SourceTrack::File,
                    SegmentState::Final,
                )
                .unwrap(),
            )?;
            Ok(2.0)
        }
    }

    fn job(id: &str, created_ms: i64) -> Transcription {
        Transcription {
            id: id.into(),
            kind: TranscriptionKind::File,
            source_filename: format!("{id}.wav"),
            source_path: format!("source-{id}"),
            source_size_bytes: 1,
            source_modified_ms: 1,
            created_ms,
            started_ms: None,
            completed_ms: None,
            status: TranscriptionStatus::Queued,
            duration_seconds: None,
            model_version: "test".into(),
            error: None,
            audio_path: None,
            completed_chunks: 0,
            total_chunks: None,
            queue_visible: true,
        }
    }

    #[test]
    fn multitrack_retry_keeps_labels_and_offsets() {
        let dir =
            std::env::temp_dir().join(format!("gigaam-multitrack-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("tracks.json"),
            r#"[{"file":"microphone.flac","offset_seconds":0.0},{"file":"system.flac","offset_seconds":5.0}]"#,
        )
        .unwrap();
        std::fs::write(dir.join("microphone.flac"), b"fake").unwrap();
        std::fs::write(dir.join("system.flac"), b"fake").unwrap();
        let store = Arc::new(Mutex::new(Store::memory().unwrap()));
        let mut job_spec = job("multi", 1);
        job_spec.source_path = dir.join("mixed.wav").to_string_lossy().into_owned();
        store.lock().unwrap().enqueue(&job_spec).unwrap();
        store.lock().unwrap().request_start("multi").unwrap();
        let mut processor = FakeProcessor { fail: false };
        assert!(process_next(&store, &mut processor, 10).unwrap());
        let store = store.lock().unwrap();
        let segments = store.segments("multi").unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].source_track, SourceTrack::Microphone);
        assert_eq!(segments[0].start_seconds, 0.0);
        assert_eq!(segments[1].source_track, SourceTrack::System);
        assert_eq!(segments[1].start_seconds, 5.0);
        assert_eq!(segments[1].index, 1);
        assert_eq!(store.get("multi").unwrap().unwrap().total_chunks, Some(2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn processes_fifo_and_persists_each_segment() {
        let store = Arc::new(Mutex::new(Store::memory().unwrap()));
        store
            .lock()
            .unwrap()
            .enqueue_batch(&[job("one", 1), job("two", 2)])
            .unwrap();
        store.lock().unwrap().request_start("one").unwrap();
        let mut processor = FakeProcessor { fail: false };
        assert!(process_next(&store, &mut processor, 10).unwrap());
        let store = store.lock().unwrap();
        assert_eq!(
            store.get("one").unwrap().unwrap().status,
            TranscriptionStatus::Completed
        );
        assert_eq!(
            store.get("two").unwrap().unwrap().status,
            TranscriptionStatus::Queued
        );
        assert_eq!(store.segments("one").unwrap()[0].text, "text-one");
    }

    #[test]
    fn failure_does_not_stop_the_rest_of_the_queue() {
        let store = Arc::new(Mutex::new(Store::memory().unwrap()));
        store
            .lock()
            .unwrap()
            .enqueue_batch(&[job("one", 1), job("two", 2)])
            .unwrap();
        store.lock().unwrap().request_start("one").unwrap();
        process_next(&store, &mut FakeProcessor { fail: true }, 10).unwrap();
        assert_eq!(
            store.lock().unwrap().get("one").unwrap().unwrap().status,
            TranscriptionStatus::Failed
        );
        store.lock().unwrap().request_start("two").unwrap();
        process_next(&store, &mut FakeProcessor { fail: false }, 11).unwrap();
        assert_eq!(
            store.lock().unwrap().get("two").unwrap().unwrap().status,
            TranscriptionStatus::Completed
        );
    }
}
