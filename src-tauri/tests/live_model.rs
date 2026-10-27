use gigaam_desktop_lib::{domain::SourceTrack, pipeline::{LiveScanner, NativeFileProcessor}};
use std::path::PathBuf;

#[test]
#[ignore = "requires explicitly provided pinned models; run with --ignored"]
fn chunk_fed_live_retains_three_phrase_anchors_on_each_track() {
    let model = PathBuf::from(std::env::var_os("GIGAAM_TEST_MODELS").expect("set GIGAAM_TEST_MODELS"));
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/besarab-clean.wav");
    let mut reader = hound::WavReader::open(path).unwrap();
    let source: Vec<f32> = reader.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
    let audio: Vec<f32> = source.iter().flat_map(|sample| [*sample; 3]).collect();
    for (track, step) in [(SourceTrack::Microphone, 24_000), (SourceTrack::System, 48_000)] {
        let mut processor = NativeFileProcessor::load_live(&model).unwrap();
        let mut scanner = LiveScanner::new().unwrap();
        let mut emitted = 0.0;
        let mut quiet = 0.0;
        let mut all = Vec::new();
        for end in (step..audio.len() + step).step_by(step) {
            scanner.push(&audio[..end.min(audio.len())], 0).unwrap();
            let (segments, next, silence) = processor.transcribe_live_ready(&mut scanner, "fixture", track, all.len(), emitted, quiet, false).unwrap();
            all.extend(segments);
            emitted = next;
            quiet = silence;
            scanner.discard_before(emitted.max(quiet));
        }
        scanner.finish().unwrap();
        let (segments, _, _) = processor.transcribe_live_ready(&mut scanner, "fixture", track, all.len(), emitted, quiet, true).unwrap();
        all.extend(segments);
        let text = all.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ").to_lowercase();
        for phrase in ["меня зовут", "журналистика", "науку и технологии"] {
            assert!(text.contains(phrase), "{track:?} missing {phrase}: {text}");
            assert_eq!(text.matches(phrase).count(), 1, "{track:?} duplicated {phrase}: {text}");
        }
        assert!(all.iter().all(|s| s.source_track == track));
        assert!(all.windows(2).all(|s| s[0].end_seconds <= s[1].start_seconds));
        assert!(all.iter().all(|s| s.start_seconds >= 0.0 && s.end_seconds <= source.len() as f64 / 16_000.0 + 0.25));
    }
}
