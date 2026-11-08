use gigaam_desktop_lib::{
    domain::SourceTrack,
    pipeline::{LiveScanner, NativeFileProcessor},
};
use gigaam_desktop_lib::{
    domain::{Transcription, TranscriptionKind, TranscriptionStatus},
    queue::FileProcessor,
};
use std::path::PathBuf;

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

#[test]
#[ignore = "requires explicitly provided pinned models; run with --ignored"]
fn chunk_fed_live_retains_three_phrase_anchors_on_each_track() {
    let model =
        PathBuf::from(std::env::var_os("GIGAAM_TEST_MODELS").expect("set GIGAAM_TEST_MODELS"));
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/besarab-clean.wav");
    let mut reader = hound::WavReader::open(&path).unwrap();
    let source: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect();
    let audio: Vec<f32> = source.iter().flat_map(|sample| [*sample; 3]).collect();
    let job = Transcription {
        id: "file-reference".into(),
        kind: TranscriptionKind::File,
        source_filename: "besarab-clean.wav".into(),
        source_path: path.to_string_lossy().into_owned(),
        source_size_bytes: 0,
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
    };
    let mut file = NativeFileProcessor::load(&model).unwrap();
    let mut reference = Vec::new();
    file.process(
        &job,
        &mut |_| Ok(()),
        &mut |segment| {
            reference.push(segment);
            Ok(())
        },
        &|| Ok(false),
    )
    .unwrap();
    let reference = normalized(
        &reference
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    );
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for (track, step) in [
            (SourceTrack::Microphone, 24_000),
            (SourceTrack::System, 48_000),
        ] {
            let (model, audio, source, reference) = (&model, &audio, &source, &reference);
            workers.push(scope.spawn(move || {
                let mut processor = NativeFileProcessor::load_live(&model).unwrap();
                let mut scanner = LiveScanner::new().unwrap();
                let mut emitted = 0.0;
                let mut quiet = 0.0;
                let mut all = Vec::new();
                for end in (step..audio.len() + step).step_by(step) {
                    scanner.push(&audio[..end.min(audio.len())], 0).unwrap();
                    let (segments, next, silence) = processor
                        .transcribe_live_ready(
                            &mut scanner,
                            "fixture",
                            track,
                            all.len(),
                            emitted,
                            quiet,
                            false,
                        )
                        .unwrap();
                    all.extend(segments);
                    emitted = next;
                    quiet = silence;
                    scanner.discard_before(emitted.max(quiet));
                }
                scanner.finish().unwrap();
                let (segments, _, _) = processor
                    .transcribe_live_ready(
                        &mut scanner,
                        "fixture",
                        track,
                        all.len(),
                        emitted,
                        quiet,
                        true,
                    )
                    .unwrap();
                all.extend(segments);
                let text = all
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_lowercase();
                let cer = edit_distance(&reference, &normalized(&text)) as f64
                    / reference.chars().count() as f64;
                println!("track={track:?} snapshot_samples={step} file_live_cer={cer:.4}");
                assert!(
                    cer <= 0.25,
                    "Live/file CER exceeds 25% baseline tolerance: {cer}"
                );
                for phrase in ["меня зовут", "журналистика", "науку и технологии"]
                {
                    assert!(text.contains(phrase), "{track:?} missing {phrase}: {text}");
                    assert_eq!(
                        text.matches(phrase).count(),
                        1,
                        "{track:?} duplicated {phrase}: {text}"
                    );
                }
                assert!(all.iter().all(|s| s.source_track == track));
                assert!(
                    all.windows(2)
                        .all(|s| s[0].end_seconds <= s[1].start_seconds)
                );
                assert!(all.iter().all(|s| s.start_seconds >= 0.0
                    && s.end_seconds <= source.len() as f64 / 16_000.0 + 0.25));
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
    });
}
