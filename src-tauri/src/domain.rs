use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt, str::FromStr};

pub const CONTRACT_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackId {
    Microphone,
    System,
    File,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct AudioBlockEvent {
    pub version: u16,
    pub session_id: String,
    pub track_id: TrackId,
    pub seq: u64,
    pub sample_rate: u32,
    pub absolute_sample_start: u64,
    pub duration_samples: u32,
    pub codec: String,
    pub codec_config: Option<String>,
    pub final_block: bool,
    pub discontinuity: bool,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TranscriptSegmentEvent {
    pub version: u16,
    pub session_id: String,
    pub segment_id: String,
    pub track_id: TrackId,
    pub seq: u64,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub text: String,
    pub final_segment: bool,
    pub revision: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingState {
    Idle,
    Recording,
    Paused,
    Stopping,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadState {
    NotStarted,
    Queued,
    Uploading,
    Durable,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecognitionState {
    NotStarted,
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    Disabled,
    Idle,
    Pending,
    Syncing,
    Synced,
    Conflict,
    Failed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SessionStates {
    pub recording: RecordingState,
    pub upload: UploadState,
    pub recognition: RecognitionState,
    pub sync: SyncState,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionKind {
    File,
    Live,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionStatus {
    Queued,
    Preparing,
    Transcribing,
    Completed,
    Cancelled,
    Failed,
    Interrupted,
}

macro_rules! string_enum {
    ($type:ty, {$($variant:ident => $value:literal),+ $(,)?}) => {
        impl fmt::Display for $type {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(match self { $(Self::$variant => $value),+ })
            }
        }

        impl FromStr for $type {
            type Err = &'static str;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value { $($value => Ok(Self::$variant),)+ _ => Err("invalid enum value") }
            }
        }
    };
}

string_enum!(TranscriptionKind, { File => "file", Live => "live" });
string_enum!(TranscriptionStatus, {
    Queued => "queued",
    Preparing => "preparing",
    Transcribing => "transcribing",
    Completed => "completed",
    Cancelled => "cancelled",
    Failed => "failed",
    Interrupted => "interrupted",
});

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Transcription {
    pub id: String,
    pub kind: TranscriptionKind,
    pub source_filename: String,
    pub source_path: String,
    pub source_size_bytes: u64,
    pub source_modified_ms: i64,
    pub created_ms: i64,
    pub started_ms: Option<i64>,
    pub completed_ms: Option<i64>,
    pub status: TranscriptionStatus,
    pub duration_seconds: Option<f64>,
    pub model_version: String,
    pub error: Option<String>,
    pub audio_path: Option<String>,
    pub completed_chunks: u64,
    pub total_chunks: Option<u64>,
    pub queue_visible: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceTrack {
    File,
    Microphone,
    System,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentState {
    Provisional,
    Final,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub transcription_id: String,
    pub index: usize,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub text: String,
    pub source_track: SourceTrack,
    pub speaker_id: Option<String>,
    pub state: SegmentState,
}

impl AudioBlockEvent {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != CONTRACT_VERSION
            || self.session_id.is_empty()
            || self.idempotency_key.is_empty()
        {
            return Err("invalid audio event identity/version");
        }
        if self.sample_rate == 0 || self.duration_samples == 0 {
            return Err("invalid audio event geometry");
        }
        Ok(())
    }
}

impl TranscriptSegmentEvent {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != CONTRACT_VERSION
            || self.session_id.is_empty()
            || self.segment_id.is_empty()
        {
            return Err("invalid transcript event identity/version");
        }
        if !self.start_seconds.is_finite()
            || !self.end_seconds.is_finite()
            || self.end_seconds <= self.start_seconds
        {
            return Err("invalid transcript event timestamps");
        }
        Ok(())
    }
}

impl TranscriptSegment {
    pub fn new(
        transcription_id: &str,
        index: usize,
        start_seconds: f64,
        end_seconds: f64,
        text: String,
        source_track: SourceTrack,
        state: SegmentState,
    ) -> Result<Self, &'static str> {
        if transcription_id.is_empty() {
            return Err("transcription ID cannot be empty");
        }
        if !start_seconds.is_finite()
            || !end_seconds.is_finite()
            || start_seconds < 0.0
            || end_seconds <= start_seconds
        {
            return Err("segment timestamps must be finite, non-negative, and ordered");
        }
        Ok(Self {
            id: format!("{transcription_id}:{index}"),
            transcription_id: transcription_id.to_owned(),
            index,
            start_seconds,
            end_seconds,
            text,
            source_track,
            speaker_id: None,
            state,
        })
    }
}

/// Remove an acoustic copy of system playback from the microphone transcript.
///
/// Dual-source recording deliberately keeps both tracks independent. When the
/// microphone can hear the speakers, ASR may produce the same phrase on both
/// tracks and incorrectly attribute the copy to the local speaker. The system
/// track is authoritative in that case. This filter is conservative: text
/// must be nearly identical and occur at the same time. Different simultaneous
/// speech (the useful double-talk case) is preserved.
pub fn suppress_microphone_echo(segments: Vec<TranscriptSegment>) -> Vec<TranscriptSegment> {
    const TIMING_TOLERANCE_SECONDS: f64 = 0.75;

    let system: Vec<&TranscriptSegment> = segments
        .iter()
        .filter(|segment| segment.source_track == SourceTrack::System)
        .collect();
    if system.is_empty() {
        return segments;
    }

    segments
        .iter()
        .filter(|segment| {
            if segment.source_track != SourceTrack::Microphone {
                return true;
            }
            let candidates: Vec<&TranscriptSegment> = system
                .iter()
                .copied()
                .filter(|candidate| {
                    segment.start_seconds <= candidate.end_seconds + TIMING_TOLERANCE_SECONDS
                        && candidate.start_seconds <= segment.end_seconds + TIMING_TOLERANCE_SECONDS
                })
                .collect();
            if candidates
                .iter()
                .any(|candidate| echo_text_matches(&segment.text, &candidate.text))
            {
                return false;
            }
            let combined = candidates
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            !echo_text_matches(&segment.text, &combined)
        })
        .cloned()
        .collect()
}

fn echo_text_matches(microphone: &str, system: &str) -> bool {
    let microphone = normalized_words(microphone);
    let system = normalized_words(system);
    if microphone.is_empty() || system.is_empty() {
        return false;
    }
    if microphone == system {
        return true;
    }
    // Short generic replies are too easy to say independently. Only exact
    // short transcripts (handled above) are safe to suppress.
    if microphone.len() < 3 || system.len() < 3 {
        return false;
    }

    let mut counts = HashMap::<&str, usize>::new();
    for word in &system {
        *counts.entry(word).or_default() += 1;
    }
    let mut overlap = 0usize;
    for word in &microphone {
        if let Some(count) = counts.get_mut(word.as_str()) {
            if *count > 0 {
                overlap += 1;
                *count -= 1;
            }
        }
    }
    let containment = overlap as f64 / microphone.len().min(system.len()) as f64;
    let dice = 2.0 * overlap as f64 / (microphone.len() + system.len()) as f64;
    containment >= 0.8 && dice >= 0.72
}

fn normalized_words(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_audio_and_segment_events_round_trip_and_validate() {
        let audio = AudioBlockEvent {
            version: CONTRACT_VERSION,
            session_id: "session".into(),
            track_id: TrackId::Microphone,
            seq: 7,
            sample_rate: 48_000,
            absolute_sample_start: 3_360,
            duration_samples: 480,
            codec: "pcm_f32le".into(),
            codec_config: None,
            final_block: false,
            discontinuity: false,
            idempotency_key: "session:microphone:7".into(),
        };
        let decoded: AudioBlockEvent =
            serde_json::from_str(&serde_json::to_string(&audio).unwrap()).unwrap();
        assert_eq!(decoded, audio);
        assert!(decoded.validate().is_ok());
        assert!(
            AudioBlockEvent {
                version: 99,
                ..audio
            }
            .validate()
            .is_err()
        );

        let segment = TranscriptSegmentEvent {
            version: CONTRACT_VERSION,
            session_id: "session".into(),
            segment_id: "segment".into(),
            track_id: TrackId::System,
            seq: 2,
            start_seconds: 1.0,
            end_seconds: 2.0,
            text: "реплика".into(),
            final_segment: true,
            revision: 3,
            idempotency_key: "session:segment:3".into(),
        };
        assert!(segment.validate().is_ok());
        assert!(
            TranscriptSegmentEvent {
                end_seconds: 0.0,
                ..segment
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn segment_serialization_is_stable() {
        let segment = TranscriptSegment::new(
            "tx-1",
            2,
            1.25,
            2.5,
            "текст".into(),
            SourceTrack::Microphone,
            SegmentState::Provisional,
        )
        .unwrap();
        let json = serde_json::to_value(segment).unwrap();
        assert_eq!(json["id"], "tx-1:2");
        assert_eq!(json["source_track"], "microphone");
        assert_eq!(json["state"], "provisional");
    }

    #[test]
    fn segment_rejects_invalid_timestamps() {
        assert!(
            TranscriptSegment::new(
                "tx-1",
                0,
                2.0,
                1.0,
                String::new(),
                SourceTrack::File,
                SegmentState::Final,
            )
            .is_err()
        );
    }

    fn segment(
        index: usize,
        start: f64,
        end: f64,
        text: &str,
        track: SourceTrack,
    ) -> TranscriptSegment {
        TranscriptSegment::new(
            "live",
            index,
            start,
            end,
            text.into(),
            track,
            SegmentState::Final,
        )
        .unwrap()
    }

    #[test]
    fn microphone_copy_of_system_phrase_is_suppressed() {
        let segments = vec![
            segment(
                0,
                1.0,
                3.0,
                "Добрый день, начинаем встречу.",
                SourceTrack::System,
            ),
            segment(
                1,
                1.1,
                3.1,
                "добрый день начинаем встречу",
                SourceTrack::Microphone,
            ),
        ];

        let filtered = suppress_microphone_echo(segments);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].source_track, SourceTrack::System);
    }

    #[test]
    fn simultaneous_different_speech_is_preserved() {
        let segments = vec![
            segment(
                0,
                1.0,
                3.0,
                "Давайте откроем следующий документ",
                SourceTrack::System,
            ),
            segment(
                1,
                1.1,
                2.8,
                "Я сейчас пришлю ссылку в чат",
                SourceTrack::Microphone,
            ),
        ];

        assert_eq!(suppress_microphone_echo(segments).len(), 2);
    }

    #[test]
    fn similar_phrase_outside_echo_window_is_preserved() {
        let segments = vec![
            segment(0, 1.0, 2.0, "Да это правильный ответ", SourceTrack::System),
            segment(
                1,
                5.0,
                6.0,
                "да это правильный ответ",
                SourceTrack::Microphone,
            ),
        ];

        assert_eq!(suppress_microphone_echo(segments).len(), 2);
    }

    #[test]
    fn statuses_round_trip_as_storage_values() {
        for status in [
            TranscriptionStatus::Queued,
            TranscriptionStatus::Preparing,
            TranscriptionStatus::Transcribing,
            TranscriptionStatus::Completed,
            TranscriptionStatus::Cancelled,
            TranscriptionStatus::Failed,
            TranscriptionStatus::Interrupted,
        ] {
            assert_eq!(status.to_string().parse(), Ok(status));
        }
    }
}
