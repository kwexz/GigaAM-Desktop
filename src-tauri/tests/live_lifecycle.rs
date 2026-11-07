use gigaam_desktop_lib::{
    domain::{
        SegmentState, SourceTrack, TranscriptSegment, Transcription, TranscriptionKind,
        TranscriptionStatus,
    },
    export::{ExportFormat, export},
    live::{ArchiveFormat, LiveCapture, LiveSource},
    storage::Store,
};

struct TestDirectory(std::path::PathBuf);
impl Drop for TestDirectory {
    fn drop(&mut self) {
        if let Ok(entries) = std::fs::read_dir(&self.0) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let _ = std::fs::remove_dir_all(path);
                } else {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        let _ = std::fs::remove_dir(&self.0);
    }
}

fn session(id: &str, started_ms: i64) -> Transcription {
    Transcription {
        id: id.into(),
        kind: TranscriptionKind::Live,
        source_filename: format!("Live session {id}"),
        source_path: String::new(),
        source_size_bytes: 0,
        source_modified_ms: started_ms,
        created_ms: started_ms,
        started_ms: Some(started_ms),
        completed_ms: None,
        status: TranscriptionStatus::Transcribing,
        duration_seconds: None,
        model_version: "v3_e2e_ctc.int8".into(),
        error: None,
        audio_path: None,
        completed_chunks: 0,
        total_chunks: None,
        queue_visible: false,
    }
}

#[test]
fn repeated_live_sessions_preserve_archives_edits_and_exports() {
    let mut store = Store::memory().unwrap();
    let mut session_ids = Vec::new();
    let mut guards = Vec::new();

    for round in 0..2u64 {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("gigaam-lifecycle-{round}-{nonce}"));
        std::fs::create_dir(&directory).unwrap();
        guards.push(TestDirectory(directory.clone()));

        let id = format!("live-round-{round}");
        let started_ms = 1_700_000_000_000 + round as i64 * 60_000;
        let mut capture = LiveCapture::new(
            id.clone(),
            started_ms,
            LiveSource::SystemAndMicrophone,
            None,
            true,
            ArchiveFormat::Wav,
            1.0,
            directory.clone(),
            None,
        );
        capture.initialize_paths().unwrap();

        // Record, pause (timestamp gap with no samples), then resume.
        capture.append_timed(true, 10_000_000, vec![0.2; 4_800]);
        capture.append_timed(false, 10_000_000, vec![-0.2; 4_800]);
        capture.append_timed(true, 30_000_000, vec![0.3; 4_800]);
        capture.append_timed(false, 30_000_000, vec![-0.3; 4_800]);

        let files = capture.finish().unwrap();
        assert_eq!(files.len(), 2, "both tracks must produce an archive");
        for file in &files {
            let reader = hound::WavReader::open(file).unwrap();
            assert_eq!(
                reader.duration(),
                9_600,
                "paused gap must not duplicate samples"
            );
            assert!(reader.duration() as f64 / 48_000.0 < 5.0);
        }

        store
            .insert_live_session(&session(&id, started_ms))
            .unwrap();
        let before = store.get(&id).unwrap().unwrap();
        assert_eq!(before.status, TranscriptionStatus::Transcribing);
        assert!(
            before.audio_path.is_none(),
            "playback must wait for a valid archive"
        );

        store.begin_live_finalization(&id, started_ms + 1).unwrap();
        store
            .complete_live_archive(&id, 4.0, Some(files[0].to_str().unwrap()), started_ms + 2)
            .unwrap();
        let completed = store.get(&id).unwrap().unwrap();
        assert_eq!(completed.status, TranscriptionStatus::Completed);
        assert!(
            completed
                .audio_path
                .as_deref()
                .is_some_and(|path| std::path::Path::new(path).is_file())
        );
        assert_eq!(completed.duration_seconds, Some(4.0));

        let first_segment = TranscriptSegment::new(
            &id,
            0,
            0.0,
            1.0,
            "исходная реплика".into(),
            SourceTrack::Microphone,
            SegmentState::Final,
        )
        .unwrap();
        store.append_segment(&first_segment).unwrap();
        store
            .append_segment(
                &TranscriptSegment::new(
                    &id,
                    1,
                    1.2,
                    2.4,
                    "системная реплика".into(),
                    SourceTrack::System,
                    SegmentState::Final,
                )
                .unwrap(),
            )
            .unwrap();
        store
            .update_segment_text(&id, 0, "исправленная реплика")
            .unwrap();
        store.rename(&id, &format!("Renamed {round}")).unwrap();

        let segments = store.segments(&id).unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "исправленная реплика");
        assert_eq!(segments[0].start_seconds, 0.0);
        assert_eq!(segments[1].source_track, SourceTrack::System);

        for format in [ExportFormat::Txt, ExportFormat::Srt, ExportFormat::Vtt] {
            let exported = export(&segments, format).unwrap();
            assert!(exported.contains("исправленная реплика"));
            assert!(exported.contains("системная реплика"));
        }
        let json = export(&segments, ExportFormat::Json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.to_string().contains("исправленная реплика"));

        capture.cleanup_committed_raw().unwrap();
        assert!(!capture.microphone_path.exists());
        assert!(!capture.system_path.exists());
        assert!(files.iter().all(|file| file.is_file()));

        session_ids.push(id);
    }

    let history = store.list_history().unwrap();
    assert_eq!(history.len(), 2);
    for (round, id) in session_ids.iter().enumerate() {
        let record = store.get(id).unwrap().unwrap();
        assert_eq!(record.source_filename, format!("Renamed {round}"));
        assert_eq!(record.status, TranscriptionStatus::Completed);
        let segments = store.segments(id).unwrap();
        assert_eq!(segments[0].text, "исправленная реплика");
        assert!(
            record
                .audio_path
                .as_deref()
                .is_some_and(|path| std::path::Path::new(path).is_file())
        );
    }
}
