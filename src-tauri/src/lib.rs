pub mod capture_recovery;
pub mod domain;
pub mod export;
pub mod google_drive;
pub mod drive_outbox;
pub mod finalization;
pub mod live;
pub mod model_manager;
pub mod pipeline;
pub mod queue;
pub mod server;
pub mod storage;
pub mod streaming;
pub mod summarization;

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

use domain::{
    TranscriptSegment, Transcription, TranscriptionKind, TranscriptionStatus,
    suppress_microphone_echo,
};
use export::ExportFormat;
use live::{ArchiveFormat, LiveCapture, LiveSource, LiveStatus, clamp_gain};
use model_manager::{ModelManager, ModelStatus};
use pipeline::{LiveScanner, NativeFileProcessor};
use queue::process_next;
use storage::Store;
use tauri::Manager;

static ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
async fn google_drive_connect(app: tauri::AppHandle) -> Result<(),String> {
    let root=app.path().app_data_dir().map_err(|_|"App data unavailable")?;
    tauri::async_runtime::spawn_blocking(move || { google_drive::connect(|url| {
        use tauri_plugin_opener::OpenerExt;
        app.opener().open_url(url, None::<String>).map_err(|_| "Could not open system browser".into())
    })?; drive_outbox::resume(&root) }).await.map_err(|_| "Google connection task failed")?
}
#[tauri::command]
async fn google_drive_disconnect() -> Result<(),String> {
    tauri::async_runtime::spawn_blocking(google_drive::disconnect).await.map_err(|_| "Google disconnect task failed")?
}
#[tauri::command]
fn google_drive_connected() -> bool { google_drive::connected() }

#[tauri::command]
async fn google_drive_queue_status(app:tauri::AppHandle)->Result<drive_outbox::QueueStatus,String>{
    let path=app.path().app_data_dir().map_err(|_|"App data unavailable")?.join("drive-outbox.sqlite");
    tauri::async_runtime::spawn_blocking(move||drive_outbox::Outbox::open(&path).and_then(|queue|queue.status()).map_err(|_|"Drive queue status unavailable".into())).await.map_err(|_|"Drive status worker failed")?
}

#[tauri::command]
async fn google_drive_prepare_folder() -> Result<String,String> {
    tauri::async_runtime::spawn_blocking(google_drive::ensure_folder).await.map_err(|_| "Google folder task failed")?
}

#[tauri::command]
async fn google_drive_upload(id: String, app: tauri::AppHandle, state: tauri::State<'_, Arc<Mutex<Store>>>) -> Result<(),String> {
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") { return Err("Invalid recording ID".into()); }
    let (record, segments, organization) = {
        let store = lock_store(&state)?;
        (store.get(&id).map_err(|e|e.to_string())?.ok_or("Recording not found")?, store.segments(&id).map_err(|e|e.to_string())?, store.load_organization().map_err(|e|e.to_string())?)
    };
    if matches!(record.status, TranscriptionStatus::Transcribing | TranscriptionStatus::Preparing) { return Err("Wait until recording/recognition completes".into()); }
    let root = app.path().app_data_dir().map_err(|_|"App data unavailable")?;
    tauri::async_runtime::spawn_blocking(move || -> Result<(),String> {
        let mut paths=Vec::new();
        if let Ok(entries)=std::fs::read_dir(root.join("live").join(&id)) {
            for entry in entries.flatten(){let path=entry.path();if matches!(path.extension().and_then(|s|s.to_str()),Some("ogg"|"wav"|"flac"|"mp3"))&&!path.file_stem().is_some_and(|s|s=="mixed"){paths.push(path);}}
        }
        paths.sort();
        if paths.is_empty(){let path=PathBuf::from(record.audio_path.as_ref().unwrap_or(&record.source_path));if path.is_file(){paths.push(path);}}
        let offsets=std::fs::read(root.join("live").join(&id).join("tracks.json")).ok().and_then(|bytes|serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        drive_outbox::enqueue_recording(&root,&id,serde_json::json!({"version":1,"recording":record,"segments":segments,"tracks":offsets,"organization":organization}),&paths)
    }).await.map_err(|_|"Drive backup worker failed")?
}

#[tauri::command]
async fn google_drive_snapshots() -> Result<Vec<google_drive::RemoteFile>,String> {
    tauri::async_runtime::spawn_blocking(google_drive::list_snapshots).await.map_err(|_|"Drive discovery worker failed")?
}

#[tauri::command]
async fn google_drive_restore(file_id:String, app:tauri::AppHandle, state:tauri::State<'_,Arc<Mutex<Store>>>) -> Result<String,String> {
    let store=state.inner().clone();
    let root=app.path().app_data_dir().map_err(|_|"App data unavailable")?;
    tauri::async_runtime::spawn_blocking(move || -> Result<String,String> {
        if let Some(id)=store.lock().map_err(|_|"Store unavailable")?.restored_snapshot(&file_id).map_err(|_|"Restore lookup failed")? {return Ok(id);}
        // Always restore as a separate stable local copy; never overwrite edits.
        let id=new_id(); let directory=root.join("live").join(&id);
        std::fs::create_dir_all(&directory).map_err(|_|"Restore directory unavailable")?;
        let manifest=directory.join("manifest.json");
        google_drive::download_file(&file_id,&manifest,8*1024*1024)?;
        let value:serde_json::Value=serde_json::from_slice(&std::fs::read(&manifest).map_err(|_|"Manifest unavailable")?).map_err(|_|"Invalid backup manifest")?;
        if value["version"]!=1{return Err("Unsupported backup version".into());}
        let mut recording:Transcription=serde_json::from_value(value["recording"].clone()).map_err(|_|"Invalid recording metadata")?;
        let segments:Vec<TranscriptSegment>=serde_json::from_value(value["segments"].clone()).map_err(|_|"Invalid transcript")?;
        let audio=value["audio"].as_array().ok_or("Backup audio missing")?;
        if audio.is_empty()||audio.len()>8{return Err("Invalid backup track count".into());}
        let mut paths=Vec::new();
        for (index,track) in audio.iter().enumerate(){
            let name=track["original_filename"].as_str().ok_or("Track filename missing")?;
            if name.contains(['/', '\\'])||name.contains("..")||name.len()>240{return Err("Unsafe backup filename".into());}
            let extension=Path::new(name).extension().and_then(|s|s.to_str()).ok_or("Track extension missing")?;
            if !["ogg","wav","flac","mp3","webm","m4a"].contains(&extension){return Err("Unsupported backup audio".into());}
            let path=directory.join(format!("restored-{index}.{extension}"));
            google_drive::download_file(track["file_id"].as_str().ok_or("Track ID missing")?,&path,512*1024*1024)?;
            use sha2::{Digest,Sha256};use std::io::Read;
            let expected=track["sha256"].as_str().ok_or("Legacy backup has no integrity hash; cannot safely restore")?;
            let mut file=std::fs::File::open(&path).map_err(|_|"Downloaded audio unavailable")?;let mut hash=Sha256::new();let mut buffer=[0u8;65536];let mut size=0u64;
            loop{let n=file.read(&mut buffer).map_err(|_|"Restore checksum failed")?;if n==0{break;}size+=n as u64;hash.update(&buffer[..n]);}
            if track["bytes"].as_u64()!=Some(size)||format!("{:x}",hash.finalize())!=expected{return Err("Restored audio integrity mismatch".into());}
            paths.push(path);
        }
        if let Some(tracks)=value["tracks"].as_array(){
            let mut restored=Vec::new();
            for track in tracks {
                let name=track["file"].as_str().ok_or("Invalid track offset filename")?;
                let offset=track["offset_seconds"].as_f64().ok_or("Invalid track offset")?;
                if !offset.is_finite()||offset<0.0{return Err("Invalid track offset".into());}
                let index=audio.iter().position(|item|item["original_filename"].as_str()==Some(name)).ok_or("Offset references missing track")?;
                restored.push(serde_json::json!({"file":paths[index].file_name().and_then(|s|s.to_str()),"offset_seconds":offset}));
            }
            std::fs::write(directory.join("tracks.json"),serde_json::to_vec(&restored).map_err(|_|"Offset serialization failed")?).map_err(|_|"Offset restore failed")?;
        }
        recording.id=id.clone();recording.source_filename=format!("{} (Drive)",recording.source_filename);
        recording.source_path=paths[0].to_string_lossy().into();recording.audio_path=Some(recording.source_path.clone());
        recording.status=TranscriptionStatus::Completed;recording.queue_visible=false;
        let mut store=store.lock().map_err(|_|"Store unavailable")?;
        let segments:Vec<_>=segments.into_iter().enumerate().map(|(index,mut segment)|{segment.transcription_id=id.clone();segment.id=format!("{id}:{index}");segment.index=index;segment}).collect();
        store.restore_snapshot_organized(&file_id,&recording,&segments,&value).map_err(|_|"Could not atomically restore recording".into())
    }).await.map_err(|_|"Drive restore worker failed")?
}

struct WorkerHandle(Sender<()>);

struct LiveState {
    lifecycle: Mutex<()>,
    capture: Arc<Mutex<Option<Arc<Mutex<LiveCapture>>>>>,
    handle: Arc<Mutex<Option<live::platform::CaptureHandle>>>,
    inference_stop: Arc<Mutex<Option<Sender<()>>>>,
    inference_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    error: Arc<Mutex<Option<String>>>,
    model_ready: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
    finalizing: AtomicBool,
    finalization_cancel: Mutex<Option<(String, Arc<AtomicBool>)>>,
}

impl LiveState {
    fn new() -> Self {
        Self {
            lifecycle: Mutex::new(()),
            capture: Arc::new(Mutex::new(None)),
            handle: Arc::new(Mutex::new(None)),
            inference_stop: Arc::new(Mutex::new(None)),
            inference_thread: Arc::new(Mutex::new(None)),
            error: Arc::new(Mutex::new(None)),
            model_ready: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            paused: Arc::new(AtomicBool::new(false)),
            finalizing: AtomicBool::new(false),
            finalization_cancel: Mutex::new(None),
        }
    }
}

static BUILD_TAG: &str = "b20261007-26";

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn live_timestamp(now_ms: i64) -> String {
    // Convert Unix time to UTC calendar components without pulling a date-time
    // dependency into the desktop binary.
    let seconds = now_ms.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let hour = seconds_of_day / 3_600;
    let minute = seconds_of_day / 60 % 60;
    let second = seconds_of_day % 60;
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
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
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("gigaam-debug.log"))
}

fn new_id() -> String {
    format!(
        "{}-{}-{}",
        now_ms(),
        std::process::id(),
        ID_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn new_live_id(now: i64) -> String {
    let timestamp = live_timestamp(now);
    let sequence = ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    if sequence == 0 {
        timestamp
    } else {
        format!("{timestamp}-{sequence}")
    }
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
fn load_organization(state: tauri::State<'_, Arc<Mutex<Store>>>) -> Result<serde_json::Value, String> {
    lock_store(&state)?.load_organization().map_err(|error| error.to_string())
}

#[tauri::command]
fn save_organization(nodes: serde_json::Value, previous: serde_json::Value, state: tauri::State<'_, Arc<Mutex<Store>>>) -> Result<(), String> {
    let mut store = lock_store(&state)?;
    store.save_organization_if_unchanged(previous, nodes)
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
    let segments = lock_store(&state)?
        .segments(&id)
        .map_err(|error| error.to_string())?;
    Ok(suppress_microphone_echo(segments))
}

#[tauri::command]
async fn cancel_transcription(
    id: String,
    state: tauri::State<'_, Arc<Mutex<Store>>>,
    live: tauri::State<'_, LiveState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    lock_store(&state)?
        .request_cancel(&id, now_ms())
        .map_err(|error| error.to_string())?;
    if let Some((session, cancel)) = live.finalization_cancel.lock().unwrap().as_ref() {
        if session == &id {
            cancel.store(true, Ordering::Release);
        }
    }
    let active_capture = live.capture.lock().unwrap().as_ref().cloned();
    let active_live = active_capture.is_some_and(|capture| capture.lock().unwrap().id == id);
    if active_live {
        if !live.finalizing.load(Ordering::SeqCst) {
            let _ = stop_live(app).await?;
        }
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
    let segments = suppress_microphone_echo(segments);
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
    let record = lock_store(&state)?
        .get(&id)
        .map_err(|error| error.to_string())?
        .ok_or("transcription not found")?;
    let mut files = Vec::new();
    if let Ok(data_dir) = app.path().app_data_dir() {
        let directory = data_dir.join("live").join(&id);
        if let Ok(entries) = std::fs::read_dir(&directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("flac" | "wav" | "mp3" | "ogg")
                ) && path.file_stem().and_then(|stem| stem.to_str()) != Some("mixed")
                {
                    files.push(path.to_string_lossy().into_owned());
                }
            }
            files.sort();
        }
    }
    if files.is_empty() {
        if let Some(audio) = record.audio_path.clone().filter(|path| !path.is_empty()) {
            files.push(audio);
        }
    }
    if files.is_empty() && !record.source_path.is_empty() && Path::new(&record.source_path).exists()
    {
        files.push(record.source_path);
    }
    Ok(files)
}

#[tauri::command]
fn clear_transcript(id: String, state: tauri::State<'_, Arc<Mutex<Store>>>) -> Result<(), String> {
    lock_store(&state)?.clear_transcript(&id).map_err(|error| error.to_string())
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

#[tauri::command]
fn rename_transcription(
    id: String,
    name: String,
    state: tauri::State<'_, Arc<Mutex<Store>>>,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 240 || name.contains(['\r', '\n']) {
        return Err("name is empty or too long".into());
    }
    lock_store(&state)?
        .rename(&id, name)
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
        .filter(|path| {
            matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("flac" | "wav" | "mp3" | "ogg")
            ) && path.file_stem().and_then(|stem| stem.to_str()) != Some("mixed")
        })
        .collect();
    inputs.sort();
    if inputs.is_empty() {
        return Err("no archived audio".into());
    }
    if inputs.len() == 1 {
        return Ok(inputs[0].to_string_lossy().into_owned());
    }
    let offsets: std::collections::HashMap<String, f64> =
        std::fs::read_to_string(directory.join("tracks.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Vec<TrackOffset>>(&text).ok())
            .map(|tracks| {
                tracks
                    .into_iter()
                    .map(|track| (track.file, track.offset_seconds.max(0.0)))
                    .collect()
            })
            .unwrap_or_default();
    let extension = inputs
        .first()
        .and_then(|path| path.extension())
        .and_then(|extension| extension.to_str())
        .unwrap_or("wav")
        .to_ascii_lowercase();
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
    let output = directory.join(format!("mixed.{extension}"));
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
        let format = match extension.as_str() {
            "flac" => ArchiveFormat::Flac,
            "mp3" => ArchiveFormat::Mp3,
            "ogg" => ArchiveFormat::Ogg,
            _ => ArchiveFormat::Wav,
        };
        let raw = output.with_extension("mix.raw.partial");
        let result = pipeline::mix_mono_48k_to_raw(&tracks, &raw)
            .and_then(|_| live::encode_raw_48k(&raw, &output, format));
        let _ = std::fs::remove_file(&raw);
        if let Err(error) = result {
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
    archive_format: Option<ArchiveFormat>,
    live_name: Option<String>,
    mic_gain: f32,
    state: tauri::State<'_, Arc<Mutex<Store>>>,
    live: tauri::State<'_, LiveState>,
    app: tauri::AppHandle,
) -> Result<LiveStatus, String> {
    let _lifecycle = live
        .lifecycle
        .try_lock()
        .map_err(|_| "live lifecycle transition in progress")?;
    if live.finalizing.load(Ordering::SeqCst) {
        return Err("previous recording is finalizing".into());
    }
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
    let started_ms = now_ms();
    let id = new_live_id(started_ms);
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
        true,
        archive_format.unwrap_or_default(),
        mic_gain,
        directory,
        debug_log_path(&app),
    );
    capture_state
        .initialize_paths()
        .map_err(|error| error.to_string())?;
    let capture = Arc::new(Mutex::new(capture_state));
    let source_filename = live_name
        .filter(|name| {
            !name.trim().is_empty() && name.chars().count() <= 240 && !name.contains(['\r', '\n'])
        })
        .unwrap_or_else(|| format!("Live session {}", live_timestamp(started_ms)));
    let transcription = Transcription {
        id,
        kind: TranscriptionKind::Live,
        source_filename,
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
        log(&format!(
            "model load done in {}s",
            load_started.elapsed().as_secs()
        ));
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
                    if n == 0 { 0 } else { total / 1000 / n }
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
            let fed = (mic_scanner.fed_input(), system_scanner.fed_input());
            if stopping {
                if let Err(error) = mic_scanner.finish().and_then(|()| system_scanner.finish()) {
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
                if (track == domain::SourceTrack::Microphone && microphone.is_empty())
                    || (track == domain::SourceTrack::System && system.is_empty())
                {
                    continue;
                }
                let next_index = match store.lock().unwrap().segments(&live_id) {
                    Ok(value) => value.len(),
                    Err(error) => {
                        *live_error.lock().unwrap() = Some(error.to_string());
                        return;
                    }
                };
                match processor.transcribe_live_ready(
                    scanner, &live_id, track, next_index, *emitted, *quiet, stopping,
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
fn set_mic_gain(mic_gain: f32, live: tauri::State<'_, LiveState>) -> Result<(), String> {
    let gain = clamp_gain(mic_gain);
    if let Some(capture) = live.capture.lock().unwrap().as_ref() {
        capture
            .lock()
            .map_err(|_| "capture lock is poisoned")?
            .mic_gain = gain;
    }
    Ok(())
}

#[tauri::command]
fn pause_live(live: tauri::State<'_, LiveState>) -> Result<(), String> {
    let _lifecycle = live
        .lifecycle
        .try_lock()
        .map_err(|_| "live lifecycle transition in progress")?;
    if live.finalizing.load(Ordering::SeqCst) {
        return Err("recording is finalizing".into());
    }
    if live.capture.lock().unwrap().is_none() {
        return Err("no active recording".into());
    }
    if let Some(handle) = live.handle.lock().unwrap().take() {
        handle.stop();
        live.paused.store(true, Ordering::Relaxed);
    }
    Ok(())
}

#[tauri::command]
fn resume_live(live: tauri::State<'_, LiveState>) -> Result<(), String> {
    let _lifecycle = live
        .lifecycle
        .try_lock()
        .map_err(|_| "live lifecycle transition in progress")?;
    if live.finalizing.load(Ordering::SeqCst) {
        return Err("recording is finalizing".into());
    }
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
    let handle = live::platform::start(capture).map_err(|error| error.to_string())?;
    *live.handle.lock().unwrap() = Some(handle);
    live.paused.store(false, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
fn live_status(live: tauri::State<'_, LiveState>) -> Option<LiveStatus> {
    let capture = live.capture.lock().unwrap().as_ref().cloned()?;
    let mut status = capture.lock().unwrap().status();
    if let Some(error) = live.error.lock().unwrap().clone() {
        status.error = Some(error);
    }
    status.model_ready = live.model_ready.load(Ordering::Relaxed);
    status.paused = live.paused.load(Ordering::Relaxed);
    Some(status)
}

#[tauri::command]
fn live_finalizing(live: tauri::State<'_, LiveState>) -> bool {
    live.finalizing.load(Ordering::SeqCst)
}

#[tauri::command]
async fn stop_live(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    let stop_requested = std::time::Instant::now();
    // Native joins and archive encoding must never execute on the webview
    // command thread. The session remains guarded until durable completion.
    let (acknowledge, acknowledgement) = tokio::sync::oneshot::channel();
    tauri::async_runtime::spawn_blocking(move || {
        let mut acknowledge = Some(acknowledge);
        let result = (|| -> Result<Vec<String>, String> {
            let live = app.state::<LiveState>();
            let lifecycle = live
                .lifecycle
                .try_lock()
                .map_err(|_| "live lifecycle transition in progress")?;
            let _guard = finalization::FinalizationClaim::acquire(&live.finalizing)
                .ok_or("recording is already finalizing")?;
            let capture = live
                .capture
                .lock()
                .map_err(|_| "capture lock poisoned")?
                .as_ref()
                .cloned()
                .ok_or("no active recording")?;
            let id = capture
                .lock()
                .map_err(|_| "capture lock poisoned")?
                .id
                .clone();
            let cancel = capture
                .lock()
                .map_err(|_| "capture lock poisoned")?
                .finalization_cancel
                .clone();
            *live.finalization_cancel.lock().unwrap() = Some((id.clone(), cancel.clone()));
            if app
                .state::<Arc<Mutex<Store>>>()
                .lock()
                .map_err(|_| "store lock poisoned")?
                .is_cancelled(&id)
                .map_err(|error| error.to_string())?
            {
                cancel.store(true, Ordering::Release);
            }
            drop(lifecycle);
            // Stop device ingress even if writing finalization metadata fails.
            // Otherwise an error could leave an invisible recording running.
            if let Some(handle) = live.handle.lock().unwrap().as_ref() {
                handle.request_stop();
            }
            let persistence_start = app
                .state::<Arc<Mutex<Store>>>()
                .lock()
                .map_err(|_| "store lock poisoned")?
                .begin_live_finalization(&id, now_ms())
                .map_err(|error| error.to_string());
            let result = finalization::finish_after_acknowledgement(
                || {
                    if let Some(path) = debug_log_path(&app) {
                        debug_log(
                            &path,
                            &format!(
                                "stop_ack_ms={} session={id} persistence_started={}",
                                stop_requested.elapsed().as_millis(),
                                persistence_start.is_ok()
                            ),
                        );
                    }
                    // Device ingress is already stopping, so the UI may regain control even
                    // when the finalization marker could not be written. The background
                    // finalizer still attempts the archive and reports/persists its error.
                    if let Some(sender) = acknowledge.take() {
                        let _ = sender.send(Ok(Vec::new()));
                    }
                },
                || {
                    stop_live_blocking(
                        app.state::<Arc<Mutex<Store>>>(),
                        app.state::<LiveState>(),
                        app.state::<WorkerHandle>(),
                        app.clone(),
                    )
                },
            );
            if let Err(error) = &result {
                // No new session can start while this claim is held. Clear only
                // the failed session, recovering a poisoned slot for future use.
                {
                    let mut slot = live
                        .capture
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if slot.as_ref().is_some_and(|capture| {
                        capture
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .id
                            == id
                    }) {
                        slot.take();
                    }
                }
                live.capture.clear_poison();
                *live
                    .error
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error.clone());
                if let Ok(mut store) = app.state::<Arc<Mutex<Store>>>().lock() {
                    let _ = store.fail_if_not_cancelled(&id, error, now_ms());
                }
            }
            let status_result = app
                .state::<Arc<Mutex<Store>>>()
                .lock()
                .map_err(|_| "store lock poisoned")?
                .finish_live_finalization(&id, result.is_ok(), now_ms())
                .map_err(|error| error.to_string());
            *live.finalization_cancel.lock().unwrap() = None;
            status_result?;
            if let Some(path) = debug_log_path(&app) {
                debug_log(
                    &path,
                    &format!(
                        "stop_finalization_ms={} session={id}",
                        stop_requested.elapsed().as_millis()
                    ),
                );
            }
            persistence_start?;
            result
        })();
        if let Some(sender) = acknowledge.take() {
            let _ = sender.send(result.clone());
        }
        if let Err(error) = result {
            if let Some(path) = debug_log_path(&app) {
                debug_log(&path, &format!("Live finalization failed: {error}"));
            }
        }
    });
    acknowledgement
        .await
        .map_err(|_| "Live finalizer stopped before acknowledging capture shutdown".to_string())?
}

fn stop_live_blocking(
    state: tauri::State<'_, Arc<Mutex<Store>>>,
    live: tauri::State<'_, LiveState>,
    worker: tauri::State<'_, WorkerHandle>,
    app: tauri::AppHandle,
) -> Result<Vec<String>, String> {
    struct NotifyWorkerOnExit(Sender<()>);
    impl Drop for NotifyWorkerOnExit {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    let _notify = NotifyWorkerOnExit(worker.0.clone());
    let log_path = debug_log_path(&app);
    let log = |message: &str| {
        if let Some(path) = log_path.as_ref() {
            debug_log(path, message);
        }
    };
    log("stop_live begin");
    live.paused.store(false, Ordering::Relaxed);
    if let Some(handle) = live.handle.lock().unwrap().take() {
        handle.stop();
    }
    log("capture stopped");
    if let Some(stop) = live.inference_stop.lock().unwrap().take() {
        let _ = stop.send(());
    }
    if let Some(thread) = live.inference_thread.lock().unwrap().take() {
        // Completion is reported only after the final inference flush. The
        // previous three-second detach left live records visibly incomplete.
        let waited = std::time::Instant::now();
        if thread.join().is_err() {
            *live.error.lock().unwrap() =
                Some("Live inference worker panicked; audio archive preserved".into());
        }
        log(&format!(
            "inference final flush joined in {}ms",
            waited.elapsed().as_millis()
        ));
    }
    let live_error = live.error.lock().unwrap().take();
    let capture = live
        .capture
        .lock()
        .unwrap()
        .take()
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
            // LiveCapture owns exact staged-output rollback. Do not delete
            // legacy filenames here: they may be valid recovery archives.
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
    if let Err(error) =
        store.complete_live_archive(&id, duration, files.first().map(String::as_str), now_ms())
    {
        let message = format!("Audio archive saved but final metadata commit failed: {error}");
        let _ = store.fail_if_not_cancelled(&id, &message, now_ms());
        let _ = worker.0.send(());
        return Err(message);
    }
    if let Err(error) = capture.cleanup_committed_raw() {
        log(&format!("committed archive raw cleanup deferred: {error}"));
    }
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
            if let Ok(root)=app.path().app_data_dir() { drive_outbox::start_worker(root); }
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
            google_drive_connect,
            google_drive_disconnect,
            google_drive_connected,
            google_drive_queue_status,
            google_drive_prepare_folder,
            google_drive_upload,
            google_drive_snapshots,
            google_drive_restore,
            load_organization,
            save_organization,
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
            clear_transcript,
            rename_transcription,
            start_live,
            set_mic_gain,
            pause_live,
            resume_live,
            list_microphones,
            live_status,
            stop_live,
            live_finalizing,
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

    #[test]
    fn live_timestamp_has_filename_safe_utc_shape() {
        assert_eq!(super::live_timestamp(0), "19700101-000000");
        assert_eq!(super::live_timestamp(1_700_000_000_000), "20231114-221320");
    }
}
