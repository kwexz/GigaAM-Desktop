use std::{
    f32::consts::PI,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::{
    domain::{SegmentState, SourceTrack, TranscriptSegment, Transcription},
    queue::FileProcessor,
};
use anyhow::{Context, Result, bail};
use half::bf16;
use ndarray::{Array2, Array3, Axis, Ix2};
use ort::{session::Session, value::Tensor};
use rubato::{FftFixedIn, Resampler};
use rustfft::{FftPlanner, num_complex::Complex};
use sha2::{Digest, Sha256};
use symphonia::core::{
    audio::sample::Sample,
    codecs::audio::AudioDecoderOptions,
    errors::Error as SymphoniaError,
    formats::{FormatOptions, TrackType, probe::Hint},
    io::MediaSourceStream,
    meta::MetadataOptions,
};
use sysinfo::{ProcessesToUpdate, System, get_current_pid};

const SAMPLE_RATE: usize = 16_000;
const FFT_SIZE: usize = 320;
const HOP_LENGTH: usize = 160;
const MEL_BINS: usize = 64;
const VAD_FRAME: usize = 512;
const VAD_CONTEXT: usize = 64;
const VAD_THRESHOLD: f32 = 0.5;
const VAD_NEG_THRESHOLD: f32 = 0.35;
const MIN_SPEECH_SAMPLES: usize = SAMPLE_RATE / 5;
const MIN_SILENCE_SAMPLES: usize = SAMPLE_RATE / 10;
const SPEECH_PAD_SAMPLES: usize = SAMPLE_RATE * 120 / 1000;
const PREFERRED_CHUNK_SAMPLES: usize = SAMPLE_RATE * 22;
const PREFERRED_SPLIT_SAMPLES: usize = SAMPLE_RATE * 15;
const MAX_CHUNK_SAMPLES: usize = SAMPLE_RATE * 30;
const SILERO_SIZE: u64 = 2_327_524;
const SILERO_SHA256: &str = "1a153a22f4509e292a94e67d6f9b85e8deb25b4988682b7e174c65279d8788e3";

#[derive(Clone, Copy, Debug, PartialEq)]
struct SpeechRegion {
    start: usize,
    end: usize,
}

pub fn spike_main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let audio_path = PathBuf::from(
        args.next()
            .context("usage: inference_spike <wav> [model] [vocab] [vad-model]")?,
    );
    let model_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("models/v3_e2e_ctc.int8.onnx"));
    let vocab_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("models/v3_e2e_ctc_vocab.txt"));
    let vad_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("models/silero_vad.onnx"));

    let mut system = System::new();
    let baseline_rss = process_rss(&mut system)?;
    verify_silero(&vad_path)?;
    let (audio, source_duration) = load_wav(&audio_path)?;
    let audio_duration = Duration::from_secs_f64(audio.len() as f64 / SAMPLE_RATE as f64);

    let load_started = Instant::now();
    let mut asr = Session::builder()?.commit_from_file(&model_path)?;
    let mut vad = Session::builder()?.commit_from_file(&vad_path)?;
    let model_load_time = load_started.elapsed();
    let loaded_rss = process_rss(&mut system)?;

    let vad_started = Instant::now();
    let speech_regions = detect_speech(&audio, &mut vad)?;
    let chunks = chunk_speech(&speech_regions, audio.len());
    let vad_time = vad_started.elapsed();

    let vocab = load_vocab(&vocab_path)?;
    validate_vocab(&vocab)?;
    let transcription_id = audio_path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("transcription")
        .to_owned();
    let mut segments = Vec::with_capacity(chunks.len());
    let mut preprocess_time = Duration::ZERO;
    let mut inference_time = Duration::ZERO;
    let mut output_shape = Vec::new();
    let inference_started = Instant::now();
    for (index, chunk) in chunks.iter().enumerate() {
        let preprocess_started = Instant::now();
        let features = log_mel_features(&audio[chunk.start..chunk.end])?;
        preprocess_time += preprocess_started.elapsed();
        let feature_frames = features.len_of(Axis(2));
        let feature_length =
            Tensor::from_array(([1usize], vec![feature_frames as i64].into_boxed_slice()))?;
        let chunk_inference_started = Instant::now();
        let outputs = asr.run(ort::inputs![
            "features" => Tensor::from_array(features)?,
            "feature_lengths" => feature_length
        ])?;
        inference_time += chunk_inference_started.elapsed();
        let log_probs = outputs["log_probs"].try_extract_array::<f32>()?;
        if log_probs.shape()[2] != vocab.len() {
            bail!(
                "model output has {} classes but vocabulary has {} entries",
                log_probs.shape()[2],
                vocab.len()
            );
        }
        output_shape = log_probs.shape().to_vec();
        let text = decode_ctc(
            log_probs
                .index_axis(Axis(0), 0)
                .into_dimensionality::<Ix2>()?,
            &vocab,
        )?;
        segments.push(
            TranscriptSegment::new(
                &transcription_id,
                index,
                chunk.start as f64 / SAMPLE_RATE as f64,
                chunk.end as f64 / SAMPLE_RATE as f64,
                text,
                SourceTrack::File,
                SegmentState::Final,
            )
            .map_err(anyhow::Error::msg)?,
        );
    }
    let processing_time = inference_started.elapsed();
    let peak_observed_rss = process_rss(&mut system)?;
    let total_time = vad_time + processing_time;

    println!("audio={}", audio_path.display());
    println!("source_duration_s={:.3}", source_duration.as_secs_f64());
    println!("audio_duration_s={:.3}", audio_duration.as_secs_f64());
    println!("speech_regions={}", speech_regions.len());
    println!("asr_chunks={}", chunks.len());
    println!("last_output_shape={output_shape:?}");
    println!("model_load_ms={}", model_load_time.as_millis());
    println!("vad_ms={}", vad_time.as_millis());
    println!("preprocess_ms={}", preprocess_time.as_millis());
    println!("inference_ms={}", inference_time.as_millis());
    println!(
        "rtf={:.4}",
        total_time.as_secs_f64() / audio_duration.as_secs_f64()
    );
    println!("rss_baseline_mib={:.1}", mib(baseline_rss));
    println!("rss_model_loaded_mib={:.1}", mib(loaded_rss));
    println!("rss_after_inference_mib={:.1}", mib(peak_observed_rss));
    println!("segments={}", serde_json::to_string_pretty(&segments)?);
    Ok(())
}

fn verify_silero(path: &Path) -> Result<()> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    if bytes.len() as u64 != SILERO_SIZE || hash != SILERO_SHA256 {
        bail!("Silero VAD artifact does not match the pinned official model");
    }
    Ok(())
}

fn detect_speech(audio: &[f32], session: &mut Session) -> Result<Vec<SpeechRegion>> {
    let mut state = ndarray::Array3::<f32>::zeros((2, 1, 128));
    let mut context = [0.0_f32; VAD_CONTEXT];
    let mut probabilities = Vec::with_capacity(audio.len().div_ceil(VAD_FRAME));

    for chunk in audio.chunks(VAD_FRAME) {
        let mut input = vec![0.0; VAD_CONTEXT + VAD_FRAME];
        input[..VAD_CONTEXT].copy_from_slice(&context);
        input[VAD_CONTEXT..VAD_CONTEXT + chunk.len()].copy_from_slice(chunk);
        let outputs = session.run(ort::inputs![
            "input" => Tensor::from_array(([1usize, VAD_CONTEXT + VAD_FRAME], input.into_boxed_slice()))?,
            "state" => Tensor::from_array(state)?,
            "sr" => Tensor::from_array(ndarray::arr0(SAMPLE_RATE as i64))?
        ])?;
        probabilities.push(outputs["output"].try_extract_array::<f32>()?[[0, 0]]);
        state = outputs["stateN"]
            .try_extract_array::<f32>()?
            .to_owned()
            .into_dimensionality()?;
        context.fill(0.0);
        let context_source = if chunk.len() >= VAD_CONTEXT {
            &chunk[chunk.len() - VAD_CONTEXT..]
        } else {
            chunk
        };
        context[VAD_CONTEXT - context_source.len()..].copy_from_slice(context_source);
    }
    Ok(regions_from_probabilities(&probabilities, audio.len()))
}

fn regions_from_probabilities(probabilities: &[f32], audio_len: usize) -> Vec<SpeechRegion> {
    let mut regions = Vec::new();
    let mut speech_start = None;
    let mut silence_start = None;

    for (frame, probability) in probabilities.iter().copied().enumerate() {
        let sample = frame * VAD_FRAME;
        if probability >= VAD_THRESHOLD {
            speech_start.get_or_insert(sample);
            silence_start = None;
        } else if speech_start.is_some() && probability < VAD_NEG_THRESHOLD {
            let silence = *silence_start.get_or_insert(sample);
            if sample.saturating_sub(silence) >= MIN_SILENCE_SAMPLES {
                let start = speech_start.take().unwrap();
                if silence.saturating_sub(start) >= MIN_SPEECH_SAMPLES {
                    regions.push(SpeechRegion {
                        start,
                        end: silence,
                    });
                }
                silence_start = None;
            }
        }
    }
    if let Some(start) = speech_start {
        if audio_len.saturating_sub(start) >= MIN_SPEECH_SAMPLES {
            regions.push(SpeechRegion {
                start,
                end: audio_len,
            });
        }
    }

    let mut padded = Vec::<SpeechRegion>::with_capacity(regions.len());
    for region in regions {
        let region = SpeechRegion {
            start: region.start.saturating_sub(SPEECH_PAD_SAMPLES),
            end: (region.end + SPEECH_PAD_SAMPLES).min(audio_len),
        };
        if let Some(previous) = padded.last_mut()
            && region.start <= previous.end
        {
            previous.end = previous.end.max(region.end);
        } else {
            padded.push(region);
        }
    }
    padded
}

fn chunk_speech(regions: &[SpeechRegion], audio_len: usize) -> Vec<SpeechRegion> {
    let mut chunks = Vec::new();
    let mut current = None::<SpeechRegion>;

    for region in regions {
        if let Some(active) = current.as_mut() {
            if active.end.saturating_sub(active.start) < PREFERRED_SPLIT_SAMPLES
                && region.end.saturating_sub(active.start) <= PREFERRED_CHUNK_SAMPLES
            {
                active.end = region.end;
                continue;
            }
            chunks.push(*active);
            current = None;
        }

        let mut start = region.start;
        while region.end.saturating_sub(start) > PREFERRED_CHUNK_SAMPLES {
            let preferred_end = start + PREFERRED_CHUNK_SAMPLES;
            let hard_end = (start + MAX_CHUNK_SAMPLES).min(region.end);
            chunks.push(SpeechRegion {
                start,
                end: preferred_end.min(hard_end),
            });
            start = preferred_end.min(hard_end);
        }
        if start < region.end {
            current = Some(SpeechRegion {
                start,
                end: region.end,
            });
        }
    }
    if let Some(chunk) = current {
        chunks.push(chunk);
    }
    chunks.retain(|chunk| chunk.start < chunk.end && chunk.start < audio_len);
    for chunk in &mut chunks {
        chunk.end = chunk.end.min(audio_len);
    }
    let mut index = 0;
    while index < chunks.len() {
        if chunks[index].end - chunks[index].start < FFT_SIZE {
            if index > 0 && chunks[index].end - chunks[index - 1].start <= MAX_CHUNK_SAMPLES {
                chunks[index - 1].end = chunks[index].end;
                chunks.remove(index);
                continue;
            }
            if index + 1 < chunks.len()
                && chunks[index + 1].end - chunks[index].start <= MAX_CHUNK_SAMPLES
            {
                chunks[index + 1].start = chunks[index].start;
                chunks.remove(index);
                continue;
            }
            chunks.remove(index);
            continue;
        }
        index += 1;
    }
    chunks
}

fn validate_vocab(vocab: &[String]) -> Result<()> {
    if vocab.is_empty() || !vocab.iter().any(|token| token == "<blk>") {
        bail!("vocabulary must contain <blk>");
    }
    Ok(())
}

fn load_wav(path: &Path) -> Result<(Vec<f32>, Duration)> {
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("wav"))
    {
        return load_pcm_wav(path);
    }
    load_media(path)
}

fn load_pcm_wav(path: &Path) -> Result<(Vec<f32>, Duration)> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let spec = reader.spec();
    if spec.channels == 0 {
        bail!("WAV has no channels");
    }
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int if spec.bits_per_sample <= 16 => reader
            .samples::<i16>()
            .map(|sample| Ok(sample? as f32 / (1_i32 << (spec.bits_per_sample - 1)) as f32))
            .collect::<Result<_>>()?,
        hound::SampleFormat::Int if spec.bits_per_sample <= 32 => reader
            .samples::<i32>()
            .map(|sample| Ok(sample? as f32 / (1_i64 << (spec.bits_per_sample - 1)) as f32))
            .collect::<Result<_>>()?,
        hound::SampleFormat::Float if spec.bits_per_sample == 32 => reader
            .samples::<f32>()
            .map(|sample| {
                let sample = sample?;
                if !sample.is_finite() {
                    bail!("WAV contains a non-finite sample");
                }
                Ok(sample.clamp(-1.0, 1.0))
            })
            .collect::<Result<_>>()?,
        _ => bail!("expected integer PCM up to 32-bit or 32-bit float WAV"),
    };
    let channels = spec.channels as usize;
    let mono: Vec<f32> = samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();
    let duration = Duration::from_secs_f64(mono.len() as f64 / spec.sample_rate as f64);
    Ok((resample(mono, spec.sample_rate as usize)?, duration))
}

fn load_media(path: &Path) -> Result<(Vec<f32>, Duration)> {
    let file = Box::new(fs::File::open(path)?);
    let stream = MediaSourceStream::new(file, Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe().probe(
        &hint,
        stream,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;
    let track = format
        .default_track(TrackType::Audio)
        .context("media contains no audio track")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs().make_audio_decoder(
        track
            .codec_params
            .as_ref()
            .context("audio codec parameters are missing")?
            .audio()
            .context("audio codec parameters are invalid")?,
        &AudioDecoderOptions::default(),
    )?;
    let mut mono = Vec::new();
    let mut sample_rate = None;
    while let Some(packet) = format.next_packet()? {
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buffer) => {
                let spec = buffer.spec();
                sample_rate = Some(spec.rate());
                let channels = spec.channels().count();
                let mut interleaved = vec![f32::MID; buffer.samples_interleaved()];
                buffer.copy_to_slice_interleaved(&mut interleaved);
                mono.extend(
                    interleaved
                        .chunks_exact(channels)
                        .map(|frame| frame.iter().sum::<f32>() / channels as f32),
                );
            }
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let sample_rate = sample_rate.context("media contained no decodable audio")? as usize;
    let duration = Duration::from_secs_f64(mono.len() as f64 / sample_rate as f64);
    Ok((resample(mono, sample_rate)?, duration))
}

fn resample(audio: Vec<f32>, source_rate: usize) -> Result<Vec<f32>> {
    resample_to(audio, source_rate, SAMPLE_RATE)
}

pub(crate) fn resample_to(
    audio: Vec<f32>,
    source_rate: usize,
    target_rate: usize,
) -> Result<Vec<f32>> {
    if source_rate == target_rate {
        return Ok(audio);
    }
    let mut resampler = FftFixedIn::<f32>::new(source_rate, target_rate, 1024, 2, 1)?;
    let chunk_size = resampler.input_frames_next();
    let delay = resampler.output_delay();
    let target_len =
        (audio.len() as f64 * target_rate as f64 / source_rate as f64).round() as usize;
    let mut output = Vec::with_capacity(target_len);
    let mut chunks = audio.chunks_exact(chunk_size);
    for chunk in &mut chunks {
        output.extend(resampler.process(&[chunk], None)?.remove(0));
    }
    if !chunks.remainder().is_empty() {
        output.extend(
            resampler
                .process_partial(Some(&[chunks.remainder()]), None)?
                .remove(0),
        );
    }
    output.extend(resampler.process_partial::<&[f32]>(None, None)?.remove(0));
    Ok(output.into_iter().skip(delay).take(target_len).collect())
}

/// Decode any media file into mono 48 kHz f32, reusing the file-pipeline
/// decoding and resampling. Used for mixing archived live tracks.
pub(crate) fn decode_mono_48k(path: &Path) -> Result<Vec<f32>> {
    let file = Box::new(fs::File::open(path)?);
    let stream = MediaSourceStream::new(file, Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe().probe(
        &hint,
        stream,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;
    let track = format
        .default_track(TrackType::Audio)
        .context("media contains no audio track")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs().make_audio_decoder(
        track
            .codec_params
            .as_ref()
            .context("audio codec parameters are missing")?
            .audio()
            .context("audio codec parameters are invalid")?,
        &AudioDecoderOptions::default(),
    )?;
    let mut mono = Vec::new();
    let mut sample_rate = None;
    while let Some(packet) = format.next_packet()? {
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buffer) => {
                sample_rate = Some(buffer.spec().rate());
                let channels = buffer.spec().channels().count().max(1);
                let mut interleaved = vec![f32::MID; buffer.samples_interleaved()];
                buffer.copy_to_slice_interleaved(&mut interleaved);
                mono.extend(
                    interleaved
                        .chunks_exact(channels)
                        .map(|frame| frame.iter().sum::<f32>() / channels as f32),
                );
            }
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let sample_rate = sample_rate.context("media contained no decodable audio")? as usize;
    if sample_rate == 48_000 {
        return Ok(mono);
    }
    resample_to(mono, sample_rate, 48_000)
}

/// Mix mono 48 kHz tracks sample-by-sample (summed, clamped) into one WAV.
/// Each track starts at its session offset; shorter tracks and leading gaps
/// are zero-padded, so the mix preserves the recording chronology.
pub(crate) fn mix_mono_48k_to_wav(inputs: &[(PathBuf, f64)], output: &Path) -> Result<()> {
    let mut mixed: Vec<f32> = Vec::new();
    for (input, offset_seconds) in inputs {
        let track = decode_mono_48k(input)?;
        let at = (offset_seconds.max(0.0) * 48_000.0) as usize;
        if at + track.len() > mixed.len() {
            mixed.resize(at + track.len(), 0.0);
        }
        for (i, sample) in track.iter().enumerate() {
            mixed[at + i] = (mixed[at + i] + sample).clamp(-1.0, 1.0);
        }
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(output, spec)?;
    for sample in mixed {
        writer.write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

fn log_mel_features(audio: &[f32]) -> Result<Array3<f32>> {
    if audio.len() < FFT_SIZE {
        bail!("audio must contain at least {FFT_SIZE} samples");
    }
    let frames = (audio.len() - FFT_SIZE) / HOP_LENGTH + 1;
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| quantize_bf16(0.5 - 0.5 * (2.0 * PI * i as f32 / FFT_SIZE as f32).cos()))
        .collect();
    let filterbank = mel_filterbank();
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let mut fft_buffer = vec![Complex::default(); FFT_SIZE];
    let mut features = Array3::zeros((1, MEL_BINS, frames));

    for frame in 0..frames {
        let start = frame * HOP_LENGTH;
        for i in 0..FFT_SIZE {
            fft_buffer[i] = Complex::new(audio[start + i] * window[i], 0.0);
        }
        fft.process(&mut fft_buffer);
        for mel in 0..MEL_BINS {
            let mut energy = 0.0;
            for frequency in 0..=FFT_SIZE / 2 {
                energy += fft_buffer[frequency].norm_sqr() * filterbank[(frequency, mel)];
            }
            features[(0, mel, frame)] = energy.clamp(1e-9, 1e9).ln();
        }
    }
    Ok(features)
}

fn mel_filterbank() -> Array2<f32> {
    let frequencies: Vec<f64> = (0..=FFT_SIZE / 2)
        .map(|i| i as f64 * (SAMPLE_RATE / 2) as f64 / (FFT_SIZE / 2) as f64)
        .collect();
    let min_mel = hz_to_mel(0.0);
    let max_mel = hz_to_mel((SAMPLE_RATE / 2) as f64);
    let mel_points: Vec<f64> = (0..MEL_BINS + 2)
        .map(|i| min_mel + (max_mel - min_mel) * i as f64 / (MEL_BINS + 1) as f64)
        .map(mel_to_hz)
        .collect();
    Array2::from_shape_fn((FFT_SIZE / 2 + 1, MEL_BINS), |(frequency, mel)| {
        let rising =
            (frequencies[frequency] - mel_points[mel]) / (mel_points[mel + 1] - mel_points[mel]);
        let falling = (mel_points[mel + 2] - frequencies[frequency])
            / (mel_points[mel + 2] - mel_points[mel + 1]);
        quantize_bf16(rising.min(falling).max(0.0) as f32)
    })
}

fn hz_to_mel(hz: f64) -> f64 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

fn mel_to_hz(mel: f64) -> f64 {
    700.0 * (10_f64.powf(mel / 2595.0) - 1.0)
}

fn quantize_bf16(value: f32) -> f32 {
    bf16::from_f32(value).to_f32()
}

fn load_vocab(path: &Path) -> Result<Vec<String>> {
    let mut vocab = Vec::new();
    for line in fs::read_to_string(path)?.lines() {
        let (token, id) = line.rsplit_once(' ').context("invalid vocabulary line")?;
        let id: usize = id.parse()?;
        if vocab.len() <= id {
            vocab.resize(id + 1, String::new());
        }
        vocab[id] = token.replace('▁', " ");
    }
    Ok(vocab)
}

fn decode_ctc(log_probs: ndarray::ArrayView2<'_, f32>, vocab: &[String]) -> Result<String> {
    let blank_id = vocab
        .iter()
        .position(|token| token == "<blk>")
        .context("vocabulary has no CTC blank token")?;
    let mut previous = blank_id;
    let mut text = String::new();
    for frame in log_probs.axis_iter(Axis(0)) {
        let token = frame
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| left.total_cmp(right))
            .map(|(index, _)| index)
            .unwrap_or(blank_id);
        if token != blank_id && token != previous {
            text.push_str(
                vocab
                    .get(token)
                    .context("model output token exceeds vocabulary")?,
            );
        }
        previous = token;
    }
    Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

const LIVE_SCAN_INPUT_RATE: usize = 48_000;
const LIVE_SCAN_MAX_16K_SAMPLES: usize = SAMPLE_RATE * 40;
const LIVE_SCAN_MIN_FRESH_48K: usize = 480;

/// Incremental 48 kHz -> 16 kHz buffer for one live track.
///
/// The capture thread appends 48 kHz samples (see `LiveCapture`); the scanner
/// resamples only the *new* span on every `push` with a persistent streaming
/// resampler, so a tick costs O(new audio) instead of O(whole tail).
/// Absolute time of `samples_16k[i]` is
/// `(abs0_48k + i * LIVE_SCAN_INPUT_RATE / SAMPLE_RATE) / LIVE_SCAN_INPUT_RATE`
/// seconds since the capture origin.
pub struct LiveScanner {
    resampler: FftFixedIn<f32>,
    pending_48k: Vec<f32>,
    skip_delay: usize,
    samples_16k: Vec<f32>,
    abs0_48k: usize,
    fed_48k: usize,
    last_base_48k: usize,
    // Streaming Silero VAD state: probabilities are computed only for new
    // frames, carrying LSTM state and context across ticks. `vad_probs[i]`
    // covers `samples_16k` indices
    // `[vad_abs0_16k + i * VAD_FRAME, vad_abs0_16k + (i + 1) * VAD_FRAME)`.
    vad_state: Array3<f32>,
    vad_context: [f32; VAD_CONTEXT],
    vad_probs: Vec<f32>,
    vad_abs0_16k: usize,
}

impl LiveScanner {
    pub fn new() -> Result<Self> {
        let resampler = FftFixedIn::<f32>::new(LIVE_SCAN_INPUT_RATE, SAMPLE_RATE, 1024, 2, 1)?;
        let skip_delay = resampler.output_delay();
        Ok(Self {
            resampler,
            pending_48k: Vec::new(),
            skip_delay,
            samples_16k: Vec::new(),
            abs0_48k: 0,
            fed_48k: 0,
            last_base_48k: 0,
            vad_state: Array3::zeros((2, 1, 128)),
            vad_context: [0.0; VAD_CONTEXT],
            vad_probs: Vec::new(),
            vad_abs0_16k: 0,
        })
    }

    /// Total 48 kHz input samples consumed so far (lets callers skip idle ticks).
    pub fn fed_48k(&self) -> usize {
        self.fed_48k
    }

    /// Absolute 48 kHz index of `samples_16k()[0]`.
    pub fn abs0_48k(&self) -> usize {
        self.abs0_48k
    }

    pub fn samples_16k(&self) -> &[f32] {
        &self.samples_16k
    }

    /// Sync with the current capture buffer (`base_48k` is its absolute base).
    pub fn push(&mut self, samples_48k: &[f32], base_48k: usize) -> Result<()> {
        if base_48k < self.last_base_48k {
            *self = Self::new()?;
        }
        if base_48k > self.abs0_48k {
            let drop_16k = (base_48k - self.abs0_48k)
                .saturating_mul(SAMPLE_RATE)
                / LIVE_SCAN_INPUT_RATE;
            self.drop_front_16k(drop_16k);
            self.fed_48k = self.fed_48k.max(base_48k);
        }
        self.last_base_48k = base_48k;
        let fresh_offset = self.fed_48k.saturating_sub(base_48k);
        if fresh_offset < samples_48k.len() {
            let fresh = &samples_48k[fresh_offset..];
            if fresh.len() >= LIVE_SCAN_MIN_FRESH_48K {
                self.feed(fresh)?;
                self.fed_48k = base_48k + samples_48k.len();
            }
        }
        if self.samples_16k.len() > LIVE_SCAN_MAX_16K_SAMPLES {
            let excess = self.samples_16k.len() - LIVE_SCAN_MAX_16K_SAMPLES;
            self.drop_front_16k(excess);
        }
        Ok(())
    }

    /// Flush resampler leftovers at the end of a session (recovers the last ~ms).
    pub fn finish(&mut self) -> Result<()> {
        if !self.pending_48k.is_empty() {
            let tail = std::mem::take(&mut self.pending_48k);
            let output = self
                .resampler
                .process_partial(Some(&[tail.as_slice()]), None)?
                .remove(0);
            self.append_output(output);
        }
        let output = self
            .resampler
            .process_partial::<&[f32]>(None, None)?
            .remove(0);
        self.append_output(output);
        Ok(())
    }

    /// Run Silero VAD over 16 kHz frames that have no probability yet.
    /// One tick costs O(new audio) instead of O(whole tail).
    pub fn run_vad(&mut self, session: &mut Session) -> Result<()> {
        let mut start = self.vad_abs0_16k + self.vad_probs.len() * VAD_FRAME;
        while start + VAD_FRAME <= self.samples_16k.len() {
            let mut input = vec![0.0; VAD_CONTEXT + VAD_FRAME];
            input[..VAD_CONTEXT].copy_from_slice(&self.vad_context);
            input[VAD_CONTEXT..].copy_from_slice(&self.samples_16k[start..start + VAD_FRAME]);
            let outputs = session.run(ort::inputs![
                "input" => Tensor::from_array(([1usize, VAD_CONTEXT + VAD_FRAME], input.into_boxed_slice()))?,
                "state" => Tensor::from_array(self.vad_state.clone())?,
                "sr" => Tensor::from_array(ndarray::arr0(SAMPLE_RATE as i64))?
            ])?;
            self.vad_probs
                .push(outputs["output"].try_extract_array::<f32>()?[[0, 0]]);
            self.vad_state = outputs["stateN"]
                .try_extract_array::<f32>()?
                .to_owned()
                .into_dimensionality()?;
            self.vad_context
                .copy_from_slice(&self.samples_16k[start + VAD_FRAME - VAD_CONTEXT..start + VAD_FRAME]);
            start += VAD_FRAME;
        }
        Ok(())
    }

    /// Speech regions for the 16 kHz span `[scan_idx16, scan_idx16 + len)`
    /// (indices relative to `abs0_48k()` mapping). The span start is quantized
    /// down to the VAD frame grid; the returned regions are relative to the
    /// quantized start.
    fn vad_regions(&self, scan_idx16: usize, len: usize) -> (Vec<SpeechRegion>, usize) {
        let quantized = scan_idx16 / VAD_FRAME * VAD_FRAME;
        let quantized = quantized.max(self.vad_abs0_16k);
        let frame_offset = (quantized - self.vad_abs0_16k) / VAD_FRAME;
        let probs = self
            .vad_probs
            .get(frame_offset.min(self.vad_probs.len())..)
            .unwrap_or(&[]);
        let audio_len = len.saturating_sub(quantized.saturating_sub(scan_idx16));
        (
            regions_from_probabilities(probs, audio_len),
            quantized,
        )
    }

    /// Drop a 16 kHz prefix from all buffers, keeping sample / probability /
    /// absolute-time alignment exact. The drop is quantized down to whole
    /// VAD frames.
    fn drop_front_16k(&mut self, drop16: usize) {
        let frames = (drop16 / VAD_FRAME)
            .min(self.vad_probs.len())
            .min(self.samples_16k.len() / VAD_FRAME);
        let drop = frames * VAD_FRAME;
        if drop == 0 {
            return;
        }
        self.samples_16k.drain(..drop);
        self.vad_probs.drain(..frames);
        self.vad_abs0_16k += drop;
        self.abs0_48k += drop * LIVE_SCAN_INPUT_RATE / SAMPLE_RATE;
    }

    /// Drop settled audio (already emitted or VAD-confirmed silence) from the
    /// front of the buffers, keeping LSTM/probability alignment exact.
    pub fn discard_before(&mut self, seconds: f64) {
        let abs0_seconds = self.abs0_48k as f64 / LIVE_SCAN_INPUT_RATE as f64;
        if seconds <= abs0_seconds {
            return;
        }
        let idx16 = ((seconds - abs0_seconds) * SAMPLE_RATE as f64) as usize;
        self.drop_front_16k(idx16);
    }

    #[cfg(test)]
    fn set_test_probs(&mut self, frames: usize) {
        // Pretend streaming VAD has covered the buffer start: used by tests
        // that cannot run an ONNX session.
        self.vad_abs0_16k = 0;
        self.vad_probs = vec![0.0; frames.min(self.samples_16k.len() / VAD_FRAME)];
    }

    fn feed(&mut self, fresh: &[f32]) -> Result<()> {
        self.pending_48k.extend_from_slice(fresh);
        let chunk = self.resampler.input_frames_next();
        while self.pending_48k.len() >= chunk {
            let output = self
                .resampler
                .process(&[&self.pending_48k[..chunk]], None)?
                .remove(0);
            self.pending_48k.drain(..chunk);
            self.append_output(output);
        }
        Ok(())
    }

    fn append_output(&mut self, mut output: Vec<f32>) {
        if self.skip_delay > 0 {
            let skip = self.skip_delay.min(output.len());
            output.drain(..skip);
            self.skip_delay -= skip;
            if output.is_empty() {
                return;
            }
        }
        self.samples_16k.extend(output);
    }
}

pub struct NativeFileProcessor {
    asr: Session,
    vad: Session,
    vocab: Vec<String>,
}

impl NativeFileProcessor {
    pub fn load(model_dir: &Path) -> Result<Self> {
        Self::load_with_threads(model_dir, None)
    }

    /// Loader for live recording: caps ORT thread pools so that the
    /// inference loop cannot saturate every core and freeze the UI/mouse.
    /// File transcription keeps using `load()` with full parallelism.
    pub fn load_live(model_dir: &Path) -> Result<Self> {
        Self::load_with_threads(model_dir, Some(2))
    }

    fn load_with_threads(model_dir: &Path, intra_threads: Option<usize>) -> Result<Self> {
        if !crate::model_manager::generation_valid(model_dir) {
            anyhow::bail!("model generation is not verified");
        }
        let vad_path = model_dir.join("silero_vad.onnx");
        verify_silero(&vad_path)?;
        let vocab = load_vocab(&model_dir.join("v3_e2e_ctc_vocab.txt"))?;
        validate_vocab(&vocab)?;
        let mut asr_builder = Session::builder()?;
        let mut vad_builder = Session::builder()?;
        if let Some(threads) = intra_threads {
            asr_builder = asr_builder
                .with_intra_threads(threads)
                .map_err(|error| anyhow::anyhow!("failed to set session threads: {error:?}"))?;
            vad_builder = vad_builder
                .with_intra_threads(threads)
                .map_err(|error| anyhow::anyhow!("failed to set session threads: {error:?}"))?;
            asr_builder = asr_builder
                .with_inter_threads(1)
                .map_err(|error| anyhow::anyhow!("failed to set session threads: {error:?}"))?;
            vad_builder = vad_builder
                .with_inter_threads(1)
                .map_err(|error| anyhow::anyhow!("failed to set session threads: {error:?}"))?;
        }
        Ok(Self {
            asr: asr_builder.commit_from_file(model_dir.join("v3_e2e_ctc.int8.onnx"))?,
            vad: vad_builder.commit_from_file(vad_path)?,
            vocab,
        })
    }

    fn transcribe_chunk(&mut self, audio: &[f32]) -> Result<String> {
        let features = log_mel_features(audio)?;
        let feature_frames = features.len_of(Axis(2));
        let feature_length =
            Tensor::from_array(([1usize], vec![feature_frames as i64].into_boxed_slice()))?;
        let outputs = self.asr.run(ort::inputs![
            "features" => Tensor::from_array(features)?,
            "feature_lengths" => feature_length
        ])?;
        let log_probs = outputs
            .get("log_probs")
            .context("ASR model has no log_probs output")?
            .try_extract_array::<f32>()?;
        if log_probs.ndim() != 3 || log_probs.shape()[2] != self.vocab.len() {
            anyhow::bail!("ASR model output does not match the vocabulary");
        }
        decode_ctc(
            log_probs
                .index_axis(Axis(0), 0)
                .into_dimensionality::<Ix2>()?,
            &self.vocab,
        )
    }

    pub fn transcribe_live_ready(
        &mut self,
        scanner: &mut LiveScanner,
        transcription_id: &str,
        source_track: SourceTrack,
        next_index: usize,
        emitted_seconds: f64,
        quiet_seconds: f64,
        flush: bool,
    ) -> Result<(Vec<TranscriptSegment>, f64, f64)> {
        scanner.run_vad(&mut self.vad)?;
        let abs0_seconds = scanner.abs0_48k() as f64 / LIVE_SCAN_INPUT_RATE as f64;
        // Scan only the unsettled tail: everything before `scan_from` is
        // either already emitted or VAD-confirmed silence, so re-scanning it
        // every tick would be pure waste that grows with session length.
        let scan_from = emitted_seconds.max(quiet_seconds).max(abs0_seconds);
        let scanned = scanner.samples_16k();
        let scan_idx = ((scan_from - abs0_seconds) * SAMPLE_RATE as f64) as usize;
        let scan_idx = scan_idx.min(scanned.len());
        let audio = &scanned[scan_idx..];
        if audio.len() < 960 {
            return Ok((Vec::new(), emitted_seconds, scan_from));
        }
        let (regions, quantized) = scanner.vad_regions(scan_idx, audio.len());
        let audio = &scanned[quantized.min(scanned.len())..];
        let scan_base = abs0_seconds + quantized.min(scanned.len()) as f64 / SAMPLE_RATE as f64;
        // Audio before the first region is confirmed silence and never needs
        // re-scanning; the emission cursor separately guards re-emission.
        let first_start = regions.first().map(|region| region.start).unwrap_or(audio.len());
        let mut quiet = scan_base + first_start as f64 / SAMPLE_RATE as f64;
        let mut segments = Vec::new();
        let mut emitted = emitted_seconds;
        for chunk in chunk_speech(&regions, audio.len()) {
            let absolute_start = scan_base + chunk.start as f64 / SAMPLE_RATE as f64;
            let absolute_end = scan_base + chunk.end as f64 / SAMPLE_RATE as f64;
            let natural_end = chunk.end + SAMPLE_RATE / 5 < audio.len();
            let forced_end = chunk.end - chunk.start >= PREFERRED_CHUNK_SAMPLES;
            if absolute_end <= emitted_seconds || (!flush && !natural_end && !forced_end) {
                continue;
            }
            let text = self.transcribe_chunk(&audio[chunk.start..chunk.end])?;
            segments.push(
                TranscriptSegment::new(
                    transcription_id,
                    next_index + segments.len(),
                    absolute_start,
                    absolute_end,
                    text,
                    source_track,
                    SegmentState::Final,
                )
                .map_err(anyhow::Error::msg)?,
            );
            emitted = emitted.max(absolute_end);
        }
        quiet = quiet.max(emitted);
        scanner.discard_before(emitted.max(quiet));
        Ok((segments, emitted, quiet))
    }
}

impl FileProcessor for NativeFileProcessor {
    fn process(
        &mut self,
        job: &Transcription,
        set_total: &mut dyn FnMut(usize) -> std::result::Result<(), String>,
        emit: &mut dyn FnMut(TranscriptSegment) -> std::result::Result<(), String>,
        is_cancelled: &dyn Fn() -> std::result::Result<bool, String>,
    ) -> std::result::Result<f64, String> {
        let (audio, duration) =
            load_wav(Path::new(&job.source_path)).map_err(|error| error.to_string())?;
        let regions = detect_speech(&audio, &mut self.vad).map_err(|error| error.to_string())?;
        let chunks = chunk_speech(&regions, audio.len());
        set_total(chunks.len())?;
        for (index, chunk) in chunks.into_iter().enumerate() {
            if is_cancelled()? {
                return Err("transcription cancelled".into());
            }
            let text = self
                .transcribe_chunk(&audio[chunk.start..chunk.end])
                .map_err(|error| error.to_string())?;
            emit(
                TranscriptSegment::new(
                    &job.id,
                    index,
                    chunk.start as f64 / SAMPLE_RATE as f64,
                    chunk.end as f64 / SAMPLE_RATE as f64,
                    text,
                    SourceTrack::File,
                    SegmentState::Final,
                )
                .map_err(str::to_owned)?,
            )?;
        }
        Ok(duration.as_secs_f64())
    }
}

fn process_rss(system: &mut System) -> Result<u64> {
    let pid = get_current_pid().map_err(anyhow::Error::msg)?;
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), false);
    Ok(system
        .process(pid)
        .context("current process is missing")?
        .memory())
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixing_sums_tracks_and_pads_shorter() {
        let dir =
            std::env::temp_dir().join(format!("gigaam-mix-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let a_path = dir.join("a.wav");
        let b_path = dir.join("b.wav");
        let mut writer = hound::WavWriter::create(&a_path, spec).unwrap();
        for i in 0..48_000 {
            let sample = ((i as f32 / 48_000.0 * 440.0 * 2.0 * PI).sin() * 10_000.0) as i16;
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        let mut writer = hound::WavWriter::create(&b_path, spec).unwrap();
        for _ in 0..24_000 {
            writer.write_sample(5_000i16).unwrap();
        }
        writer.finalize().unwrap();
        let out_path = dir.join("mixed.wav");
        mix_mono_48k_to_wav(&[(a_path, 0.0), (b_path, 0.0)], &out_path).unwrap();
        let mut reader = hound::WavReader::open(&out_path).unwrap();
        assert_eq!(reader.spec().sample_rate, 48_000);
        let samples: Vec<i16> = reader.samples().map(|sample| sample.unwrap()).collect();
        assert_eq!(samples.len(), 48_000);
        // first sample: A is sin(0)=0, B contributes ~5000 (f32 rounding ±2)
        assert!((samples[0] - 5_000).abs() <= 2, "first was {}", samples[0]);
        // second half: only track A (B padded with zeros), sine RMS ~7071
        let tail = &samples[24_000..];
        let rms =
            (tail.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>() / tail.len() as f64)
                .sqrt();
        assert!(rms > 3_000.0, "tail rms was {rms}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mixing_respects_track_offsets() {
        let dir =
            std::env::temp_dir().join(format!("gigaam-mixoff-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let a_path = dir.join("a.wav");
        let mut writer = hound::WavWriter::create(&a_path, spec).unwrap();
        for _ in 0..4_800 {
            writer.write_sample(10_000i16).unwrap();
        }
        writer.finalize().unwrap();
        let out_path = dir.join("mixed.wav");
        // track starts 0.5 s in: first half silence, second half signal
        mix_mono_48k_to_wav(&[(a_path, 0.5)], &out_path).unwrap();
        let mut reader = hound::WavReader::open(&out_path).unwrap();
        let samples: Vec<i16> = reader.samples().map(|sample| sample.unwrap()).collect();
        assert_eq!(samples.len(), 24_000 + 4_800);
        assert!(samples[..24_000].iter().all(|sample| *sample == 0));
        assert!(samples[24_000..].iter().all(|sample| (*sample - 10_000).abs() <= 2));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ctc_collapses_repeats_and_blank() {
        let logits = Array2::from_shape_vec(
            (6, 3),
            vec![
                9.0, 0.0, 0.0, 8.0, 0.0, 0.0, 0.0, 0.0, 9.0, 0.0, 8.0, 0.0, 0.0, 8.0, 0.0, 0.0,
                0.0, 9.0,
            ],
        )
        .unwrap();
        let vocab = vec!["а".into(), "б".into(), "<blk>".into()];
        assert_eq!(decode_ctc(logits.view(), &vocab).unwrap(), "аб");
    }

    #[test]
    fn resampling_keeps_audio_at_the_end() {
        let mut input = vec![0.0; 48_000];
        input[47_000..].fill(1.0);
        let output = resample(input, 48_000).unwrap();
        assert_eq!(output.len(), 16_000);
        assert!(output[15_500..].iter().any(|sample| sample.abs() > 0.5));
    }

    #[test]
    fn vad_boundaries_keep_timeline_and_padding() {
        let mut probabilities = vec![0.0; 100];
        probabilities[10..40].fill(0.9);
        let regions = regions_from_probabilities(&probabilities, 100 * VAD_FRAME);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].start, 10 * VAD_FRAME - SPEECH_PAD_SAMPLES);
        assert_eq!(regions[0].end, 40 * VAD_FRAME + SPEECH_PAD_SAMPLES);
    }

    #[test]
    fn chunking_splits_at_pause_after_preferred_duration() {
        let regions = [
            SpeechRegion {
                start: 0,
                end: SAMPLE_RATE * 16,
            },
            SpeechRegion {
                start: SAMPLE_RATE * 17,
                end: SAMPLE_RATE * 20,
            },
        ];
        assert_eq!(chunk_speech(&regions, SAMPLE_RATE * 20), regions);
    }

    #[test]
    fn chunking_enforces_limits_and_bounds() {
        for duration in [23, 30, 44, 60] {
            let chunks = chunk_speech(
                &[SpeechRegion {
                    start: 0,
                    end: SAMPLE_RATE * duration,
                }],
                SAMPLE_RATE * duration,
            );
            assert!(chunks.iter().all(|chunk| {
                chunk.start < chunk.end
                    && chunk.end <= SAMPLE_RATE * duration
                    && chunk.end - chunk.start <= PREFERRED_CHUNK_SAMPLES
                    && chunk.end - chunk.start <= MAX_CHUNK_SAMPLES
            }));
            assert!(chunks.windows(2).all(|pair| pair[0].end == pair[1].start));
        }
    }

    #[test]
    fn chunking_does_not_leave_sub_fft_tail() {
        let audio_len = PREFERRED_CHUNK_SAMPLES + FFT_SIZE - 1;
        let chunks = chunk_speech(
            &[SpeechRegion {
                start: 0,
                end: audio_len,
            }],
            audio_len,
        );
        assert_eq!(
            chunks,
            [SpeechRegion {
                start: 0,
                end: audio_len
            }]
        );
        assert!(chunks[0].end - chunks[0].start <= MAX_CHUNK_SAMPLES);
    }

    #[test]
    fn chunking_repairs_sub_fft_tail_before_another_region() {
        let first_end = PREFERRED_CHUNK_SAMPLES + FFT_SIZE - 1;
        let chunks = chunk_speech(
            &[
                SpeechRegion {
                    start: 0,
                    end: first_end,
                },
                SpeechRegion {
                    start: SAMPLE_RATE * 40,
                    end: SAMPLE_RATE * 45,
                },
            ],
            SAMPLE_RATE * 45,
        );
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.end - chunk.start >= FFT_SIZE)
        );
        assert_eq!(chunks[0].end, first_end);
    }

    #[test]
    fn vad_boundaries_handle_eof_and_clipped_padding() {
        let mut probabilities = vec![0.9; 8];
        assert_eq!(
            regions_from_probabilities(&probabilities, 8 * VAD_FRAME),
            [SpeechRegion {
                start: 0,
                end: 8 * VAD_FRAME
            }]
        );
        probabilities.fill(0.0);
        assert!(regions_from_probabilities(&probabilities, 8 * VAD_FRAME).is_empty());
    }

    fn sine_48k(samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin() * 0.5)
            .collect()
    }

    #[test]
    fn live_scanner_streams_deterministically() {
        let input = sine_48k(48_000);
        let mut whole = LiveScanner::new().unwrap();
        whole.push(&input, 0).unwrap();
        let mut split = LiveScanner::new().unwrap();
        // capture buffers are cumulative: every push carries the whole
        // buffer from the base, only longer than the previous one.
        for end in [100, 5_100, 25_100, 48_000] {
            split.push(&input[..end], 0).unwrap();
        }
        split.finish().unwrap();
        whole.finish().unwrap();
        assert_eq!(whole.fed_48k(), 48_000);
        assert_eq!(split.fed_48k(), 48_000);
        assert_eq!(whole.samples_16k(), split.samples_16k());
        let expected = (48_000.0 * SAMPLE_RATE as f64 / 48_000.0).round() as usize;
        // streaming resampling keeps the filter tail that one-shot
        // resample() truncates via take(target_len): allow filter tolerance.
        assert!((whole.samples_16k().len() as i64 - expected as i64).abs() <= 2048);
        assert!(whole.samples_16k().iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn live_scanner_handles_capture_trim() {
        let input = sine_48k(96_000);
        let mut scanner = LiveScanner::new().unwrap();
        scanner.push(&input, 0).unwrap();
        // streaming VAD would have covered the buffer by now
        scanner.set_test_probs(usize::MAX);
        let before = scanner.samples_16k().len();
        assert!(before > 16_000);
        scanner.push(&input[48_000..], 48_000).unwrap();
        assert_eq!(scanner.fed_48k(), 96_000);
        // drops are quantized to whole VAD frames: abs0 lands within one
        // frame (512 samples = 1536 input samples) of the capture base, and
        // the sample/probability/absolute-time mapping stays exact.
        assert!((48_000i64 - scanner.abs0_48k() as i64).abs() <= 1536);
        assert_eq!(
            before - scanner.samples_16k().len(),
            scanner.abs0_48k() / 3
        );
        scanner.push(&input[48_000..], 48_000).unwrap();
        assert_eq!(scanner.fed_48k(), 96_000);
    }

    #[test]
    fn live_scanner_discard_before_keeps_alignment() {
        let input = sine_48k(96_000);
        let mut scanner = LiveScanner::new().unwrap();
        scanner.push(&input, 0).unwrap();
        scanner.set_test_probs(usize::MAX);
        let before = scanner.samples_16k().len();
        assert!(before >= 15_872 + 512);
        scanner.discard_before(1.0);
        // 1 s = 16000 samples -> 31 whole VAD frames = 15872 samples
        assert_eq!(scanner.abs0_48k(), 15_872 * 3);
        assert_eq!(scanner.samples_16k().len(), before - 15_872);
        scanner.discard_before(0.5);
        assert_eq!(scanner.abs0_48k(), 15_872 * 3);
    }
}
