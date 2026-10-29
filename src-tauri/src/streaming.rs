//! Streaming audio decoding for file transcription.
//!
//! The file pipeline used to decode a whole recording into one `Vec<f32>`
//! before VAD/ASR. Multi-hour recordings made that O(duration) in RAM. This
//! module exposes the same decoding/downmix semantics as a bounded block
//! producer so callers only ever hold one block (plus the streaming resampler
//! state) at a time.
//!
//! It is deliberately platform independent: Windows WASAPI and macOS
//! ScreenCaptureKit adapters stay in `live::platform`.

use std::{fs, path::Path};

use anyhow::{Context, Result, bail};
use rubato::{FftFixedIn, Resampler};
use symphonia::core::{
    audio::sample::Sample,
    codecs::audio::{AudioDecoder, AudioDecoderOptions},
    errors::Error as SymphoniaError,
    formats::{FormatOptions, FormatReader, TrackType, probe::Hint},
    io::MediaSourceStream,
    meta::MetadataOptions,
};

/// Frames requested per block. 16 384 frames is ~0.37 s at 44.1 kHz and keeps
/// the downstream 16 kHz window small without adding per-block overhead.
pub(crate) const SOURCE_BLOCK_FRAMES: usize = 16_384;

/// Consume a synthetic stream without ASR and report RSS plateau evidence.
/// This intentionally measures the decoder/resampler/scanner orchestration,
/// not model memory, so 180-minute acceptance does not burn inference time.
pub(crate) fn rss_plateau_smoke(minutes: &[u64]) -> Result<Vec<(u64, u64, u64)>> {
    let mut system = sysinfo::System::new();
    let mut rows = Vec::new();
    for &minutes in minutes {
        let source_samples = minutes as usize * 60 * 16_000;
        let mut scanner = crate::pipeline::LiveScanner::with_input_rate(16_000)?;
        let mut processed = 0usize;
        let before = process_rss(&mut system)?;
        while processed < source_samples {
            let count = (source_samples - processed).min(SOURCE_BLOCK_FRAMES);
            // Deterministic low-amplitude signal; no whole-duration buffer.
            let block = vec![0.0_f32; count];
            scanner.push_file_block(&block, processed)?;
            processed += count;
            if processed % (SOURCE_BLOCK_FRAMES * 64) == 0 {
                scanner.compact_audio_tail(16_000);
            }
        }
        scanner.finish()?;
        scanner.trim_to_input_end();
        let after = process_rss(&mut system)?;
        rows.push((minutes, before, after));
    }
    Ok(rows)
}

fn process_rss(system: &mut sysinfo::System) -> Result<u64> {
    let pid = sysinfo::get_current_pid().map_err(|error| anyhow::anyhow!(error))?;
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    Ok(system
        .process(pid)
        .map(|process| process.memory())
        .unwrap_or(0))
}

/// A bounded producer of mono f32 frames at the source sample rate.
///
/// Implementations must not buffer the whole file: `next_block` returns at
/// most the configured frame count and returns `None` only at end of stream.
pub(crate) trait AudioBlockSource {
    fn sample_rate(&self) -> usize;
    /// Frames advertised by the container, when reliable.
    fn total_frames_hint(&self) -> Option<u64>;
    fn next_block(&mut self) -> Result<Option<Vec<f32>>>;
}

pub(crate) fn open_audio_blocks(path: &Path) -> Result<Box<dyn AudioBlockSource>> {
    {
        use std::io::Read;
        let mut magic = [0u8; 4];
        let mut file = std::fs::File::open(path)?;
        if file.read(&mut magic)? == 4 && &magic == b"OggS" {
            return Ok(Box::new(OpusBlockSource::open(path)?));
        }
    }
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("wav"))
    {
        return Ok(Box::new(WavBlockSource::open(path)?));
    }
    Ok(Box::new(SymphoniaBlockSource::open(path)?))
}

struct OpusBlockSource {
    reader: opus_pure::OggOpusReader<std::io::BufReader<std::fs::File>>,
    decoder: opus_pure::OpusDecoder,
    channels: usize,
    skip: usize,
}
impl OpusBlockSource {
    fn open(path: &Path) -> Result<Self> {
        let reader = opus_pure::OggOpusReader::new(std::io::BufReader::new(std::fs::File::open(path)?))?;
        let head = reader.head().clone();
        Ok(Self { decoder: head.decoder(48000)?, channels: head.channel_count as usize, skip: head.pre_skip as usize, reader })
    }
}
impl AudioBlockSource for OpusBlockSource {
    fn sample_rate(&self) -> usize { 48000 }
    fn total_frames_hint(&self) -> Option<u64> { None }
    fn next_block(&mut self) -> Result<Option<Vec<f32>>> {
        loop {
            let Some(packet) = self.reader.packets().next() else { return Ok(None); };
            let packet = packet?;
            let mut samples = vec![0.0; opus_pure::MAX_PACKET_SAMPLES * self.channels];
            let frames = self.decoder.decode(&packet.data, opus_pure::MAX_PACKET_SAMPLES, &mut samples)?;
            let skip = self.skip.min(frames); self.skip -= skip;
            let mono: Vec<f32> = samples[skip * self.channels..frames * self.channels].chunks(self.channels).map(|frame| frame.iter().sum::<f32>() / self.channels as f32).collect();
            if !mono.is_empty() { return Ok(Some(mono)); }
        }
    }
}

struct WavBlockSource {
    samples: Box<dyn Iterator<Item = Result<f32>> + Send>,
    channels: usize,
    sample_rate: usize,
    total_frames: Option<u64>,
    block_frames: usize,
}

impl WavBlockSource {
    fn open(path: &Path) -> Result<Self> {
        let reader = hound::WavReader::open(path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        let spec = reader.spec();
        if spec.channels == 0 {
            bail!("WAV has no channels");
        }
        if spec.sample_rate == 0 {
            bail!("WAV has an invalid sample rate");
        }
        let total_frames = u64::from(reader.duration());
        let bits = spec.bits_per_sample;
        let samples: Box<dyn Iterator<Item = Result<f32>> + Send> = match spec.sample_format {
            hound::SampleFormat::Int if bits <= 16 => {
                let scale = (1_i32 << bits.saturating_sub(1)) as f32;
                Box::new(
                    reader
                        .into_samples::<i16>()
                        .map(move |sample| Ok(sample? as f32 / scale)),
                )
            }
            hound::SampleFormat::Int if bits <= 32 => {
                let scale = (1_i64 << bits.saturating_sub(1)) as f32;
                Box::new(
                    reader
                        .into_samples::<i32>()
                        .map(move |sample| Ok(sample? as f32 / scale)),
                )
            }
            hound::SampleFormat::Float if bits == 32 => {
                Box::new(reader.into_samples::<f32>().map(|sample| {
                    let sample = sample?;
                    if !sample.is_finite() {
                        bail!("WAV contains a non-finite sample");
                    }
                    Ok(sample.clamp(-1.0, 1.0))
                }))
            }
            _ => bail!("expected integer PCM up to 32-bit or 32-bit float WAV"),
        };
        Ok(Self {
            samples,
            channels: spec.channels as usize,
            sample_rate: spec.sample_rate as usize,
            total_frames: Some(total_frames),
            block_frames: SOURCE_BLOCK_FRAMES,
        })
    }
}

impl AudioBlockSource for WavBlockSource {
    fn sample_rate(&self) -> usize {
        self.sample_rate
    }

    fn total_frames_hint(&self) -> Option<u64> {
        self.total_frames
    }

    fn next_block(&mut self) -> Result<Option<Vec<f32>>> {
        let mut interleaved = Vec::with_capacity(self.block_frames * self.channels);
        while interleaved.len() < self.block_frames * self.channels {
            match self.samples.next() {
                Some(sample) => interleaved.push(sample?),
                None => break,
            }
        }
        if interleaved.is_empty() {
            return Ok(None);
        }
        Ok(Some(downmix(&interleaved, self.channels)))
    }
}

struct SymphoniaBlockSource {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    sample_rate: usize,
    total_frames_hint: Option<u64>,
    mono: Vec<f32>,
    mono_start: usize,
    eof: bool,
    block_frames: usize,
}

impl SymphoniaBlockSource {
    fn open(path: &Path) -> Result<Self> {
        let file = Box::new(fs::File::open(path)?);
        let stream = MediaSourceStream::new(file, Default::default());
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
            hint.with_extension(extension);
        }
        let format = symphonia::default::get_probe().probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )?;
        let track = format
            .default_track(TrackType::Audio)
            .context("media contains no audio track")?;
        let track_id = track.id;
        let total_frames_hint = track.num_frames;
        let audio_params = track
            .codec_params
            .as_ref()
            .context("audio codec parameters are missing")?
            .audio()
            .context("audio codec parameters are invalid")?;
        let sample_rate = audio_params
            .sample_rate
            .context("media sample rate is unknown")? as usize;
        if sample_rate == 0 {
            bail!("media has an invalid sample rate");
        }
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(audio_params, &AudioDecoderOptions::default())?;
        Ok(Self {
            format,
            decoder,
            track_id,
            sample_rate,
            total_frames_hint,
            mono: Vec::new(),
            mono_start: 0,
            eof: false,
            block_frames: SOURCE_BLOCK_FRAMES,
        })
    }

    fn decode_more(&mut self) -> Result<()> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    self.eof = true;
                    return Ok(());
                }
                Err(SymphoniaError::IoError(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    self.eof = true;
                    return Ok(());
                }
                Err(error) => return Err(error.into()),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(buffer) => {
                    let spec = buffer.spec();
                    if spec.rate() as usize != self.sample_rate {
                        bail!("media sample rate changed mid-stream");
                    }
                    let channels = spec.channels().count().max(1);
                    let mut interleaved = vec![f32::MID; buffer.samples_interleaved()];
                    buffer.copy_to_slice_interleaved(&mut interleaved);
                    self.mono.extend(downmix(&interleaved, channels));
                    return Ok(());
                }
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl AudioBlockSource for SymphoniaBlockSource {
    fn sample_rate(&self) -> usize {
        self.sample_rate
    }

    fn total_frames_hint(&self) -> Option<u64> {
        self.total_frames_hint
    }

    fn next_block(&mut self) -> Result<Option<Vec<f32>>> {
        while self.mono.len() - self.mono_start < self.block_frames && !self.eof {
            self.decode_more()?;
        }
        let available = self.mono.len() - self.mono_start;
        if available == 0 {
            return Ok(None);
        }
        let take = available.min(self.block_frames);
        let block = self.mono[self.mono_start..self.mono_start + take].to_vec();
        self.mono_start += take;
        if self.mono_start == self.mono.len() {
            self.mono.clear();
            self.mono_start = 0;
        }
        Ok(Some(block))
    }
}

fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    let channels = channels.max(1);
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Decode a whole source through the block API. Only used by tests and small
/// diagnostics; production callers consume blocks incrementally.
#[cfg(test)]
pub(crate) fn decode_all(path: &Path) -> Result<(Vec<f32>, usize)> {
    let mut source = open_audio_blocks(path)?;
    let rate = source.sample_rate();
    let mut all = Vec::new();
    while let Some(block) = source.next_block()? {
        all.extend(block);
    }
    Ok((all, rate))
}

/// Streaming resampler with a global input/output sample timeline.
///
/// Only `pending` (less than one filter block) plus the current output block
/// are held in memory. The filter delay is skipped once, so output sample 0
/// corresponds to input sample 0.
pub(crate) struct BlockResampler {
    /// `None` is an exact 1:1 passthrough (no filter delay or colouring).
    resampler: Option<FftFixedIn<f32>>,
    pending: Vec<f32>,
    skip_delay: usize,
    chunk: usize,
    input_rate: usize,
    output_rate: usize,
    input_total: usize,
    emitted_total: usize,
}

impl BlockResampler {
    pub(crate) fn new(input_rate: usize, output_rate: usize) -> Result<Self> {
        if input_rate == 0 || output_rate == 0 {
            bail!("resampler rates must be positive");
        }
        if input_rate == output_rate {
            return Ok(Self {
                resampler: None,
                pending: Vec::new(),
                skip_delay: 0,
                chunk: 0,
                input_rate,
                output_rate,
                input_total: 0,
                emitted_total: 0,
            });
        }
        let resampler = FftFixedIn::<f32>::new(input_rate, output_rate, 1024, 2, 1)?;
        let skip_delay = resampler.output_delay();
        let chunk = resampler.input_frames_next();
        Ok(Self {
            resampler: Some(resampler),
            pending: Vec::new(),
            skip_delay,
            chunk,
            input_rate,
            output_rate,
            input_total: 0,
            emitted_total: 0,
        })
    }

    pub(crate) fn push(&mut self, input: &[f32]) -> Result<Vec<f32>> {
        self.input_total += input.len();
        let Some(resampler) = self.resampler.as_mut() else {
            self.emitted_total += input.len();
            return Ok(input.to_vec());
        };
        self.pending.extend_from_slice(input);
        let mut output = Vec::new();
        while self.pending.len() >= self.chunk {
            let block = resampler
                .process(&[&self.pending[..self.chunk]], None)?
                .remove(0);
            self.pending.drain(..self.chunk);
            Self::append(&mut output, block, &mut self.skip_delay);
        }
        self.emitted_total += output.len();
        Ok(output)
    }

    /// Flush the filter tail and truncate it to the exact source timeline.
    /// Streaming filters emit padding past the input length; the batch
    /// resampler truncated that tail, so archive mixes must not grow.
    pub(crate) fn finish(&mut self) -> Result<Vec<f32>> {
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(Vec::new());
        };
        let mut output = Vec::new();
        if !self.pending.is_empty() {
            let tail = std::mem::take(&mut self.pending);
            let block = resampler
                .process_partial(Some(&[tail.as_slice()]), None)?
                .remove(0);
            Self::append(&mut output, block, &mut self.skip_delay);
        }
        let block = resampler.process_partial::<&[f32]>(None, None)?.remove(0);
        Self::append(&mut output, block, &mut self.skip_delay);
        let target = (self.input_total as f64 * self.output_rate as f64 / self.input_rate as f64)
            .round() as usize;
        output.truncate(target.saturating_sub(self.emitted_total));
        self.emitted_total += output.len();
        Ok(output)
    }

    fn append(output: &mut Vec<f32>, mut block: Vec<f32>, skip_delay: &mut usize) {
        if *skip_delay > 0 {
            let skip = (*skip_delay).min(block.len());
            block.drain(..skip);
            *skip_delay -= skip;
        }
        output.extend(block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures")
            .join(name)
    }

    /// Frozen copy of the pre-phase-02 batch WAV decoder used as the
    /// independent reference (whole-file hound read + downmix).
    fn batch_decode_wav(path: &Path) -> (Vec<f32>, usize) {
        let mut reader = hound::WavReader::open(path).unwrap();
        let spec = reader.spec();
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Int if spec.bits_per_sample <= 16 => reader
                .samples::<i16>()
                .map(|sample| sample.unwrap() as f32 / (1_i32 << (spec.bits_per_sample - 1)) as f32)
                .collect(),
            _ => panic!("fixture format"),
        };
        let channels = spec.channels as usize;
        let mono = samples
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect();
        (mono, spec.sample_rate as usize)
    }

    #[test]
    fn block_decoder_matches_batch_wav_decoder() {
        for name in ["besarab-clean.wav", "besarab-dual.wav"] {
            let path = fixture(name);
            let (batch, batch_rate) = batch_decode_wav(&path);
            let (streamed, stream_rate) = decode_all(&path).unwrap();
            assert_eq!(batch_rate, stream_rate);
            assert_eq!(batch.len(), streamed.len(), "{name} sample count");
            for (index, (left, right)) in batch.iter().zip(streamed.iter()).enumerate() {
                assert!(
                    (left - right).abs() < 1e-6,
                    "{name} sample {index}: {left} != {right}"
                );
            }
        }
    }

    #[test]
    fn block_decoder_reads_compressed_media_in_bounded_blocks() {
        let directory =
            std::env::temp_dir().join(format!("gigaam-stream-flac-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let input: Vec<f32> = (0..96_000)
            .map(|i| (2.0 * std::f32::consts::PI * 330.0 * i as f32 / 48_000.0).sin() * 0.4)
            .collect();
        let path = directory.join("tones.flac");
        crate::live::encode_mono_48k(&input, &path, crate::live::ArchiveFormat::Flac).unwrap();

        let mut source = open_audio_blocks(&path).unwrap();
        assert_eq!(source.sample_rate(), 48_000);
        let mut total = 0usize;
        while let Some(block) = source.next_block().unwrap() {
            assert!(block.len() <= SOURCE_BLOCK_FRAMES);
            total += block.len();
        }
        assert_eq!(total, input.len());
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn stream_resampler_matches_batch_resampler_timeline() {
        for rate in [8_000usize, 22_050, 44_100, 48_000] {
            let seconds = 3.0;
            let input: Vec<f32> = (0..(rate as f64 * seconds) as usize)
                .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate as f32).sin() * 0.5)
                .collect();
            let mut stream = BlockResampler::new(rate, 16_000).unwrap();
            let mut streamed = Vec::new();
            for block in input.chunks(997) {
                streamed.extend(stream.push(block).unwrap());
            }
            streamed.extend(stream.finish().unwrap());
            let expected_len = (seconds * 16_000.0).round() as usize;
            assert!(
                streamed.len().abs_diff(expected_len) <= 2048,
                "{rate} Hz: {} vs {expected_len}",
                streamed.len()
            );
            assert!(streamed.iter().all(|sample| sample.is_finite()));
            // The end of the input must reach the output (filter tail flushed).
            assert!(
                streamed[expected_len.saturating_sub(2048)..]
                    .iter()
                    .any(|sample| sample.abs() > 0.1),
                "{rate} Hz: tail lost"
            );
        }
    }

    #[test]
    fn corrupt_sources_surface_as_errors() {
        let corrupt = fixture("corrupt.wav");
        assert!(open_audio_blocks(&corrupt).is_err());
    }

    #[test]
    #[ignore = "resource measurement; run explicitly for phase 02 evidence"]
    fn synthetic_15_60_180_minute_rss_plateau() {
        let rows = rss_plateau_smoke(&[15, 60, 180]).unwrap();
        for (minutes, before, after) in &rows {
            println!("minutes={minutes} rss_before_bytes={before} rss_after_bytes={after}");
            assert!(
                after.saturating_sub(*before) < 128 * 1024 * 1024,
                "{minutes} min RSS grew by {} bytes",
                after.saturating_sub(*before)
            );
        }
    }
}
