pub mod domain;
pub mod export;
pub mod live;
pub mod model_manager;
pub mod pipeline;
pub mod queue;
pub mod storage;

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use domain::{TranscriptSegment, Transcription, TranscriptionKind, TranscriptionStatus};
use export::ExportFormat;
use live::{LiveCapture, LiveSource, LiveStatus, clamp_gain};
use model_manager::{ModelManager, ModelStatus};
use pipeline::{LiveScanner, NativeFileProcessor};
use queue::process_next;
use storage::Store;
use tauri::Manager;

static ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct WorkerHandle(Sender<()>);

struct LiveState {
    capture: Arc<Mutex<Option<Arc<Mutex<LiveCapture>>>>>,
    handle: Arc<Mutex<Option<live::platform::CaptureHandle>>>,
    inference_stop: Arc<Mutex<Option<Sender<()>>>>,
    inference_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    error: Arc<Mutex<Option<String>>>,
    model_ready: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
}

impl LiveState {
    fn new() -> Self {
        Self {
            capture: Arc::new(Mutex::new(None)),
            handle: Arc::new(Mutex::new(None)),
            inference_stop: Arc::new(Mutex::new(None)),
            inference_thread: Arc::new(Mutex::new(None)),
            error: Arc::new(Mutex::new(None)),
            model_ready: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            paused: Arc::new(AtomicBool::new(false)),
        }
    }
}

static BUILD_TAG: &str = "b20261006-25";

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Best-effort debug log for the recording lifecycle.
/// Written to `<app_data>/gigaam-debug.log`; failures are silently ignored
/// so logging can never break recording.
pub(crate) fn debug_log(path: &Path, message: &str) {
    use std::io::Write as _;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let _ = writeln!(
            file,
            "[{}.{:03}] {message}",
            now.as_secs(),
            now.subsec_millis()
        );
    }
}

pub(crate) fn debug_log_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|dir| dir.join("gigaam-debug.log"))
}

fn new_id() -> String {
    format!(
        "{}-{}-{}",
        now_ms(),
        std::process::id(),
        ID_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn lock_store<'a>(
    state: &'a tauri::State<'_, Arc<Mutex<Store>>>,
) -> Result<MutexGuard<'a, Store>, String> {
    state.lock().map_err(|_| "database lock is poisoned".into())
}

#[tauri::command]
fn enqueue_files(
    paths: Vec<String>,
    state: tauri::State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<Transcription>, String> {
    let mut queued = Vec::with_capacity(paths.len());
    for path in paths {
        let path = PathBuf::from(path);
        let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err(format!("not a file: {}", path.display()));
        }
        let created_ms = now_ms();
        let transcription = Transcription {
            id: new_id(),
            kind: TranscriptionKind::File,
            source_filename: path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| "source filename is not valid UTF-8".to_string())?
                .to_owned(),
            source_path: path.to_string_lossy().into_owned(),
            source_size_bytes: metadata.len(),
            source_modified_ms: metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_millis() as i64)
                .unwrap_or(created_ms),
            created_ms,
            started_ms: None,
            completed_ms: None,
            status: TranscriptionStatus::Queued,
            duration_seconds: None,
            model_version: "v3_e2e_ctc.int8".into(),
            error: None,
            audio_path: Some(path.to_string_lossy().into_owned()),
            completed_chunks: 0,
            total_chunks: None,
            queue_visible: true,
        };
        queued.push(transcription);
    }
    lock_store(&state)?
        .enqueue_batch(&queued)
        .map_err(|error| error.to_string())?;
    Ok(queued)
}

#[tauri::command]
fn start_transcription(
    id: String,
    state: tauri::State<'_, Arc<Mutex<Store>>>,
    worker: tauri::State<'_, WorkerHandle>,
) -> Result<(), String> {
    lock_store(&state)?
        .request_start(&id)
        .map_err(|error| error.to_string())?;
    worker.0.send(()).map_err(|error| error.to_string())
}

fn start_worker(store: Arc<Mutex<Store>>, model_dir: PathBuf) -> WorkerHandle {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut processor = None;
        while receiver.recv().is_ok() {
            if processor.is_none() {
                processor = NativeFileProcessor::load(&model_dir).ok();
            }
            if processor.is_some() {
                loop {
                    let outcome = {
                        let current = processor.as_mut().unwrap();
                        catch_unwind(AssertUnwindSafe(|| process_next(&store, current, now_ms())))
                    };
                    match outcome {
                        Ok(Ok(true)) => {}
                        Ok(Ok(false)) | Ok(Err(_)) => break,
                        Err(_) => {
                            let mut store = store.lock().unwrap();
                            let _ = store.recover_interrupted(now_ms());
                            drop(store);
                            processor = None;
                            if let Ok(reloaded) = NativeFileProcessor::load(&model_dir) {
                                processor = Some(reloaded);
                            }
                            if processor.is_some() {
                                continue;
                            }
                            break;
                        }
                    }
                }
            } else {
                let error = format!("Models are not installed in {}", model_dir.display());
                loop {
                    let job = store.lock().unwrap().claim_next(now_ms()).ok().flatten();
                    let Some(job) = job else { break };
                    let mut store = store.lock().unwrap();
                    let _ = store.transition(
                        &job.id,
                        TranscriptionStatus::Transcribing,
                        now_ms(),
                        None,
                    );
                    if matches!(
                        store.fail_if_not_cancelled(&job.id, &error, now_ms()),
                        Ok(false)
                    ) {
                        let _ = store.acknowledge_cancel(&job.id, now_ms());
                    }
                }
            }
        }
    });
    WorkerHandle(sender)
}

#[tauri::command]
fn list_queue(state: tauri::State<'_, Arc<Mutex<Store>>>) -> Result<Vec<Transcription>, String> {
    lock_store(&state)?
        .list_queue()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn list_history(state: tauri::State<'_, Arc<Mutex<Store>>>) -> Result<Vec<Transcription>, String> {
    lock_store(&state)?
        .list_history()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn search_history(
    query: String,
    state: tauri::State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<Transcription>, String> {
    lock_store(&state)?
        .search(&query)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_segments(
    id: String,
    state: tauri::State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<TranscriptSegment>, String> {
    lock_store(&state)?
        .segments(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn cancel_transcription(
    id: String,
    state: tauri::State<'_, Arc<Mutex<Store>>>,
    live: tauri::State<'_, LiveState>,
    worker: tauri::State<'_, WorkerHandle>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    lock_store(&state)?
        .request_cancel(&id, now_ms())
        .map_err(|error| error.to_string())?;
    let active_live = live
        .capture
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|capture| capture.lock().unwrap().id == id);
    if active_live {
        let _ = stop_live(state, live, worker, app)?;
    }
    Ok(())
}

#[tauri::command]
fn retry_transcription(
    id: String,
    state: tauri::State<'_, Arc<Mutex<Store>>>,
    worker: tauri::State<'_, WorkerHandle>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // A live session re-transcribes from its MIXED archive audio (both
    // tracks); reset_for_retry then turns the row into a regular file job.
    let live = lock_store(&state)?
        .get(&id)
        .map_err(|error| error.to_string())?
        .is_some_and(|record| record.kind == TranscriptionKind::Live);
    if live {
        let mixed = mixed_session_audio(id.clone(), app)?;
        lock_store(&state)?
            .set_audio_path(&id, Some(&mixed))
            .map_err(|error| error.to_string())?;
    }
    lock_store(&state)?
        .reset_for_retry(&id)
        .map_err(|error| error.to_string())?;
    worker.0.send(()).map_err(|error| error.to_string())
}

#[tauri::command]
fn remove_queue_item(id: String, state: tauri::State<'_, Arc<Mutex<Store>>>) -> Result<(), String> {
    lock_store(&state)?
        .remove_from_queue(&id, now_ms())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn delete_history(
    id: String,
    state: tauri::State<'_, Arc<Mutex<Store>>>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        return Err("invalid transcription ID".into());
    }
    let record = lock_store(&state)?
        .get(&id)
        .map_err(|error| error.to_string())?
        .ok_or("transcription not found")?;
    if matches!(
        record.status,
        TranscriptionStatus::Queued
            | TranscriptionStatus::Preparing
            | TranscriptionStatus::Transcribing
    ) {
        return Err("active transcription cannot be deleted".into());
    }
    if let Ok(data_dir) = app.path().app_data_dir() {
        let directory = data_dir.join("live").join(&id);
        if directory.exists() {
            std::fs::remove_dir_all(directory).map_err(|error| error.to_string())?;
        }
    }
    lock_store(&state)?
        .delete_history(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn export_transcription(
    id: String,
    format: String,
    state: tauri::State<'_, Arc<Mutex<Store>>>,
) -> Result<String, String> {
    let segments = lock_store(&state)?
        .segments(&id)
        .map_err(|error| error.to_string())?;
    let format = match format.as_str() {
        "txt" => ExportFormat::Txt,
        "srt" => ExportFormat::Srt,
        "vtt" => ExportFormat::Vtt,
        "json" => ExportFormat::Json,
        _ => return Err("unsupported export format".into()),
    };
    export::export(&segments, format).map_err(|error| error.to_string())
}

#[tauri::command]
fn save_export(
    id: String,
    format: String,
    path: String,
    state: tauri::State<'_, Arc<Mutex<Store>>>,
) -> Result<(), String> {
    let contents = export_transcription(id, format, state)?;
    std::fs::write(path, contents).map_err(|error| error.to_string())
}

#[tauri::command]
fn session_audio_files(
    id: String,
    state: tauri::State<'_, Arc<Mutex<Store>>>,
    app: tauri::AppHandle,
) -> Result<Vec<String>, String> {
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        return Err("invalid transcription ID".into());
    }
    let mut files = Vec::new();
    if let Ok(data_dir) = app.path().app_data_dir() {
        let directory = data_dir.join("live").join(&id);
        if let Ok(entries) = std::fs::read_dir(&directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) == Some("flac") {
                    files.push(path.to_string_lossy().into_owned());
                }
            }
            files.sort();
        }
    }
    if files.is_empty() {
        let record = lock_store(&state)?
            .get(&id)
            .map_err(|error| error.to_string())?
            .ok_or("transcription not found")?;
        if let Some(audio) = record.audio_path.filter(|path| !path.is_empty()) {
            files.push(audio);
        }
    }
    Ok(files)
}

#[tauri::command]
fn update_segment_text(
    transcription_id: String,
    index: usize,
    text: String,
    state: tauri::State<'_, Arc<Mutex<Store>>>,
) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() || text.len() > 4000 {
        return Err("segment text is empty or too long".into());
    }
    lock_store(&state)?
        .update_segment_text(&transcription_id, index, text)
        .map_err(|error| error.to_string())
}

#[derive(serde::Deserialize)]
struct TrackOffset {
    file: String,
    #[serde(default)]
    offset_seconds: f64,
}

#[tauri::command]
fn mixed_session_audio(id: String, app: tauri::AppHandle) -> Result<String, String> {
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        return Err("invalid transcription ID".into());
    }
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("live")
        .join(&id);
    let mut inputs: Vec<PathBuf> = std::fs::read_dir(&directory)
        .map_err(|error| error.to_string())?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("flac"))
        .collect();
    inputs.sort();
    if inputs.is_empty() {
        return Err("no archived audio".into());
    }
    if inputs.len() == 1 {
        return Ok(inputs[0].to_string_lossy().into_owned());
    }
    let offsets: std::collections::HashMap<String, f64> = std::fs::read_to_string(directory.join("tracks.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<TrackOffset>>(&text).ok())
        .map(|tracks| {
            tracks
                .into_iter()
                .map(|track| (track.file, track.offset_seconds.max(0.0)))
                .collect()
        })
        .unwrap_or_default();
    let tracks: Vec<(PathBuf, f64)> = inputs
        .into_iter()
        .map(|path| {
            let offset = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| offsets.get(name).copied())
                .unwrap_or(0.0);
            (path, offset)
        })
        .collect();
    let output = directory.join("mixed.wav");
    let log_path = debug_log_path(&app);
    let log = |message: &str| {
        if let Some(path) = log_path.as_ref() {
            debug_log(path, message);
        }
    };
    let newest_input = tracks
        .iter()
        .filter_map(|(path, _)| std::fs::metadata(path).ok()?.modified().ok())
        .max();
    let output_fresh = std::fs::metadata(&output)
        .ok()
        .and_then(|meta| meta.modified().ok());
    let rebuild = match (newest_input, output_fresh) {
        (Some(input), Some(output)) => input > output,
        _ => true,
    };
    if rebuild {
        if let Err(error) = pipeline::mix_mono_48k_to_wav(&tracks, &output) {
            log(&format!("mixed_session_audio failed for {id}: {error}"));
            return Err(error.to_string());
        }
        log(&format!("mixed_session_audio ok for {id}"));
    }
    Ok(output.to_string_lossy().into_owned())
}

#[tauri::command]
fn model_status(models: tauri::State<'_, ModelManager>) -> ModelStatus {
    models.status()
}

#[tauri::command]
fn download_model(
    models: tauri::State<'_, ModelManager>,
    worker: tauri::State<'_, WorkerHandle>,
) -> Result<(), String> {
    let sender = worker.0.clone();
    let (completion_tx, completion_rx) = mpsc::channel();
    models
        .start(Some(completion_tx))
        .map_err(|error| error.to_string())?;
    std::thread::spawn(move || {
        if matches!(completion_rx.recv(), Ok(true)) {
            let _ = sender.send(());
        }
    });
    Ok(())
}

#[tauri::command]
fn cancel_model_download(models: tauri::State<'_, ModelManager>) {
    models.cancel();
}

#[tauri::command]
fn start_live(
    source: LiveSource,
    microphone: String,
    keep_audio: bool,
    mic_gain: f32,
    state: tauri::State<'_, Arc<Mutex<Store>>>,
    live: tauri::State<'_, LiveState>,
    app: tauri::AppHandle,
) -> Result<LiveStatus, String> {
    if live.capture.lock().unwrap().is_some() {
        return Err("recording already active".into());
    }
    *live.error.lock().unwrap() = None;
    live.model_ready.store(false, Ordering::Relaxed);
    let log_path = debug_log_path(&app);
    if let Some(path) = log_path.as_ref() {
        if std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0) > 512 * 1024 {
            let _ = std::fs::remove_file(path);
        }
        debug_log(path, "start_live begin");
    }
    if lock_store(&state)?
        .has_active()
        .map_err(|error| error.to_string())?
    {
        return Err("another transcription is active".into());
    }
    let id = new_id();
    let started_ms = now_ms();
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("live")
        .join(&id);
    let mut capture_state = LiveCapture::new(
        id.clone(),
        started_ms,
        source,
        (!microphone.is_empty()).then_some(microphone),
        keep_audio,
        mic_gain,
        directory,
        debug_log_path(&app),
    );
    capture_state
        .initialize_paths()
        .map_err(|error| error.to_string())?;
    let capture = Arc::new(Mutex::new(capture_state));
    let transcription = Transcription {
        id,
        kind: TranscriptionKind::Live,
        source_filename: "Live session".into(),
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
        queue_visible: true,
    };
    lock_store(&state)?
        .insert_live_session(&transcription)
        .map_err(|error| error.to_string())?;
    let handle = match live::platform::start(capture.clone()) {
        Ok(handle) => handle,
        Err(error) => {
            let mut store = lock_store(&state)?;
            let _ = store.fail_if_not_cancelled(&transcription.id, &error.to_string(), now_ms());
            return Err(error.to_string());
        }
    };
    let status = capture.lock().unwrap().status();
    *live.capture.lock().unwrap() = Some(capture.clone());
    *live.handle.lock().unwrap() = Some(handle);
    if let Some(path) = log_path.as_ref() {
        debug_log(path, "capture threads spawned");
    }
    let store = state.inner().clone();
    let model_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("models");
    let (stop_tx, stop_rx) = mpsc::channel();
    let live_id = transcription.id.clone();
    let live_error = live.error.clone();
    let live_ready = live.model_ready.clone();
    let live_gen = live.generation.clone();
    let my_gen = live.generation.fetch_add(1, Ordering::Relaxed) + 1;
    let inference_thread = std::thread::spawn(move || {
        let log = |message: &str| {
            if let Some(path) = log_path.as_ref() {
                debug_log(path, message);
            }
        };
        let mut mic_scanner = match LiveScanner::new() {
            Ok(scanner) => scanner,
            Err(error) => {
                *live_error.lock().unwrap() = Some(error.to_string());
                log("inference thread: scanner init failed");
                return;
            }
        };
        let mut system_scanner = match LiveScanner::new() {
            Ok(scanner) => scanner,
            Err(error) => {
                *live_error.lock().unwrap() = Some(error.to_string());
                log("inference thread: scanner init failed");
                return;
            }
        };
        log("model load begin");
        let load_started = std::time::Instant::now();
        let Ok(mut processor) = NativeFileProcessor::load_live(&model_dir) else {
            *live_error.lock().unwrap() = Some("Live transcription model is unavailable".into());
            log("model load FAILED");
            return;
        };
        log(&format!("model load done in {}s", load_started.elapsed().as_secs()));
        live_ready.store(true, Ordering::Relaxed);
        if live_gen.load(Ordering::Relaxed) != my_gen {
            log("stale generation after load, exit");
            return;
        }
        let mut mic_emitted = 0.0;
        let mut system_emitted = 0.0;
        let mut mic_quiet = 0.0;
        let mut system_quiet = 0.0;
        let mut previous_fed = (0_usize, 0_usize);
        let mut last_stats = std::time::Instant::now();
        loop {
            if live_gen.load(Ordering::Relaxed) != my_gen {
                log("stale generation, exit");
                break;
            }
            let stopping = stop_rx.try_recv().is_ok();
            if store
                .lock()
                .unwrap()
                .is_cancelled(&live_id)
                .unwrap_or(false)
            {
                break;
            }
            let (microphone, system, mic_base, system_base) = {
                let capture = capture.lock().unwrap();
                (
                    capture.microphone.clone(),
                    capture.system.clone(),
                    capture.microphone_base_sample,
                    capture.system_base_sample,
                )
            };
            if last_stats.elapsed() > std::time::Duration::from_secs(10) {
                last_stats = std::time::Instant::now();
                let capture = capture.lock().unwrap();
                let avg = |total: u64, n: u64| {
                    if n == 0 {
                        0
                    } else {
                        total / 1000 / n
                    }
                };
                log(&format!(
                    "stats mic_fed={} sys_fed={} mic_raw={} sys_raw={} mic_fmt={} sys_fmt={} mic_pkt={} sys_pkt={} mic_it={} sys_it={} mic_ph={}/{}/{}ms+{}to sys_ph={}/{}/{}ms+{}to mic_gap_ms={} sys_gap_ms={} mic_err={} sys_err={} resyncs={}",
                    capture.microphone_base_sample + capture.microphone.len(),
                    capture.system_base_sample + capture.system.len(),
                    capture.mic_raw,
                    capture.sys_raw,
                    capture.mic_format,
                    capture.sys_format,
                    capture.mic_packets,
                    capture.sys_packets,
                    capture.mic_iters,
                    capture.sys_iters,
                    avg(capture.mic_read_us, capture.mic_iters),
                    avg(capture.mic_work_us, capture.mic_iters),
                    avg(capture.mic_lock_us, capture.mic_iters),
                    capture.mic_wait_to,
                    avg(capture.sys_read_us, capture.sys_iters),
                    avg(capture.sys_work_us, capture.sys_iters),
                    avg(capture.sys_lock_us, capture.sys_iters),
                    capture.sys_wait_to,
                    capture.mic_gap_ms,
                    capture.sys_gap_ms,
                    capture.mic_error_packets,
                    capture.sys_error_packets,
                    capture.gap_resyncs
                ));
            }
            if let Err(error) = mic_scanner.push(&microphone, mic_base) {
                *live_error.lock().unwrap() = Some(error.to_string());
                return;
            }
            if let Err(error) = system_scanner.push(&system, system_base) {
                *live_error.lock().unwrap() = Some(error.to_string());
                return;
            }
            let fed = (mic_scanner.fed_48k(), system_scanner.fed_48k());
            if stopping {
                if let Err(error) = mic_scanner
                    .finish()
                    .and_then(|()| system_scanner.finish())
                {
                    *live_error.lock().unwrap() = Some(error.to_string());
                    return;
                }
            } else if fed == previous_fed {
                std::thread::sleep(std::time::Duration::from_millis(250));
                continue;
            }
            previous_fed = fed;
            let tick_started = std::time::Instant::now();
            let mut emitted_counts = (0_usize, 0_usize);
            for (scanner, track, emitted, quiet) in [
                (
                    &mut mic_scanner,
                    domain::SourceTrack::Microphone,
                    &mut mic_emitted,
                    &mut mic_quiet,
                ),
                (
                    &mut system_scanner,
                    domain::SourceTrack::System,
                    &mut system_emitted,
                    &mut system_quiet,
                ),
            ] {
                let next_index = match store.lock().unwrap().segments(&live_id) {
                    Ok(value) => value.len(),
                    Err(error) => {
                        *live_error.lock().unwrap() = Some(error.to_string());
                        return;
                    }
                };
                match processor.transcribe_live_ready(
                    scanner,
                    &live_id,
                    track,
                    next_index,
                    *emitted,
                    *quiet,
                    stopping,
                ) {
                    Ok((segments, next_emitted, next_quiet)) => {
                        let mut storage = store.lock().unwrap();
                        let count = segments.len();
                        for segment in segments {
                            if let Err(error) = storage.append_segment(&segment) {
                                *live_error.lock().unwrap() = Some(error.to_string());
                                return;
                            }
                        }
                        if track == domain::SourceTrack::Microphone {
                            emitted_counts.0 = count;
                        } else {
                            emitted_counts.1 = count;
                        }
                        *emitted = next_emitted;
                        *quiet = next_quiet;
                    }
                    Err(error) => {
                        *live_error.lock().unwrap() = Some(error.to_string());
                        return;
                    }
                }
            }
            let tick_ms = tick_started.elapsed().as_millis();
            if stopping {
                log(&format!("final flush took {tick_ms}ms"));
            } else if tick_ms > 1500 {
                log(&format!(
                    "slow tick: {tick_ms}ms (mic+{} sys+{} segments)",
                    emitted_counts.0, emitted_counts.1
                ));
            }
            if stopping {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1000));
        }
        log("inference loop exit");
    });
    *live.inference_stop.lock().unwrap() = Some(stop_tx);
    *live.inference_thread.lock().unwrap() = Some(inference_thread);
    Ok(status)
}

#[tauri::command]
fn list_microphones() -> Result<Vec<String>, String> {
    live::platform::microphones().map_err(|error| error.to_string())
}

#[tauri::command]
fn set_mic_gain(
    mic_gain: f32,
    live: tauri::State<'_, LiveState>,
) -> Result<(), String> {
    let gain = clamp_gain(mic_gain);
    if let Some(capture) = live.capture.lock().unwrap().as_ref() {
        capture.lock().map_err(|_| "capture lock is poisoned")?.mic_gain = gain;
    }
    Ok(())
}

#[tauri::command]
fn pause_live(live: tauri::State<'_, LiveState>) -> Result<(), String> {
    if live.capture.lock().unwrap().is_none() {
        return Err("no active recording".into());
    }
    if live.handle.lock().unwrap().is_some() {
        if let Some(handle) = live.handle.lock().unwrap().take() {
            handle.stop();
        }
        live.paused.store(true, Ordering::Relaxed);
    }
    Ok(())
}

#[tauri::command]
fn resume_live(live: tauri::State<'_, LiveState>) -> Result<(), String> {
    if live.handle.lock().unwrap().is_some() {
        return Ok(());
    }
    let capture = live
        .capture
        .lock()
        .unwrap()
        .as_ref()
        .cloned()
        .ok_or("no active recording")?;
    let handle =
        live::platform::start(capture).map_err(|error| error.to_string())?;
    *live.handle.lock().unwrap() = Some(handle);
    live.paused.store(false, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
fn live_status(live: tauri::State<'_, LiveState>) -> Option<LiveStatus> {
    let mut status = live
        .capture
        .lock()
        .unwrap()
        .as_ref()
        .map(|capture| capture.lock().unwrap().status())?;
    if let Some(error) = live.error.lock().unwrap().clone() {
        status.error = Some(error);
    }
    status.model_ready = live.model_ready.load(Ordering::Relaxed);
    status.paused = live.paused.load(Ordering::Relaxed);
    Some(status)
}

#[tauri::command]
fn stop_live(
    state: tauri::State<'_, Arc<Mutex<Store>>>,
    live: tauri::State<'_, LiveState>,
    worker: tauri::State<'_, WorkerHandle>,
    app: tauri::AppHandle,
) -> Result<Vec<String>, String> {
    let log_path = debug_log_path(&app);
    let log = |message: &str| {
        if let Some(path) = log_path.as_ref() {
            debug_log(path, message);
        }
    };
    log("stop_live begin");
    // Any detached inference thread from an earlier session is stale now.
    live.generation.fetch_add(1, Ordering::Relaxed);
    live.paused.store(false, Ordering::Relaxed);
    if let Some(handle) = live.handle.lock().unwrap().take() {
        handle.stop();
    }
    log("capture stopped");
    if let Some(stop) = live.inference_stop.lock().unwrap().take() {
        let _ = stop.send(());
    }
    if let Some(thread) = live.inference_thread.lock().unwrap().take() {
        // Never block the UI longer than a few seconds on a busy inference
        // pass (e.g. stopping mid model-load or mid-chunk): after a timeout
        // the thread is detached and exits on its own via the stop signal.
        let waited = std::time::Instant::now();
        while !thread.is_finished() && waited.elapsed() < std::time::Duration::from_secs(3) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if thread.is_finished() {
            log(&format!(
                "inference joined in {}ms",
                waited.elapsed().as_millis()
            ));
            let _ = thread.join();
        } else {
            log("inference detached after 3s timeout (exits on its own)");
        }
    }
    let live_error = live.error.lock().unwrap().take();
    let capture = live
        .capture
        .lock()
        .unwrap()
        .as_ref()
        .cloned()
        .ok_or("no active recording")?;
    let mut capture = capture.lock().map_err(|_| "capture lock is poisoned")?;
    let id = capture.id.clone();
    let capture_error = capture.error.clone();
    let duration = (now_ms() - capture.started_ms) as f64 / 1000.0;
    let files = match capture.finish() {
        Ok(files) => files,
        Err(error) => {
            let mut store = lock_store(&state)?;
            if store.is_cancelled(&id).unwrap_or(false) {
                let _ = store.acknowledge_cancel(&id, now_ms());
            } else {
                let _ = store.fail_if_not_cancelled(&id, &error.to_string(), now_ms());
            }
            let _ = std::fs::remove_file(capture.directory.join("microphone.flac"));
            let _ = std::fs::remove_file(capture.directory.join("system.flac"));
            drop(capture);
            *live.capture.lock().unwrap() = None;
            let _ = worker.0.send(());
            return Err(error.to_string());
        }
    }
    .into_iter()
    .map(|path| path.to_string_lossy().into_owned())
    .collect();
    let mut store = lock_store(&state)?;
    if let Some(error) = live_error.or(capture_error) {
        if store.is_cancelled(&id).unwrap_or(false) {
            let _ = store.acknowledge_cancel(&id, now_ms());
        } else {
            let _ = store.fail_if_not_cancelled(&id, &error, now_ms());
        }
        drop(capture);
        *live.capture.lock().unwrap() = None;
        let _ = worker.0.send(());
        return Err(error);
    }
    if store.is_cancelled(&id).unwrap_or(false) {
        let _ = store.acknowledge_cancel(&id, now_ms());
        drop(capture);
        *live.capture.lock().unwrap() = None;
        let _ = worker.0.send(());
        return Ok(files);
    }
    store
        .set_duration(&id, duration)
        .map_err(|error| error.to_string())?;
    store
        .set_audio_path(&id, files.first().map(String::as_str))
        .map_err(|error| error.to_string())?;
    store
        .transition(&id, TranscriptionStatus::Completed, now_ms(), None)
        .map_err(|error| error.to_string())?;
    drop(capture);
    *live.capture.lock().unwrap() = None;
    let _ = worker.0.send(());
    log("stop_live completed");
    Ok(files)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let mut store = Store::open(data_dir.join("gigaam.sqlite3"))?;
            store.recover_interrupted(now_ms())?;
            let store = Arc::new(Mutex::new(store));
            let model_dir = data_dir.join("models");
            let worker = start_worker(store.clone(), model_dir.clone());
            worker.0.send(()).map_err(std::io::Error::other)?;
            app.manage(store);
            app.manage(worker);
            app.manage(ModelManager::new(model_dir));
            app.manage(LiveState::new());
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(&format!("GigaAM Desktop {BUILD_TAG}"));
            }
            if let Some(path) = app
                .path()
                .app_data_dir()
                .ok()
                .map(|dir| dir.join("gigaam-debug.log"))
            {
                debug_log(&path, &format!("app started {BUILD_TAG}"));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            enqueue_files,
            start_transcription,
            list_queue,
            list_history,
            search_history,
            get_segments,
            cancel_transcription,
            retry_transcription,
            remove_queue_item,
            delete_history,
            export_transcription,
            save_export,
            session_audio_files,
            mixed_session_audio,
            update_segment_text,
            start_live,
            set_mic_gain,
            pause_live,
            resume_live,
            list_microphones,
            live_status,
            stop_live,
            model_status,
            download_model,
            cancel_model_download,
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    #[test]
    fn generated_ids_are_unique() {
        let ids: HashSet<_> = (0..100).map(|_| super::new_id()).collect();
        assert_eq!(ids.len(), 100);
    }
}
