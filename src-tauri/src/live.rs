use std::{
    fs,
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use crate::debug_log;

use anyhow::{Context, Result};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use flac_codec::{
    byteorder::LittleEndian,
    encode::{FlacByteWriter, Options},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveSource {
    Microphone,
    System,
    SystemAndMicrophone,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveFormat {
    Flac,
    Wav,
    Mp3,
}

impl Default for ArchiveFormat {
    fn default() -> Self {
        Self::Flac
    }
}

impl ArchiveFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Flac => "flac",
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
        }
    }
}

fn archive_timestamp(now_ms: i64) -> String {
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

#[derive(Clone, Debug, Serialize)]
pub struct LiveStatus {
    pub recording: bool,
    pub started_ms: i64,
    pub source: LiveSource,
    pub microphone_level: f32,
    pub system_level: f32,
    pub error: Option<String>,
    pub model_ready: bool,
    pub mic_format: String,
    pub sys_format: String,
    pub paused: bool,
    pub microphone_capture_state: String,
    pub system_capture_state: String,
}

pub fn clamp_gain(gain: f32) -> f32 {
    if gain.is_finite() {
        gain.clamp(0.25, 12.0)
    } else {
        1.0
    }
}

pub struct LiveCapture {
    pub id: String,
    pub started_ms: i64,
    pub source: LiveSource,
    pub microphone_device: Option<String>,
    pub keep_audio: bool,
    pub archive_format: ArchiveFormat,
    pub mic_gain: f32,
    pub directory: PathBuf,
    pub microphone_path: PathBuf,
    pub system_path: PathBuf,
    // Archive files stay open for the whole session: opening/closing them
    // per audio packet stalls the capture thread under disk pressure and
    // shreds the recording with gaps.
    microphone_file: Option<BufWriter<fs::File>>,
    system_file: Option<BufWriter<fs::File>>,
    pub microphone: Vec<f32>,
    pub system: Vec<f32>,
    pub gap_resyncs: u64,
    pub mic_gap_ms: u64,
    pub sys_gap_ms: u64,
    pub mic_error_packets: u64,
    pub sys_error_packets: u64,
    pub mic_packets: u64,
    pub sys_packets: u64,
    pub mic_raw: u64,
    pub sys_raw: u64,
    pub mic_start_offset: Option<f64>,
    pub sys_start_offset: Option<f64>,
    pub mic_iters: u64,
    pub sys_iters: u64,
    pub mic_read_us: u64,
    pub sys_read_us: u64,
    pub mic_work_us: u64,
    pub sys_work_us: u64,
    pub mic_lock_us: u64,
    pub sys_lock_us: u64,
    pub mic_wait_to: u64,
    pub sys_wait_to: u64,
    pub mic_format: String,
    pub sys_format: String,
    pub log_path: Option<PathBuf>,
    pub timeline_origin_100ns: Option<u64>,
    pub microphone_capture_state: String,
    pub system_capture_state: String,
    pub microphone_base_sample: usize,
    pub system_base_sample: usize,
    archive_error: Option<String>,
    pub finalization_cancel: Arc<AtomicBool>,
    pub error: Option<String>,
}

impl LiveCapture {
    pub fn new(
        id: String,
        started_ms: i64,
        source: LiveSource,
        microphone_device: Option<String>,
        keep_audio: bool,
        archive_format: ArchiveFormat,
        mic_gain: f32,
        directory: PathBuf,
        log_path: Option<PathBuf>,
    ) -> Self {
        Self {
            id,
            started_ms,
            source,
            microphone_device,
            keep_audio,
            archive_format,
            mic_gain: clamp_gain(mic_gain),
            directory,
            microphone_path: PathBuf::new(),
            system_path: PathBuf::new(),
            microphone: Vec::new(),
            system: Vec::new(),
            microphone_file: None,
            system_file: None,
            gap_resyncs: 0,
            mic_gap_ms: 0,
            sys_gap_ms: 0,
            mic_error_packets: 0,
            sys_error_packets: 0,
            mic_packets: 0,
            sys_packets: 0,
            mic_raw: 0,
            sys_raw: 0,
            mic_start_offset: None,
            sys_start_offset: None,
            mic_iters: 0,
            sys_iters: 0,
            mic_read_us: 0,
            sys_read_us: 0,
            mic_work_us: 0,
            sys_work_us: 0,
            mic_lock_us: 0,
            sys_lock_us: 0,
            mic_wait_to: 0,
            sys_wait_to: 0,
            mic_format: String::new(),
            sys_format: String::new(),
            log_path,
            timeline_origin_100ns: None,
            microphone_capture_state: "starting".into(),
            system_capture_state: "starting".into(),
            microphone_base_sample: 0,
            system_base_sample: 0,
            error: None,
            archive_error: None,
            finalization_cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn status(&self) -> LiveStatus {
        LiveStatus {
            recording: true,
            started_ms: self.started_ms,
            source: self.source,
            microphone_level: rms_tail(&self.microphone),
            system_level: rms_tail(&self.system),
            error: self.error.clone(),
            model_ready: false,
            mic_format: self.mic_format.clone(),
            sys_format: self.sys_format.clone(),
            paused: false,
            microphone_capture_state: self.microphone_capture_state.clone(),
            system_capture_state: self.system_capture_state.clone(),
        }
    }

    pub fn set_error(&mut self, error: impl ToString) {
        self.error = Some(error.to_string());
    }

    pub fn finish(&mut self) -> Result<Vec<PathBuf>> {
        if self.finalization_cancel.load(Ordering::Acquire) { anyhow::bail!("Live finalization cancelled; raw audio retained"); }
        if let Some(error) = &self.archive_error {
            anyhow::bail!("Audio archive integrity compromised; raw material retained: {error}");
        }
        if !self.keep_audio {
            return Ok(Vec::new());
        }
        // Flush and close before transcoding so the reader sees everything.
        drop(self.microphone_file.take());
        drop(self.system_file.take());
        fs::create_dir_all(&self.directory)?;
        struct UncommittedArchives {
            paths: Vec<PathBuf>,
            committed: bool,
        }
        impl Drop for UncommittedArchives {
            fn drop(&mut self) {
                if !self.committed {
                    for path in &self.paths { let _ = fs::remove_file(path); }
                }
            }
        }
        let mut archive_guard = UncommittedArchives { paths: Vec::new(), committed: false };
        let mut files = Vec::new();
        let timestamp = archive_timestamp(self.started_ms);
        if self.microphone_path.exists() {
            let path = self.directory.join(format!(
                "microphone-{timestamp}.{}",
                self.archive_format.extension()
            ));
            transcode_raw_cancellable(&self.microphone_path, &path, self.archive_format, Some(&self.finalization_cancel))?;
            archive_guard.paths.push(path.clone());
            files.push(path);
        }
        if self.system_path.exists() {
            let path = self.directory.join(format!(
                "system-{timestamp}.{}",
                self.archive_format.extension()
            ));
            transcode_raw_cancellable(&self.system_path, &path, self.archive_format, Some(&self.finalization_cancel))?;
            archive_guard.paths.push(path.clone());
            files.push(path);
        }
        check_archive_cancel(Some(&self.finalization_cancel))?;
        // Track start offsets (seconds from the session origin) so the
        // archived tracks can later be mixed back together chronologically.
        // Sessions predating this file mix from zero, as before.
        let mut tracks = Vec::new();
        let microphone_file = format!("microphone-{timestamp}.{}", self.archive_format.extension());
        let system_file = format!("system-{timestamp}.{}", self.archive_format.extension());
        for (file, offset) in [
            (microphone_file.as_str(), self.mic_start_offset),
            (system_file.as_str(), self.sys_start_offset),
        ] {
            if self.directory.join(file).exists() {
                tracks.push(serde_json::json!({
                    "file": file,
                    "offset_seconds": offset.unwrap_or(0.0).max(0.0),
                }));
            }
        }
        if !tracks.is_empty() {
            let manifest = self.directory.join("tracks.json");
            let staged = self.directory.join("tracks.json.partial");
            archive_guard.paths.push(staged.clone());
            fs::write(&staged, serde_json::to_string_pretty(&tracks)?)?;
            fs::OpenOptions::new().write(true).open(&staged)?.sync_all()?;
            check_archive_cancel(Some(&self.finalization_cancel))?;
            // A pre-existing manifest may describe recovery material. Do not
            // replace it silently during retry/finalization.
            if manifest.exists() { anyhow::bail!("Track manifest already exists; recovery material retained"); }
            fs::rename(&staged, &manifest)?;
            archive_guard.paths.push(manifest);
        }
        check_archive_cancel(Some(&self.finalization_cancel))?;
        archive_guard.committed = true;
        // Raw material is intentionally retained until the caller has
        // durably committed archive metadata. Never delete it in preparation.
        Ok(files)
    }

    pub fn cleanup_committed_raw(&self) -> Result<()> {
        for path in [&self.microphone_path, &self.system_path] {
            if path.is_file() { fs::remove_file(path)?; }
        }
        Ok(())
    }

    pub fn initialize_paths(&mut self) -> Result<()> {
        if !self.keep_audio {
            return Ok(());
        }
        fs::create_dir_all(&self.directory)?;
        self.microphone_path = self.directory.join("microphone.f32");
        self.system_path = self.directory.join("system.f32");
        Ok(())
    }

    fn append_archive(&mut self, microphone: bool, samples: &[f32]) -> Result<()> {
        if !self.keep_audio || samples.is_empty() {
            return Ok(());
        }
        // Lazily (but once): tracks that never receive audio leave no files.
        let (writer, path) = if microphone {
            (&mut self.microphone_file, &self.microphone_path)
        } else {
            (&mut self.system_file, &self.system_path)
        };
        if writer.is_none() {
            *writer = Some(BufWriter::new(fs::File::create(path).with_context(
                || format!("archive: cannot create {}", path.display()),
            )?));
        }
        let writer = writer.as_mut().expect("archive writer just created");
        for sample in samples {
            writer.write_all(&sample.to_le_bytes())?;
        }
        Ok(())
    }

    pub fn append_timed(&mut self, microphone: bool, timestamp_100ns: u64, samples: Vec<f32>) {
        if !samples.is_empty() {
            if microphone {
                self.mic_packets += 1;
            } else {
                self.sys_packets += 1;
            }
        }
        let first_packet =
            (microphone && self.mic_packets == 1) || (!microphone && self.sys_packets == 1);
        if first_packet {
            if let Some(path) = self.log_path.clone() {
                debug_log(
                    &path,
                    if microphone {
                        "mic stream flowing"
                    } else {
                        "sys stream flowing"
                    },
                );
            }
        }
        let samples: Vec<f32> = if microphone && self.mic_gain != 1.0 {
            samples
                .into_iter()
                .map(|sample| (sample * self.mic_gain).clamp(-1.0, 1.0))
                .collect()
        } else {
            samples
        };
        if samples.is_empty() {
            return;
        }
        // Sequential timeline, like every proven recorder: append whatever
        // arrived, in arrival order. Device timestamps only DIAGNOSE jumps
        // (counters below); they never reshape, pad, or drop audio, so a
        // lying clock cannot shred the recording into "gurgling".
        // The first packet of each track additionally fixes its start offset
        // from the session origin, so archive tracks can be mixed back
        // together chronologically later (see tracks.json in finish()).
        if timestamp_100ns != 0 {
            let origin = match self.timeline_origin_100ns {
                Some(origin) => origin,
                None => {
                    self.timeline_origin_100ns = Some(timestamp_100ns);
                    timestamp_100ns
                }
            };
            if timestamp_100ns >= origin {
                let offset = (timestamp_100ns - origin) as f64 / 10_000_000.0;
                if microphone {
                    self.mic_start_offset.get_or_insert(offset);
                } else {
                    self.sys_start_offset.get_or_insert(offset);
                }
                let start = (offset * 48_000.0) as usize;
                let end = if microphone {
                    self.microphone_base_sample + self.microphone.len()
                } else {
                    self.system_base_sample + self.system.len()
                };
                if start > end {
                    let gap_ms = ((start - end) * 1000 / 48_000) as u64;
                    if microphone {
                        self.mic_gap_ms += gap_ms;
                    } else {
                        self.sys_gap_ms += gap_ms;
                    }
                    if start - end > 48_000 / 2 {
                        self.gap_resyncs += 1;
                    }
                }
            }
        }
        if let Err(error) = self.append_archive(microphone, &samples) {
            self.archive_error = Some(error.to_string());
            self.error = Some(error.to_string());
            return;
        }
        if microphone {
            self.microphone.extend_from_slice(&samples);
        } else {
            self.system.extend_from_slice(&samples);
        }
        const MAX_MEMORY_SAMPLES: usize = 48_000 * 35;
        let (track, base) = if microphone {
            (&mut self.microphone, &mut self.microphone_base_sample)
        } else {
            (&mut self.system, &mut self.system_base_sample)
        };
        if track.len() > MAX_MEMORY_SAMPLES + 48_000 * 5 {
            let remove = track.len() - MAX_MEMORY_SAMPLES;
            track.drain(..remove);
            *base += remove;
        }
    }
}

fn transcode_raw_cancellable(input: &Path, output: &Path, format: ArchiveFormat, cancel: Option<&AtomicBool>) -> Result<()> {
    let partial = output.with_extension(format!("{}.partial", format.extension()));
    let result = (|| {
        match format {
        ArchiveFormat::Flac => transcode_raw_to_flac_cancellable(input, &partial, cancel),
        ArchiveFormat::Wav => transcode_raw_to_wav_cancellable(input, &partial, cancel),
        ArchiveFormat::Mp3 => transcode_raw_to_mp3_cancellable(input, &partial, cancel),
        }?;
        check_archive_cancel(cancel)?;
        fs::OpenOptions::new().write(true).open(&partial)?.sync_all()?;
        fs::rename(&partial, output)?;
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(&partial); }
    result
}

fn check_archive_cancel(cancel: Option<&AtomicBool>) -> Result<()> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        anyhow::bail!("Live finalization cancelled; raw audio retained");
    }
    Ok(())
}

fn transcode_raw_to_flac(input: &Path, output: &Path) -> Result<()> {
    transcode_raw_to_flac_cancellable(input, output, None)
}
fn transcode_raw_to_flac_cancellable(input: &Path, output: &Path, cancel: Option<&AtomicBool>) -> Result<()> {
    use std::io::{Read, Write};
    let output_file = fs::File::create(output)
        .with_context(|| format!("transcode: cannot create {}", output.display()))?;
    let mut writer = FlacByteWriter::<_, LittleEndian>::new(
        output_file,
        Options::default(),
        48_000,
        16,
        1,
        None,
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let mut input = std::io::BufReader::new(
        fs::File::open(input)
            .with_context(|| format!("transcode: cannot open {}", input.display()))?,
    );
    let mut floats = [0_u8; 16 * 1024];
    loop {
        check_archive_cancel(cancel)?;
        let read = input.read(&mut floats)?;
        if read == 0 {
            break;
        }
        let mut pcm = Vec::with_capacity(read / 2);
        for chunk in floats[..read].chunks_exact(4) {
            let sample = f32::from_le_bytes(chunk.try_into().unwrap());
            pcm.extend_from_slice(
                &((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes(),
            );
        }
        writer.write_all(&pcm)?;
    }
    writer
        .finalize()
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

fn transcode_raw_to_wav_cancellable(input: &Path, output: &Path, cancel: Option<&AtomicBool>) -> Result<()> {
    let mut input = std::io::BufReader::new(fs::File::open(input)?);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(output, spec)?;
    let mut floats = [0_u8; 16 * 1024];
    loop {
        check_archive_cancel(cancel)?;
        let read = input.read(&mut floats)?;
        if read == 0 {
            break;
        }
        for chunk in floats[..read].chunks_exact(4) {
            let sample = f32::from_le_bytes(chunk.try_into().unwrap());
            writer.write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
        }
    }
    writer.finalize()?;
    Ok(())
}

fn transcode_raw_to_mp3(input: &Path, output: &Path) -> Result<()> {
    transcode_raw_to_mp3_cancellable(input, output, None)
}
fn transcode_raw_to_mp3_cancellable(input: &Path, output: &Path, cancel: Option<&AtomicBool>) -> Result<()> {
    use shine_rs::{Mp3Encoder, Mp3EncoderConfig, StereoMode};

    let mut input = std::io::BufReader::new(fs::File::open(input)?);
    let mut output = BufWriter::new(fs::File::create(output)?);
    let config = Mp3EncoderConfig::new()
        .sample_rate(48_000)
        .bitrate(128)
        .channels(1)
        .stereo_mode(StereoMode::Mono);
    let mut encoder =
        Mp3Encoder::new(config).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let mut floats = [0_u8; 16 * 1024];
    loop {
        check_archive_cancel(cancel)?;
        let read = input.read(&mut floats)?;
        if read == 0 {
            break;
        }
        let pcm: Vec<i16> = floats[..read]
            .chunks_exact(4)
            .map(|chunk| {
                let sample = f32::from_le_bytes(chunk.try_into().unwrap());
                (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
            })
            .collect();
        for frame in encoder
            .encode_interleaved(&pcm)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?
        {
            output.write_all(&frame)?;
        }
    }
    output.write_all(
        &encoder
            .finish()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?,
    )?;
    output.flush()?;
    Ok(())
}

pub(crate) fn encode_mono_48k(samples: &[f32], output: &Path, format: ArchiveFormat) -> Result<()> {
    match format {
        ArchiveFormat::Wav => crate::pipeline::write_mono_48k_wav(samples, output),
        ArchiveFormat::Flac => {
            let raw = output.with_extension("mix.raw");
            write_raw_samples(&raw, samples)?;
            let result = transcode_raw_to_flac(&raw, output);
            let _ = fs::remove_file(raw);
            result
        }
        ArchiveFormat::Mp3 => {
            let raw = output.with_extension("mix.raw");
            write_raw_samples(&raw, samples)?;
            let result = transcode_raw_to_mp3(&raw, output);
            let _ = fs::remove_file(raw);
            result
        }
    }
}

fn write_raw_samples(path: &Path, samples: &[f32]) -> Result<()> {
    let mut writer = BufWriter::new(fs::File::create(path)?);
    for sample in samples {
        writer.write_all(&sample.to_le_bytes())?;
    }
    writer.flush()?;
    Ok(())
}

/// Device bytes (little-endian f32, s16 or s24, interleaved) into mono f32.
/// Only the layouts real devices offer are supported; anything else is
/// rejected loudly instead of being misparsed into "gurgling".
#[cfg(target_os = "windows")]
fn bytes_to_mono_f32(raw: &[u8], channels: usize, sample_bytes: usize, float32: bool) -> Vec<f32> {
    let channels = channels.max(1);
    let frame_bytes = sample_bytes * channels;
    if frame_bytes == 0 {
        return Vec::new();
    }
    let mut mono = Vec::with_capacity(raw.len() / frame_bytes);
    for frame in raw.chunks_exact(frame_bytes) {
        let mut acc = 0.0;
        for c in 0..channels {
            let o = c * sample_bytes;
            acc += sample_to_f32(&frame[o..o + sample_bytes], float32);
        }
        mono.push(acc / channels as f32);
    }
    mono
}

#[cfg(target_os = "windows")]
fn sample_to_f32(bytes: &[u8], float32: bool) -> f32 {
    if float32 {
        f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    } else if bytes.len() == 2 {
        i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32768.0
    } else {
        let sign = if bytes[2] & 0x80 != 0 { 0xFF } else { 0x00 };
        i32::from_le_bytes([bytes[0], bytes[1], bytes[2], sign]) as f32 / 8_388_608.0
    }
}

fn rms_tail(samples: &[f32]) -> f32 {
    let tail = &samples[samples.len().saturating_sub(4_800)..];
    if tail.is_empty() {
        return 0.0;
    }
    (tail.iter().map(|sample| sample * sample).sum::<f32>() / tail.len() as f32).sqrt()
}

#[cfg(target_os = "windows")]
pub mod platform {
    use super::*;
    use crate::capture_recovery::{CaptureAttempt, SupervisorEvent};
    use rubato::{FftFixedIn, Resampler};
    use std::{
        collections::VecDeque,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };
    use wasapi::{
        AudioClientProperties, DeviceEnumerator, Direction, SampleType, StreamCategory, StreamMode,
        WaveFormat, initialize_mta,
    };

    pub fn microphones() -> Result<Vec<String>> {
        let devices = DeviceEnumerator::new()?.get_device_collection(&Direction::Capture)?;
        (&devices)
            .into_iter()
            .map(|device| Ok(device?.get_friendlyname()?))
            .collect()
    }

    pub struct CaptureHandle {
        stop: Arc<AtomicBool>,
        threads: Mutex<Vec<thread::JoinHandle<()>>>,
    }

    impl CaptureHandle {
        pub fn request_stop(&self) {
            self.stop.store(true, Ordering::Relaxed);
        }
        pub fn stop(&self) {
            self.request_stop();
            for thread in self.threads.lock().unwrap().drain(..) {
                let _ = thread.join();
            }
        }
    }

    pub fn start(capture: Arc<Mutex<LiveCapture>>) -> Result<CaptureHandle> {
        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        for (direction, mic) in [(Direction::Capture, true), (Direction::Render, false)] {
            if (mic
                && !matches!(
                    capture.lock().unwrap().source,
                    LiveSource::Microphone | LiveSource::SystemAndMicrophone
                ))
                || (!mic
                    && !matches!(
                        capture.lock().unwrap().source,
                        LiveSource::System | LiveSource::SystemAndMicrophone
                    ))
            {
                continue;
            }
            let stop_flag = stop.clone();
            let capture = capture.clone();
            threads.push(thread::spawn(move || {
                let _ = initialize_mta();
                struct Attempt<'a> {
                    capture: &'a Arc<Mutex<LiveCapture>>,
                    mic: bool,
                    direction: Direction,
                    stop_flag: &'a AtomicBool,
                }
                impl CaptureAttempt for Attempt<'_> {
                    fn run(&mut self, reconnect_attempts: &mut usize) -> Result<(), String> {
                        let capture = self.capture;
                        let mic = self.mic;
                        let direction = &self.direction;
                        let stop_flag = self.stop_flag;
                        (|| -> Result<()> {
                    let enumerator = DeviceEnumerator::new()?;
                    let dual_source = matches!(
                        capture.lock().unwrap().source,
                        LiveSource::SystemAndMicrophone
                    );
                    let device = if mic {
                        capture
                            .lock()
                            .unwrap()
                            .microphone_device
                            .as_deref()
                            .map(|name| {
                                enumerator
                                    .get_device_collection(&Direction::Capture)?
                                    .get_device_with_name(name)
                            })
                            .transpose()?
                            .unwrap_or(enumerator.get_default_device(direction)?)
                    } else {
                        enumerator.get_default_device(direction)?
                    };
                    let endpoint_id = device.get_id()?;
                    let mut endpoint_checked = std::time::Instant::now();
                    // Open the device in its native mix format and convert
                    // here (downmix + resample to 48 kHz mono). Engine
                    // autoconvert stays ON: this engine rejects some
                    // hand-built formats outright (AUDCLNT_E_UNSUPPORTED_FORMAT),
                    // and the vendor itself documents channel-mask quirks,
                    // so the mask is retried with a fresh client.
                    let mix = device.get_device_format().ok();
                    let dev_rate = mix
                        .as_ref()
                        .map(|format| format.get_samplespersec() as usize)
                        .unwrap_or(48_000);
                    let dev_rate = if dev_rate == 0 { 48_000 } else { dev_rate };
                    let dev_ch = mix
                        .as_ref()
                        .map(|format| format.get_nchannels().max(1) as usize)
                        .unwrap_or(1);
                    let passthrough = mix.is_some();
                    let (float32, bits) = match mix.as_ref() {
                        Some(format) => {
                            let float32 =
                                matches!(format.get_subformat()?, SampleType::Float);
                            (float32, format.get_validbitspersample().max(8))
                        }
                        None => (true, 32),
                    };
                    let bytes_per_sample = if float32 {
                        if bits != 32 {
                            anyhow::bail!("unsupported {bits}-bit float device format");
                        }
                        4
                    } else if bits == 16 {
                        2
                    } else if bits == 24 {
                        3
                    } else {
                        anyhow::bail!("unsupported {bits}-bit PCM device format");
                    };
                    {
                        let mut state = capture.lock().unwrap();
                        let label = format!(
                            "{dev_rate}Hzx{dev_ch}ch/{}{}",
                            if float32 {
                                "f32"
                            } else if bits == 16 {
                                "s16"
                            } else {
                                "s24"
                            },
                            if passthrough { "" } else { "+auto" }
                        );
                        if mic {
                            state.mic_format = label;
                        } else {
                            state.sys_format = label;
                        }
                    }
                    let sample_type = if float32 {
                        SampleType::Float
                    } else {
                        SampleType::Int
                    };
                    let mut client_opt = None;
                    let mut initialized_mask: Option<u32> = None;
                    let mut init_error = None;
                    for mask in [None, Some(0)] {
                        let mut attempt = device.get_iaudioclient()?;
                        // The Communications category asks Windows/the audio
                        // driver to apply microphone effects such as AEC.
                        // Unsupported endpoints fall back to transcript-level
                        // de-duplication, so category failure is non-fatal.
                        if mic && dual_source {
                            let properties = AudioClientProperties::new()
                                .set_category(StreamCategory::Communications);
                            if let Err(error) = attempt.set_properties(properties) {
                                if let Some(path) = capture.lock().unwrap().log_path.clone() {
                                    debug_log(
                                        &path,
                                        &format!("mic AEC category unavailable: {error}"),
                                    );
                                }
                            }
                        }
                        let format = WaveFormat::new(
                            bytes_per_sample * 8,
                            bits as usize,
                            &sample_type,
                            dev_rate,
                            dev_ch,
                            mask,
                        );
                        let (_, minimum) = attempt.get_device_period()?;
                        match attempt.initialize_client(
                            &format,
                            &Direction::Capture,
                            &StreamMode::EventsShared {
                                autoconvert: true,
                                // A deep capture buffer (not the device minimum):
                                // under CPU load a tiny buffer overruns, the
                                // engine drops audio, and the recording stutters.
                                // Reads still drain everything available on every
                                // event, so this adds no latency.
                                buffer_duration_hns: minimum.max(2_500_000),
                            },
                        ) {
                            Ok(()) => {
                                client_opt = Some(attempt);
                                initialized_mask = mask;
                                break;
                            }
                            Err(error) => init_error = Some(error),
                        }
                    }
                    let client = client_opt.ok_or_else(|| {
                        init_error
                            .map(|error| anyhow::anyhow!(error.to_string()))
                            .unwrap_or_else(|| anyhow::anyhow!("audio client init failed"))
                    })?;
                    if mic && dual_source {
                        let aec_result: Result<bool> = (|| {
                            if !client.is_aec_supported()? {
                                return Ok(false);
                            }
                            let render = enumerator.get_default_device(&Direction::Render)?;
                            client
                                .get_aec_control()?
                                .set_echo_cancellation_render_endpoint(Some(render.get_id()?))?;
                            Ok(true)
                        })();
                        if let Some(path) = capture.lock().unwrap().log_path.clone() {
                            match aec_result {
                                Ok(true) => debug_log(
                                    &path,
                                    "mic acoustic echo cancellation enabled",
                                ),
                                Ok(false) => debug_log(
                                    &path,
                                    "mic acoustic echo cancellation unsupported; using transcript guard",
                                ),
                                Err(error) => debug_log(
                                    &path,
                                    &format!("mic acoustic echo cancellation unavailable: {error}"),
                                ),
                            }
                        }
                    }
                    if let Some(path) = capture
                        .lock()
                        .unwrap()
                        .log_path
                        .clone()
                    {
                        debug_log(
                            &path,
                            &format!(
                                "{} stream open (mask={})",
                                if mic { "mic" } else { "sys" },
                                initialized_mask.unwrap_or(u32::MAX)
                            ),
                        );
                    }
                    let event = client.set_get_eventhandle()?;
                    let reader = client.get_audiocaptureclient()?;
                    let mut bytes = VecDeque::new();
                    // Persistent streaming resampler device-rate -> 48 kHz.
                    let mut resampler: Option<FftFixedIn<f32>> =
                        if dev_rate != 48_000 {
                            Some(FftFixedIn::<f32>::new(dev_rate, 48_000, 1024, 2, 1)?)
                        } else {
                            None
                        };
                    let chunk_size = resampler
                        .as_ref()
                        .map(|resampler| resampler.input_frames_next())
                        .unwrap_or(0);
                    let mut skip = resampler
                        .as_ref()
                        .map(|resampler| resampler.output_delay())
                        .unwrap_or(0);
                    let mut pending: Vec<f32> = Vec::new();
                    // QPC timestamp of the last packet + converted samples
                    // emitted since it, for stamping the flush tail.
                    let mut last_ts = 0u64;
                    let mut since_ts = 0usize;
                    client.start_stream()?;
                    let mut healthy_audio_samples = 0usize;
                    {
                        let mut state = capture.lock().unwrap();
                        let value = if *reconnect_attempts > 0 { "resumed" } else { "capturing" };
                        if mic { state.microphone_capture_state = value.into(); }
                        else { state.system_capture_state = value.into(); }
                    }
                    let loop_result: Result<()> = (|| {
                    while !stop_flag.load(Ordering::Relaxed) {
                        if !mic && endpoint_checked.elapsed() >= std::time::Duration::from_secs(1) {
                            endpoint_checked = std::time::Instant::now();
                            let current_endpoint = enumerator.get_default_device(direction)
                                .and_then(|device| device.get_id())
                                .map_err(|error| anyhow::anyhow!("AUDCLNT_E_DEVICE_INVALIDATED: default render unavailable: {error}"))?;
                            if current_endpoint != endpoint_id {
                                anyhow::bail!("AUDCLNT_E_DEVICE_INVALIDATED: default render endpoint changed");
                            }
                        }
                        let t0 = std::time::Instant::now();
                        let info = reader.read_from_device_to_deque(&mut bytes)?;
                        let t1 = std::time::Instant::now();
                        // Bulk-convert: byte-wise popping costs per-packet CPU
                        // on the time-critical capture thread.
                        let raw: Vec<u8> = bytes.drain(..).collect();
                        let mono = bytes_to_mono_f32(&raw, dev_ch, bytes_per_sample, float32);
                        let raw_len = mono.len();
                        healthy_audio_samples = healthy_audio_samples.saturating_add(raw_len.saturating_mul(48_000) / dev_rate);
                        crate::capture_recovery::reset_after_healthy_capture(reconnect_attempts, healthy_audio_samples);
                        let samples_48k = if let Some(resampler) = resampler.as_mut() {
                            pending.extend_from_slice(&mono);
                            let mut out = Vec::new();
                            while pending.len() >= chunk_size {
                                out.extend(
                                    resampler
                                        .process(&[&pending[..chunk_size]], None)?
                                        .remove(0),
                                );
                                pending.drain(..chunk_size);
                            }
                            if skip > 0 {
                                let drop = skip.min(out.len());
                                out.drain(..drop);
                                skip -= drop;
                            }
                            out
                        } else {
                            mono
                        };
                        let t2 = std::time::Instant::now();
                        let mut state = capture.lock().unwrap();
                        let t3 = std::time::Instant::now();
                        if !info.flags.timestamp_error {
                            if info.timestamp != 0 {
                                last_ts = info.timestamp;
                                since_ts = 0;
                            }
                            since_ts += samples_48k.len();
                            if mic {
                                state.mic_raw += raw_len as u64;
                            } else {
                                state.sys_raw += raw_len as u64;
                            }
                            state.append_timed(mic, info.timestamp, samples_48k);
                        } else if mic {
                            state.mic_error_packets += 1;
                        } else {
                            state.sys_error_packets += 1;
                        }
                        // Phase accounting for loop diagnostics.
                        let read_us = t1.saturating_duration_since(t0).as_micros() as u64;
                        let work_us = t2.saturating_duration_since(t1).as_micros() as u64;
                        let lock_us = t3.saturating_duration_since(t2).as_micros() as u64;
                        if mic {
                            state.mic_iters += 1;
                            state.mic_read_us += read_us;
                            state.mic_work_us += work_us;
                            state.mic_lock_us += lock_us;
                        } else {
                            state.sys_iters += 1;
                            state.sys_read_us += read_us;
                            state.sys_work_us += work_us;
                            state.sys_lock_us += lock_us;
                        }
                        drop(state);
                        let waited = event.wait_for_event(100);
                        if waited.is_err() {
                            let mut state = capture.lock().unwrap();
                            if mic {
                                state.mic_wait_to += 1;
                            } else {
                                state.sys_wait_to += 1;
                            }
                        }
                    }
                    Ok(())
                    })();
                    // Flush resampler leftovers so the session tail is kept.
                    if let Some(resampler) = resampler.as_mut() {
                        if !pending.is_empty() {
                            let tail = std::mem::take(&mut pending);
                            let mut out = resampler
                                .process_partial(Some(&[tail.as_slice()]), None)?
                                .remove(0);
                            out.extend(
                                resampler
                                    .process_partial::<&[f32]>(None, None)?
                                    .remove(0),
                            );
                            if skip > 0 {
                                let drop = skip.min(out.len());
                                out.drain(..drop);
                            }
                            if !out.is_empty() && last_ts != 0 {
                                let tail_ts = last_ts
                                    + (since_ts as u64 * 10_000_000 / 48_000);
                                capture
                                    .lock()
                                    .unwrap()
                                    .append_timed(mic, tail_ts, out);
                            }
                        }
                    }
                    client.stop_stream()?;
                    loop_result?;
                    Ok(())
                        })()
                        .map_err(|error| error.to_string())
                    }
                }
                let explicitly_selected =
                    mic && capture.lock().unwrap().microphone_device.is_some();
                let mut attempt = Attempt {
                    capture: &capture,
                    mic,
                    direction,
                    stop_flag: &stop_flag,
                };
                crate::capture_recovery::supervise_capture(
                    &mut attempt,
                    &stop_flag,
                    explicitly_selected,
                    &mut |event| match event {
                        SupervisorEvent::AttemptStarted { .. } | SupervisorEvent::Stopped => {}
                        SupervisorEvent::Reconnecting { attempt, delay } => {
                            {
                                let mut state = capture.lock().unwrap();
                                if mic {
                                    state.microphone_capture_state = "reconnecting".into();
                                } else {
                                    state.system_capture_state = "reconnecting".into();
                                }
                            }
                            if let Some(path) = capture.lock().unwrap().log_path.clone() {
                                debug_log(
                                    &path,
                                    &format!(
                                        "capture reconnecting mic={mic} attempt={attempt} delay_ms={}",
                                        delay.as_millis()
                                    ),
                                );
                            }
                        }
                        SupervisorEvent::Failed { error } => {
                            let mut state = capture.lock().unwrap();
                            if let Some(path) = state.log_path.clone() {
                                debug_log(&path, &format!("capture thread error (mic={mic}): {error}"));
                            }
                            state.set_error(error);
                        }
                    },
                );
            }));
        }
        Ok(CaptureHandle {
            stop,
            threads: Mutex::new(threads),
        })
    }
}

#[cfg(target_os = "macos")]
pub mod platform {
    use super::*;
    use screencapturekit::prelude::*;
    use std::sync::{Arc, Mutex};
    use std::sync::atomic::{AtomicBool, Ordering};

    pub fn microphones() -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    struct Handler {
        capture: Arc<Mutex<LiveCapture>>,
        stopped: Arc<AtomicBool>,
    }
    impl SCStreamOutputTrait for Handler {
        fn did_output_sample_buffer(
            &self,
            sample: CMSampleBuffer,
            output_type: SCStreamOutputType,
        ) {
            if self.stopped.load(Ordering::Acquire) { return; }
            let time = sample.output_presentation_timestamp();
            let timestamp_100ns = if time.timescale > 0 {
                (time.value.max(0) as u128 * 10_000_000 / time.timescale as u128) as u64
            } else {
                0
            };
            if let Ok(list) = sample.audio_buffer_list() {
                let mut capture = self.capture.lock().unwrap();
                if self.stopped.load(Ordering::Acquire) { return; }
                for buffer in &list {
                    let samples = buffer
                        .data()
                        .chunks_exact(4)
                        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap_or([0; 4])));
                    match output_type {
                        SCStreamOutputType::Audio => {
                            let samples: Vec<_> = samples.collect();
                            capture.append_timed(false, timestamp_100ns, samples)
                        }
                        SCStreamOutputType::Microphone => {
                            let samples: Vec<_> = samples.collect();
                            capture.append_timed(true, timestamp_100ns, samples)
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    pub struct CaptureHandle {
        stream: Mutex<Option<SCStream>>,
        stopped: Arc<AtomicBool>,
    }
    impl CaptureHandle {
        pub fn request_stop(&self) { self.stopped.store(true, Ordering::Release); }
        pub fn stop(&self) {
            self.request_stop();
            if let Some(stream) = self.stream.lock().unwrap().take() {
                let _ = stream.stop_capture();
            }
        }
    }

    pub fn start(capture: Arc<Mutex<LiveCapture>>) -> Result<CaptureHandle> {
        let content = SCShareableContent::get()?;
        let display = content
            .displays()
            .into_iter()
            .next()
            .context("no display available")?;
        let filter = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build()?;
        let source = capture.lock().unwrap().source;
        let config = SCStreamConfiguration::new()
            .with_width(2)
            .with_height(2)
            .with_captures_audio(matches!(
                source,
                LiveSource::System | LiveSource::SystemAndMicrophone
            ))
            .with_sample_rate(48_000)
            .with_channel_count(1)
            .with_captures_microphone(matches!(
                source,
                LiveSource::Microphone | LiveSource::SystemAndMicrophone
            ))?;
        let mut stream = SCStream::new(&filter, &config)?;
        let stopped = Arc::new(AtomicBool::new(false));
        if matches!(source, LiveSource::System | LiveSource::SystemAndMicrophone) {
            stream.add_output_handler(
                Handler {
                    capture: capture.clone(),
                    stopped: stopped.clone(),
                },
                SCStreamOutputType::Audio,
            )?;
        }
        if matches!(
            source,
            LiveSource::Microphone | LiveSource::SystemAndMicrophone
        ) {
            stream.add_output_handler(Handler { capture, stopped: stopped.clone() }, SCStreamOutputType::Microphone)?;
        }
        stream.start_capture()?;
        Ok(CaptureHandle {
            stream: Mutex::new(Some(stream)),
            stopped,
        })
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub mod platform {
    use super::*;
    use std::sync::{Arc, Mutex};
    pub struct CaptureHandle;
    impl CaptureHandle {
        pub fn request_stop(&self) {}
        pub fn stop(&self) {}
    }
    pub fn start(_capture: Arc<Mutex<LiveCapture>>) -> Result<CaptureHandle> {
        anyhow::bail!("live capture is unsupported")
    }
    pub fn microphones() -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_track_failure_rolls_back_first_archive_and_keeps_raw() {
        let directory = std::env::temp_dir().join(format!("gigaam-track-failure-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&directory).unwrap();
        let mut capture = LiveCapture::new("dual-failure".into(), 0, LiveSource::SystemAndMicrophone,
            None, true, ArchiveFormat::Wav, 1.0, directory.clone(), None);
        capture.initialize_paths().unwrap();
        capture.append_timed(true, 10_000_000, vec![0.1; 4800]);
        drop(capture.system_file.take());
        if capture.system_path.exists() { fs::remove_file(&capture.system_path).unwrap(); }
        fs::create_dir(&capture.system_path).unwrap();
        assert!(capture.finish().is_err());
        assert!(capture.microphone_path.exists());
        assert!(!fs::read_dir(&directory).unwrap().flatten().any(|entry| entry.path().extension().is_some_and(|extension| extension == "wav")));
        drop(capture);
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { fs::remove_dir(path).unwrap(); } else { fs::remove_file(path).unwrap(); }
        }
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn cancelled_archive_keeps_raw_recovery_material() {
        let directory = std::env::temp_dir().join(format!("gigaam-cancel-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&directory).unwrap();
        let input = directory.join("audio.f32");
        fs::write(&input, vec![0u8; 16_384]).unwrap();
        let cancel = AtomicBool::new(true);
        for format in [ArchiveFormat::Wav, ArchiveFormat::Flac, ArchiveFormat::Mp3] {
            let output = directory.join(format!("audio.{}", format.extension()));
            assert!(transcode_raw_cancellable(&input, &output, format, Some(&cancel)).unwrap_err().to_string().contains("cancelled"));
            assert_eq!(fs::metadata(&input).unwrap().len(), 16_384);
            if output.exists() { fs::remove_file(output).unwrap(); }
        }
        fs::remove_file(input).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn compromised_archive_cannot_report_success_even_if_capture_error_clears() {
        let mut capture = LiveCapture::new("failed-archive".into(), 0, LiveSource::Microphone,
            None, true, ArchiveFormat::Wav, 1.0, PathBuf::new(), None);
        capture.archive_error = Some("injected partial write".into());
        capture.error = None;
        assert!(capture.finish().unwrap_err().to_string().contains("integrity compromised"));
    }

    #[test]
    fn dual_tracks_remain_separate() {
        let mut capture = LiveCapture::new(
            "live".into(),
            0,
            LiveSource::SystemAndMicrophone,
            None,
            false,
            ArchiveFormat::Flac,
            1.0,
            PathBuf::new(),
            None,
        );
        capture.microphone.extend([0.25, 0.5]);
        capture.system.extend([-0.25, -0.5]);
        assert_ne!(capture.microphone, capture.system);
        assert!(capture.finish().unwrap().is_empty());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn bytes_to_mono_downmixes_all_layouts() {
        // stereo s16: L=+0.5, R=-0.5 -> mono 0.0
        let mut raw = Vec::new();
        raw.extend_from_slice(&16384i16.to_le_bytes());
        raw.extend_from_slice(&(-16384i16).to_le_bytes());
        assert_eq!(bytes_to_mono_f32(&raw, 2, 2, false), vec![0.0]);
        // mono s16 full scale
        assert_eq!(
            bytes_to_mono_f32(&32767i16.to_le_bytes(), 1, 2, false)[0],
            32767.0 / 32768.0
        );
        // mono s24 extremes
        assert_eq!(bytes_to_mono_f32(&[0x00, 0x00, 0x80], 1, 3, false)[0], -1.0);
        assert_eq!(
            bytes_to_mono_f32(&[0xFF, 0xFF, 0x7F], 1, 3, false)[0],
            8_388_607.0 / 8_388_608.0
        );
        // stereo f32 passthrough average
        let mut raw = Vec::new();
        raw.extend_from_slice(&0.5f32.to_le_bytes());
        raw.extend_from_slice(&1.0f32.to_le_bytes());
        assert_eq!(bytes_to_mono_f32(&raw, 2, 4, true), vec![0.75]);
        assert!(bytes_to_mono_f32(&[], 2, 4, true).is_empty());
    }

    #[test]
    fn track_start_offsets_anchor_to_session_origin() {
        let mut capture = LiveCapture::new(
            "live".into(),
            0,
            LiveSource::SystemAndMicrophone,
            None,
            false,
            ArchiveFormat::Flac,
            1.0,
            PathBuf::new(),
            None,
        );
        capture.append_timed(true, 1_000_000, vec![0.1; 10]);
        capture.append_timed(false, 1_000_000 + 2 * 10_000_000, vec![0.2; 10]);
        capture.append_timed(true, 1_000_000 + 3 * 10_000_000, vec![0.1; 10]);
        assert_eq!(capture.mic_start_offset, Some(0.0));
        assert_eq!(capture.sys_start_offset, Some(2.0));
        assert_eq!(capture.microphone.len(), 20);
        assert_eq!(capture.system.len(), 10);
    }

    #[test]
    fn sequential_appends_concatenate_without_dedup() {
        let mut capture = LiveCapture::new(
            "live".into(),
            0,
            LiveSource::Microphone,
            None,
            false,
            ArchiveFormat::Flac,
            1.0,
            PathBuf::new(),
            None,
        );
        capture.append_timed(true, 1_000_000, vec![1.0; 10]);
        // overlapping timestamps do not drop audio anymore
        capture.append_timed(true, 1_001_875, vec![2.0; 10]);
        assert_eq!(capture.microphone.len(), 20);
        assert_eq!(&capture.microphone[..10], &[1.0; 10]);
        assert_eq!(&capture.microphone[10..], &[2.0; 10]);
    }

    #[test]
    fn microphone_gain_amplifies_and_clips() {
        let mut capture = LiveCapture::new(
            "live".into(),
            0,
            LiveSource::Microphone,
            None,
            false,
            ArchiveFormat::Flac,
            2.0,
            PathBuf::new(),
            None,
        );
        capture.append_timed(true, 1_000_000, vec![0.25; 10]);
        assert!(capture.microphone.iter().all(|sample| *sample == 0.5));
        let mut loud = LiveCapture::new(
            "live".into(),
            0,
            LiveSource::Microphone,
            None,
            false,
            ArchiveFormat::Flac,
            4.0,
            PathBuf::new(),
            None,
        );
        loud.append_timed(true, 1_000_000, vec![0.5; 10]);
        assert!(loud.microphone.iter().all(|sample| *sample == 1.0));
        // system track is never amplified
        loud.append_timed(false, 1_000_000, vec![0.5; 10]);
        assert!(loud.system.iter().all(|sample| *sample == 0.5));
        assert_eq!(loud.mic_packets, 1);
        assert_eq!(loud.sys_packets, 1);
    }

    #[test]
    fn archive_files_open_once_and_transcode() {
        let dir = std::env::temp_dir().join(format!("gigaam-live-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut capture = LiveCapture::new(
            "live".into(),
            0,
            LiveSource::Microphone,
            None,
            true,
            ArchiveFormat::Flac,
            1.0,
            dir.clone(),
            None,
        );
        capture.initialize_paths().unwrap();
        capture.append_timed(true, 1_000_000, vec![0.25; 48_000]);
        capture.append_timed(true, 1_010_000, vec![0.25; 48_000]);
        assert!(fs::metadata(&capture.microphone_path).unwrap().len() > 0);
        // the untouched track leaves no file behind
        assert!(!capture.system_path.exists());
        let files = capture.finish().unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn timestamp_jump_counts_gap_without_inserting_silence() {
        let mut capture = LiveCapture::new(
            "live".into(),
            0,
            LiveSource::Microphone,
            None,
            false,
            ArchiveFormat::Flac,
            1.0,
            PathBuf::new(),
            None,
        );
        capture.append_timed(true, 1_000_000, vec![0.5; 10]);
        // 700 s jump is only counted, never stuffed into the recording
        capture.append_timed(true, 1_000_000 + 700 * 10_000_000, vec![0.5; 10]);
        assert_eq!(capture.gap_resyncs, 1);
        assert_eq!(capture.mic_gap_ms, 699_999);
        assert_eq!(capture.microphone.len(), 20);
        assert!(capture.microphone.iter().all(|sample| *sample == 0.5));
        assert_eq!(capture.microphone_base_sample, 0);
    }

    #[test]
    fn timestamp_gap_counts_without_padding() {
        let mut capture = LiveCapture::new(
            "live".into(),
            0,
            LiveSource::Microphone,
            None,
            false,
            ArchiveFormat::Flac,
            1.0,
            PathBuf::new(),
            None,
        );
        capture.append_timed(true, 1_000_000, vec![1.0; 10]);
        // 0.25 s jump: counted for diagnostics, audio concatenated as-is
        capture.append_timed(true, 1_000_000 + 2_500_000, vec![2.0; 10]);
        assert_eq!(&capture.microphone[..10], &[1.0; 10]);
        assert_eq!(&capture.microphone[10..], &[2.0; 10]);
        assert_eq!(capture.mic_gap_ms, 249);
        assert_eq!(capture.gap_resyncs, 0);
    }
}
