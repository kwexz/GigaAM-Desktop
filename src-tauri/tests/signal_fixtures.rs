use std::path::Path;

#[test]
fn stereo_fixture_has_independent_tracks_and_expected_duration() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/dual-tones.wav");
    let mut reader = hound::WavReader::open(path).expect("licensed signal fixture");
    assert_eq!(reader.spec().channels, 2);
    assert_eq!(reader.spec().sample_rate, 16_000);
    assert_eq!(reader.duration(), 96_000);
    let samples: Vec<i16> = reader.samples().collect::<Result<_, _>>().unwrap();
    assert!(samples.chunks_exact(2).any(|pair| pair[0] != pair[1]));
    // The second second in both channels is intentional silence.
    assert!(samples[32_000..64_000].iter().all(|sample| *sample == 0));
}

#[test]
fn malformed_fixture_is_rejected() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/corrupt.wav");
    assert!(hound::WavReader::open(path).is_err());
}
