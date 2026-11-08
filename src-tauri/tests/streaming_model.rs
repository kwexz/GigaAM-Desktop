//! Streaming file ASR vs the batch implementation that phase 02 replaced.
//!
//! The batch transcript is captured once from the pre-refactor production
//! `FileProcessor` into `state/runs/02/batch-baseline.json`; afterwards this
//! test compares the streaming production path against that frozen artifact.
//! Run with `--ignored` and `GIGAAM_TEST_MODELS` pointing at pinned models.
//!
//! Set `GIGAAM_WRITE_BATCH_BASELINE=1` only while the batch path still exists.

use gigaam_desktop_lib::{
    domain::{
        SourceTrack, TranscriptSegment, Transcription, TranscriptionKind, TranscriptionStatus,
    },
    pipeline::NativeFileProcessor,
    queue::{FileProcessor, process_next},
    storage::Store,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const FIXTURES: &[(&str, &[&str])] = &[
    (
        "besarab-clean.wav",
        &["меня зовут", "журналистика", "науку и технологии"],
    ),
    (
        "besarab-noisy.wav",
        &["меня зовут", "журналистика", "науку и технологии"],
    ),
    ("russian-clean.wav", &[]),
];

fn normalized(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ac) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, bc) in b.iter().enumerate() {
            let old = row[j + 1];
            row[j + 1] = (row[j] + 1)
                .min(old + 1)
                .min(diagonal + usize::from(ac != *bc));
            diagonal = old;
        }
    }
    row[b.len()]
}

fn job(path: &Path) -> Transcription {
    Transcription {
        id: "streaming-fixture".into(),
        kind: TranscriptionKind::File,
        source_filename: path.file_name().unwrap().to_string_lossy().into_owned(),
        source_path: path.to_string_lossy().into_owned(),
        source_size_bytes: std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0),
        source_modified_ms: 0,
        created_ms: 0,
        started_ms: None,
        completed_ms: None,
        status: TranscriptionStatus::Transcribing,
        duration_seconds: None,
        model_version: "test".into(),
        error: None,
        audio_path: None,
        completed_chunks: 0,
        total_chunks: None,
        queue_visible: false,
    }
}

fn transcribe(processor: &mut NativeFileProcessor, path: &Path) -> (Vec<TranscriptSegment>, f64) {
    let mut segments = Vec::new();
    let mut totals = Vec::new();
    let duration = processor
        .process(
            &job(path),
            &mut |total| {
                totals.push(total);
                Ok(())
            },
            &mut |segment| {
                segments.push(segment);
                Ok(())
            },
            &|| Ok(false),
        )
        .unwrap();
    println!(
        "{}: segments={} totals={totals:?} duration={duration:.3}",
        path.file_name().unwrap().to_string_lossy(),
        segments.len()
    );
    (segments, duration)
}

#[test]
#[ignore = "requires pinned models via GIGAAM_TEST_MODELS"]
fn phrase_queue_persists_real_model_output() {
    let model = PathBuf::from(std::env::var_os("GIGAAM_TEST_MODELS").expect("set GIGAAM_TEST_MODELS"));
    let mut processor = NativeFileProcessor::load(&model).unwrap();
    let mut task = job(&fixture_path("besarab-clean.wav"));
    task.status = TranscriptionStatus::Queued;
    task.queue_visible = true;
    let store = Arc::new(Mutex::new(Store::memory().unwrap()));
    store.lock().unwrap().enqueue(&task).unwrap();
    store.lock().unwrap().request_start(&task.id).unwrap();
    assert!(process_next(&store, &mut processor, 1).unwrap());
    let segments = store.lock().unwrap().segments(&task.id).unwrap();
    assert!(!segments.is_empty());
    assert!(segments.windows(2).all(|pair| pair[0].end_seconds <= pair[1].start_seconds));
    let text = segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ").to_lowercase();
    for anchor in ["меня зовут", "журналистика", "науку и технологии"] { assert_eq!(text.matches(anchor).count(), 1, "{anchor}: {text}"); }
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures")
        .join(name)
}

#[test]
#[ignore = "requires pinned models via GIGAAM_TEST_MODELS; run with --ignored"]
fn streaming_file_transcription_matches_frozen_batch_baseline() {
    let model =
        PathBuf::from(std::env::var_os("GIGAAM_TEST_MODELS").expect("set GIGAAM_TEST_MODELS"));
    let baseline_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../state/runs/02/batch-baseline.json");
    let write_baseline = std::env::var_os("GIGAAM_WRITE_BATCH_BASELINE").is_some();
    let mut processor = NativeFileProcessor::load(&model).unwrap();

    let mut current = serde_json::Map::new();
    for (name, anchors) in FIXTURES {
        let path = fixture_path(name);
        let (segments, duration) = transcribe(&mut processor, &path);
        assert!(segments.iter().enumerate().all(|(index, segment)| segment.index == index && segment.id == format!("streaming-fixture:{index}")), "{name}: unstable segment identity");
        let text = segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let normalized_text = normalized(&text);
        for anchor in *anchors {
            let count = text.to_lowercase().matches(anchor).count();
            assert_eq!(count, 1, "{name} anchor {anchor:?} count={count}: {text}");
        }
        assert!(
            segments
                .windows(2)
                .all(|pair| pair[0].end_seconds <= pair[1].start_seconds),
            "{name} segments overlap: {segments:?}"
        );
        assert!(
            segments.iter().all(
                |segment| segment.start_seconds >= 0.0 && segment.end_seconds <= duration + 0.5
            ),
            "{name} segment outside source bounds"
        );
        assert!(
            segments
                .iter()
                .all(|segment| segment.source_track == SourceTrack::File)
        );
        current.insert(
            (*name).to_owned(),
            serde_json::json!({
                "duration": duration,
                "normalized": normalized_text,
                "segments": segments.iter().map(|segment| serde_json::json!({
                    "start": segment.start_seconds,
                    "end": segment.end_seconds,
                    "text": segment.text,
                })).collect::<Vec<_>>(),
            }),
        );
    }

    if write_baseline {
        let directory = baseline_path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(
            &baseline_path,
            serde_json::to_string_pretty(&Value::Object(current)).unwrap(),
        )
        .unwrap();
        println!("wrote batch baseline to {}", baseline_path.display());
        return;
    }

    let baseline: Value =
        serde_json::from_str(&std::fs::read_to_string(&baseline_path).unwrap()).unwrap();
    for (name, _) in FIXTURES {
        let expected = &baseline[*name];
        let actual = &current[*name];
        let expected_text = expected["normalized"].as_str().unwrap();
        let actual_text = actual["normalized"].as_str().unwrap();
        // This fixture is intentionally not a reliable ASR reference (see
        // manifest.json); compare its streaming boundary/count contract only.
        if expected_text.is_empty() || *name == "russian-clean.wav" {
            println!("{name}: ASR text comparison not applicable for degraded fixture");
            continue;
        }
        let cer = edit_distance(expected_text, actual_text) as f64
            / expected_text.chars().count().max(1) as f64;
        println!("{name}: batch_vs_streaming_cer={cer:.4}");
        assert!(
            cer <= 0.25,
            "{name} streaming CER {cer} exceeds 25% vs batch"
        );

        // Sentence splitting can add boundaries within each VAD window, but
        // the original window starts must still be represented nearby.
        let expected_starts: Vec<f64> = expected["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|segment| segment["start"].as_f64().unwrap())
            .collect();
        let actual_starts: Vec<f64> = actual["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|segment| segment["start"].as_f64().unwrap())
            .collect();
        let tolerance = 2.5;
        for start in &expected_starts {
            assert!(
                actual_starts
                    .iter()
                    .any(|candidate| (candidate - start).abs() <= tolerance),
                "{name} lost boundary at {start:.3}s (streaming={actual_starts:?})"
            );
        }
    }
}
