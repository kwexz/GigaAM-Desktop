//! Helpers for applying offline speaker turns without changing audio or text.
use crate::domain::TranscriptSegment;
use anyhow::{Context, Result};
use ort::{session::Session, value::Tensor};
use std::path::Path;
use std::io::{Read, Write};
use sha2::{Digest, Sha256};

const SPEAKER_MODEL_BYTES: u64 = 26_534_127;
const SPEAKER_MODEL_SHA256: &str = "9fea6516d7ad6bf0a76c7689f5a49b65d330fad6dde96c91bb4435ffbfe056a1";
const SPEAKER_MODEL_URL: &str = "https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34/resolve/ff1ac5bca8ef11e90662b879aa923979e0bd277b/voxceleb_resnet34.onnx";

/// Called only by an explicit user action; writes a verified temporary file.
pub fn download_speaker_model(directory: &Path) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(directory)?;
    let path = directory.join("wespeaker-resnet34.onnx");
    if verify_speaker_model(&path).is_ok() { return Ok(path); }
    let partial = directory.join("wespeaker-resnet34.download");
    let result = (|| -> Result<()> {
        let client = reqwest::blocking::Client::builder().connect_timeout(std::time::Duration::from_secs(15)).timeout(std::time::Duration::from_secs(600)).build()?;
        let mut response = client.get(SPEAKER_MODEL_URL).send()?.error_for_status()?;
        let mut file = std::fs::File::create(&partial)?;
        let mut buffer = [0; 65536];
        let mut size = 0u64;
        loop {
            let count = response.read(&mut buffer)?;
            if count == 0 { break; }
            size += count as u64;
            anyhow::ensure!(size <= SPEAKER_MODEL_BYTES, "speaker model exceeds expected size");
            file.write_all(&buffer[..count])?;
        }
        file.sync_all()?;
        drop(file);
        verify_speaker_model(&partial)?;
        if path.exists() { anyhow::bail!("invalid existing model; remove it manually before retrying"); }
        std::fs::rename(&partial, &path)?;
        Ok(())
    })();
    if result.is_err() { let _ = std::fs::remove_file(&partial); }
    result?;
    Ok(path)
}

pub fn verify_speaker_model(path: &Path) -> Result<()> {
    anyhow::ensure!(std::fs::metadata(path)?.len() == SPEAKER_MODEL_BYTES, "speaker model size mismatch");
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 65536];
    loop { let n = file.read(&mut buffer)?; if n == 0 { break; } hasher.update(&buffer[..n]); }
    anyhow::ensure!(format!("{:x}", hasher.finalize()) == SPEAKER_MODEL_SHA256, "speaker model checksum mismatch");
    Ok(())
}

/// Explicitly loaded speaker model. The application never downloads it here.
pub struct SpeakerEncoder { session: Session }

impl SpeakerEncoder {
    pub fn load(path: &Path) -> Result<Self> {
        let mut builder = Session::builder()?.with_intra_threads(1).map_err(|error| anyhow::anyhow!("speaker session setup failed: {error:?}"))?;
        Ok(Self { session: builder.commit_from_file(path)? })
    }

    pub fn embed(&mut self, audio: &[f32]) -> Result<Vec<f32>> {
        let rows = crate::speaker_features::speaker_features(audio).map_err(anyhow::Error::msg)?;
        let frames = rows.len();
        let values: Vec<f32> = rows.into_iter().flatten().collect();
        let outputs = self.session.run(ort::inputs!["feats" => Tensor::from_array(([1usize, frames, 80usize], values.into_boxed_slice()))?])?;
        let output = outputs.get("embs").context("speaker model has no embs output")?.try_extract_array::<f32>()?;
        let embedding: Vec<f32> = output.iter().copied().collect();
        anyhow::ensure!(!embedding.is_empty() && embedding.iter().all(|value| value.is_finite()), "invalid speaker embedding");
        Ok(embedding)
    }
}

/// Analyze bounded windows of a completed recording. Short fragments remain
/// unlabelled rather than being assigned invented speaker identities.
pub fn analyze_file(path: &Path, model: &Path, segments: &[TranscriptSegment]) -> Result<Vec<Option<String>>> {
    let mut source = crate::streaming::open_audio_blocks(path)?;
    let rate = source.sample_rate();
    anyhow::ensure!(rate > 0, "invalid sample rate");
    let mut encoder = SpeakerEncoder::load(model)?;
    let mut vectors = Vec::new();
    let mut positions = Vec::new();
    let mut order: Vec<usize> = (0..segments.len()).collect();
    order.sort_by(|&a, &b| segments[a].start_seconds.total_cmp(&segments[b].start_seconds));
    let mut active: Vec<(usize, Vec<f32>)> = Vec::new();
    let mut cursor = 0usize;
    let mut sample_index = 0usize;
    while let Some(block) = source.next_block()? {
        let block_start = sample_index;
        let block_end = block_start + block.len();
        while cursor < order.len() && (segments[order[cursor]].start_seconds.max(0.0) * rate as f64) < block_end as f64 {
            active.push((order[cursor], Vec::new()));
            cursor += 1;
        }
        for (index, window) in &mut active {
            let segment = &segments[*index];
            let start = (segment.start_seconds.max(0.0) * rate as f64) as usize;
            let end = ((segment.end_seconds.min(segment.start_seconds + 3.0)) * rate as f64) as usize;
            let lo = start.max(block_start);
            let hi = end.min(block_end);
            if hi > lo { window.extend_from_slice(&block[lo - block_start..hi - block_start]); }
        }
        let mut pending = Vec::new();
        for (index, audio) in active.drain(..) {
            let segment = &segments[index];
            let end = (segment.end_seconds.min(segment.start_seconds + 3.0) * rate as f64) as usize;
            if end > block_end { pending.push((index, audio)); continue; }
            add_embedding(&mut encoder, rate, index, audio, &mut positions, &mut vectors)?;
        }
        active = pending;
        sample_index = block_end;
    }
    for (index, audio) in active { add_embedding(&mut encoder, rate, index, audio, &mut positions, &mut vectors)?; }
    let labels = cluster_two(&vectors).map_err(anyhow::Error::msg)?;
    let mut result: Vec<_> = segments.iter().map(|segment| segment.speaker_id.clone()).collect();
    for (index, speaker) in positions.into_iter().zip(labels) { result[index] = Some(format!("Speaker {}", speaker + 1)); }
    Ok(result)
}

fn add_embedding(encoder: &mut SpeakerEncoder, rate: usize, index: usize, audio: Vec<f32>, positions: &mut Vec<usize>, vectors: &mut Vec<Vec<f32>>) -> Result<()> {
    if audio.len() < rate / 2 { return Ok(()); }
    let resampled = if rate == 16000 { audio } else {
        let count = audio.len() * 16000 / rate;
        (0..count).map(|i| {
            let pos = i as f64 * rate as f64 / 16000.0;
            let left = pos as usize;
            let right = (left + 1).min(audio.len() - 1);
            let fraction = (pos - left as f64) as f32;
            audio[left] * (1.0 - fraction) + audio[right] * fraction
        }).collect()
    };
    vectors.push(encoder.embed(&resampled)?);
    positions.push(index);
    Ok(())
}

#[derive(Clone, Debug)]
pub struct SpeakerTurn {
    pub start: f64,
    pub end: f64,
    pub speaker: usize,
}

/// Two-speaker clustering for normalized or unnormalized embeddings.
/// No labels are returned for degenerate or inconsistent input.
pub fn cluster_two(vectors: &[Vec<f32>]) -> Result<Vec<usize>, &'static str> {
    if vectors.len() < 2 || vectors[0].is_empty() { return Err("not enough speaker embeddings"); }
    let width = vectors[0].len();
    let normalized: Vec<Vec<f32>> = vectors.iter().map(|v| {
        if v.len() != width || v.iter().any(|x| !x.is_finite()) { return Err("invalid speaker embedding"); }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm < 1e-6 { return Err("empty speaker embedding"); }
        Ok(v.iter().map(|x| x / norm).collect())
    }).collect::<Result<_, _>>()?;
    let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    let (mut left, mut right, mut minimum) = (0, 1, f32::INFINITY);
    for i in 0..normalized.len() { for j in i + 1..normalized.len() {
        let similarity = dot(&normalized[i], &normalized[j]);
        if similarity < minimum { (left, right, minimum) = (i, j, similarity); }
    }}
    // A pair of outliers is not evidence of a second speaker.
    let mut centers = [normalized[left].clone(), normalized[right].clone()];
    let mut labels = vec![0; vectors.len()];
    for _ in 0..20 {
        let mut sums = [vec![0.0; width], vec![0.0; width]];
        let mut counts = [0usize; 2];
        for (i, vector) in normalized.iter().enumerate() {
            let group = usize::from(dot(vector, &centers[1]) > dot(vector, &centers[0]));
            labels[i] = group;
            counts[group] += 1;
            for (sum, value) in sums[group].iter_mut().zip(vector) { *sum += value; }
        }
        if counts.contains(&0) { return Err("speaker clustering collapsed"); }
        for group in 0..2 {
            let norm = sums[group].iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm < 1e-6 { return Err("speaker clustering collapsed"); }
            centers[group] = sums[group].iter().map(|x| x / norm).collect();
        }
    }
    let cross = dot(&centers[0], &centers[1]);
    let mut within = [0.0f32; 2];
    let mut counts = [0usize; 2];
    for (vector, &label) in normalized.iter().zip(&labels) {
        within[label] += dot(vector, &centers[label]);
        counts[label] += 1;
    }
    #[cfg(test)]
    eprintln!("speaker clustering: counts={counts:?}, center_similarity={cross:.3}, within={:.3}/{:.3}", within[0] / counts[0] as f32, within[1] / counts[1] as f32);
    if counts.iter().any(|count| *count < 2) || cross > 0.5 || within.iter().enumerate().any(|(group, score)| score / counts[group] as f32 - cross < 0.13) {
        return Err("Недостаточно уверенное разделение двух голосов. Попробуйте запись с более длинными отдельными репликами; результат не сохранён.");
    }
    Ok(labels)
}

/// Assign a label only when one speaker has unambiguous majority overlap.
pub fn match_speakers(segments: &[TranscriptSegment], turns: &[SpeakerTurn]) -> Vec<Option<String>> {
    segments.iter().map(|segment| {
        let mut totals = std::collections::BTreeMap::<usize, f64>::new();
        for turn in turns {
            if !turn.start.is_finite() || !turn.end.is_finite() || turn.end <= turn.start { continue; }
            let overlap = (segment.end_seconds.min(turn.end) - segment.start_seconds.max(turn.start)).max(0.0);
            *totals.entry(turn.speaker).or_default() += overlap;
        }
        let mut ranked: Vec<_> = totals.into_iter().filter(|(_, duration)| *duration > 0.0).collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        let (speaker, overlap) = *ranked.first()?;
        if overlap < (segment.end_seconds - segment.start_seconds) * 0.5 || ranked.get(1).is_some_and(|other| other.1 >= overlap * 0.8) { return None; }
        Some(format!("Speaker {}", speaker + 1))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{SegmentState, SourceTrack};
    #[test]
    fn leaves_ambiguous_speech_unlabelled() {
        let segment = TranscriptSegment::new("tx", 0, 0.0, 2.0, "text".into(), SourceTrack::File, SegmentState::Final).unwrap();
        let turns = [SpeakerTurn { start: 0.0, end: 1.0, speaker: 0 }, SpeakerTurn { start: 1.0, end: 2.0, speaker: 1 }];
        assert_eq!(match_speakers(&[segment.clone()], &turns), vec![None]);
        assert_eq!(match_speakers(&[segment], &turns[..1]), vec![Some("Speaker 1".into())]);
    }

    #[test]
    fn separates_two_distinct_embedding_groups() {
        let labels = cluster_two(&[vec![1.0, 0.0], vec![0.99, 0.01], vec![0.0, 1.0], vec![0.01, 0.99]]).unwrap();
        assert_eq!(labels[0], labels[1]);
        assert_eq!(labels[2], labels[3]);
        assert_ne!(labels[0], labels[2]);
        assert!(cluster_two(&[vec![1.0, 0.0], vec![1.0, 0.0]]).is_err());
    }

    #[test]
    #[ignore = "requires explicitly downloaded speaker model"]
    fn real_speaker_model_produces_finite_embeddings() {
        let path = std::env::var_os("GIGAAM_SPEAKER_MODEL").expect("set GIGAAM_SPEAKER_MODEL");
        let path = Path::new(&path);
        verify_speaker_model(path).unwrap();
        let mut encoder = SpeakerEncoder::load(path).unwrap();
        let audio: Vec<f32> = (0..32000).map(|i| (i as f32 * 0.07).sin() * 0.1).collect();
        let vector = encoder.embed(&audio).unwrap();
        assert!(!vector.is_empty());
        assert!(vector.iter().all(|value| value.is_finite()));
    }

    #[test]
    #[ignore = "requires explicitly downloaded speaker model"]
    fn real_audio_analysis_preserves_input_segments() {
        let model = std::env::var_os("GIGAAM_SPEAKER_MODEL").expect("set GIGAAM_SPEAKER_MODEL");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/besarab-clean.wav");
        let segments: Vec<_> = (0..4).map(|index| TranscriptSegment::new("sample", index, index as f64 * 3.0, (index + 1) as f64 * 3.0, format!("fragment {index}"), SourceTrack::File, SegmentState::Final).unwrap()).collect();
        let before = segments.clone();
        let result = analyze_file(&fixture, Path::new(&model), &segments);
        assert_eq!(segments, before);
        assert!(result.is_err(), "single-speaker recording must not be forced into two speakers: {result:?}");
    }

    #[test]
    #[ignore = "requires explicitly downloaded synthetic audio and speaker model"]
    fn labeled_two_speaker_fixture() {
        let model = std::env::var_os("GIGAAM_SPEAKER_MODEL").expect("set GIGAAM_SPEAKER_MODEL");
        let audio = std::env::var_os("GIGAAM_DIARIZATION_AUDIO").expect("set GIGAAM_DIARIZATION_AUDIO");
        let reference = std::env::var_os("GIGAAM_DIARIZATION_REFERENCE").expect("set GIGAAM_DIARIZATION_REFERENCE");
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(reference).unwrap()).unwrap();
        let speakers = json["speakers"].as_array().unwrap();
        let starts = json["timestamps_start"].as_array().unwrap();
        let ends = json["timestamps_end"].as_array().unwrap();
        let mut candidates = Vec::new();
        for i in 0..speakers.len() {
            let speaker = speakers[i].as_str().unwrap();
            let start = starts[i].as_f64().unwrap();
            let end = ends[i].as_f64().unwrap();
            if end - start < 2.0 || (0..speakers.len()).any(|j| j != i && starts[j].as_f64().unwrap() < end && ends[j].as_f64().unwrap() > start) { continue; }
            candidates.push((speaker.to_owned(), start, end));
        }
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        for (speaker, _, _) in &candidates { *counts.entry(speaker.clone()).or_default() += 1; }
        let mut ranked: Vec<_> = counts.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1));
        assert!(ranked.len() >= 2 && ranked[1].1 >= 2, "fixture lacks two speakers with two turns each");
        let selected: Vec<_> = candidates.into_iter().filter_map(|(speaker, start, end)| ranked[..2].iter().position(|(name, _)| *name == speaker).map(|label| (label, start, end))).collect();
        let segments: Vec<_> = selected.iter().enumerate().map(|(i, (_, start, end))| TranscriptSegment::new("fixture", i, *start, *end, "speech".into(), SourceTrack::File, SegmentState::Final).unwrap()).collect();
        let labels = analyze_file(Path::new(&audio), Path::new(&model), &segments).unwrap();
        let correct = selected.iter().zip(&labels).filter(|((expected, _, _), actual)| actual.as_ref().is_some_and(|label| label == &format!("Speaker {}", expected + 1))).count();
        let score = correct.max(labels.len() - correct) as f64 / labels.len() as f64;
        println!("non-overlapping turns={}, permutation-invariant accuracy={score:.3}", labels.len());
        assert!(labels.len() >= 4 && score >= 0.75);
    }

    #[test]
    #[ignore = "requires explicitly selected local recording and database"]
    fn diagnostic_local_recording() {
        let model = std::env::var_os("GIGAAM_SPEAKER_MODEL").unwrap();
        let db = std::env::var_os("GIGAAM_DIARIZATION_DB").unwrap();
        let id = std::env::var("GIGAAM_DIARIZATION_ID").unwrap();
        let store = crate::storage::Store::open(Path::new(&db)).unwrap();
        let record = store.get(&id).unwrap().unwrap();
        let segments = store.segments(&id).unwrap();
        let result = analyze_file(Path::new(record.audio_path.as_deref().unwrap_or(&record.source_path)), Path::new(&model), &segments);
        println!("segments={}, outcome={}", segments.len(), if result.is_ok() { "assigned" } else { "rejected" });
        assert!(result.is_ok(), "{}", result.unwrap_err());
    }
}
