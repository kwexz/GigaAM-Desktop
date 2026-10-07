use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

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

#[cfg(test)]
mod tests {
    use super::*;

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
