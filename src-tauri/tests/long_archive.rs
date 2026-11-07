use gigaam_desktop_lib::live::{ArchiveFormat, LiveCapture, LiveSource};

struct TestDirectory(std::path::PathBuf);
impl Drop for TestDirectory {
    fn drop(&mut self) {
        if let Ok(entries) = std::fs::read_dir(&self.0) {
            for entry in entries.flatten() {
                if entry.path().is_file() {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let _ = std::fs::remove_dir(&self.0);
    }
}

#[test]
#[ignore = "writes a 30-minute synthetic archive; opt-in measurement"]
fn thirty_minute_archive_preserves_duration() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("gigaam-long-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let _directory_guard = TestDirectory(directory.clone());
    let mut capture = LiveCapture::new(
        "long-test".into(),
        0,
        LiveSource::Microphone,
        None,
        true,
        ArchiveFormat::Wav,
        1.0,
        directory.clone(),
        None,
    );
    capture.initialize_paths().unwrap();
    let started = std::time::Instant::now();
    for second in 0..1800u64 {
        capture.append_timed(true, 10_000_000 + second * 10_000_000, vec![0.0; 48_000]);
    }
    let archive_started = std::time::Instant::now();
    let (ack_tx, ack_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        gigaam_desktop_lib::finalization::finish_after_acknowledgement(
            || {
                ack_tx.send(()).unwrap();
            },
            || {
                release_rx.recv().unwrap();
                capture.finish().map_err(|error| error.to_string())
            },
        )
    });
    ack_rx
        .recv_timeout(std::time::Duration::from_millis(500))
        .unwrap();
    let ack_ms = archive_started.elapsed().as_millis();
    assert!(
        ack_ms < 500,
        "Stop acknowledgement must stay below the 500 ms UI target"
    );
    release_tx.send(()).unwrap();
    let paths = worker.join().unwrap().unwrap();
    let reader = hound::WavReader::open(&paths[0]).unwrap();
    assert_eq!(reader.duration(), 1800 * 48_000);
    println!(
        "synthetic_duration_s=1800 orchestration_ack_ms={ack_ms} archive_ms={} total_ms={}",
        archive_started.elapsed().as_millis(),
        started.elapsed().as_millis()
    );
    drop(reader);
    // Remove only this test's known files, never unrelated temp directories.
    for entry in std::fs::read_dir(&directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            std::fs::remove_file(path).unwrap();
        }
    }
    std::fs::remove_dir(directory).unwrap();
}
