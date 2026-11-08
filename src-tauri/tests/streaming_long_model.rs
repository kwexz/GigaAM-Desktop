use gigaam_desktop_lib::{
    domain::{Transcription, TranscriptionKind, TranscriptionStatus},
    pipeline::NativeFileProcessor,
    queue::FileProcessor,
};
use std::path::PathBuf;

#[test]
#[ignore = "requires pinned models via GIGAAM_TEST_MODELS; run with --ignored"]
fn five_minute_streaming_model_e2e_keeps_duration_and_segments() {
    let model =
        PathBuf::from(std::env::var_os("GIGAAM_TEST_MODELS").expect("set GIGAAM_TEST_MODELS"));
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/besarab-clean.wav");
    let directory = std::env::temp_dir().join(format!("gigaam-stream-long-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("five-minute.wav");
    let mut source = hound::WavReader::open(&fixture).unwrap();
    let spec = source.spec();
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    let samples: Vec<i16> = source
        .samples::<i16>()
        .map(|sample| sample.unwrap())
        .collect();
    let repeats = (5.0 * 60.0 / (samples.len() as f64 / spec.sample_rate as f64)).ceil() as usize;
    for _ in 0..repeats {
        for sample in &samples {
            writer.write_sample(*sample).unwrap();
        }
    }
    writer.finalize().unwrap();

    let job = Transcription {
        id: "five-minute-stream".into(),
        kind: TranscriptionKind::File,
        source_filename: "five-minute.wav".into(),
        source_path: path.to_string_lossy().into_owned(),
        source_size_bytes: std::fs::metadata(&path).unwrap().len(),
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
    let expected_duration = samples.len() as f64 * repeats as f64 / spec.sample_rate as f64;
    let mut processor = NativeFileProcessor::load(&model).unwrap();
    let mut segments = Vec::new();
    let duration = processor
        .process(
            &job,
            &mut |_| Ok(()),
            &mut |segment| {
                segments.push(segment);
                Ok(())
            },
            &|| Ok(false),
        )
        .unwrap();
    assert!(
        (duration - expected_duration).abs() < 0.1,
        "duration={duration} expected={expected_duration}"
    );
    assert!(segments.len() >= repeats * 2, "segments={}", segments.len());
    assert!(
        segments
            .windows(2)
            .all(|pair| pair[0].end_seconds <= pair[1].start_seconds)
    );
    println!(
        "five_minute_duration_s={duration:.3} segments={}",
        segments.len()
    );
    let _ = std::fs::remove_dir_all(&directory);
}
