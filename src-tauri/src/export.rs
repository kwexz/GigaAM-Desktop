use crate::domain::TranscriptSegment;

#[derive(Clone, Copy)]
pub enum ExportFormat {
    Txt,
    Srt,
    Vtt,
    Json,
}

pub fn export(
    segments: &[TranscriptSegment],
    format: ExportFormat,
) -> Result<String, serde_json::Error> {
    match format {
        ExportFormat::Txt => Ok(segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")),
        ExportFormat::Srt => Ok(segments
            .iter()
            .map(|segment| {
                format!(
                    "{}\n{} --> {}\n{}\n",
                    segment.index + 1,
                    timestamp(segment.start_seconds, ','),
                    timestamp(segment.end_seconds, ','),
                    segment.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n")),
        ExportFormat::Vtt => Ok(format!(
            "WEBVTT\n\n{}",
            segments
                .iter()
                .map(|segment| format!(
                    "{} --> {}\n{}\n",
                    timestamp(segment.start_seconds, '.'),
                    timestamp(segment.end_seconds, '.'),
                    segment.text
                ))
                .collect::<Vec<_>>()
                .join("\n")
        )),
        ExportFormat::Json => serde_json::to_string_pretty(segments),
    }
}

fn timestamp(seconds: f64, separator: char) -> String {
    let total_ms = (seconds * 1000.0).round() as u64;
    let hours = total_ms / 3_600_000;
    let minutes = total_ms / 60_000 % 60;
    let seconds = total_ms / 1000 % 60;
    let milliseconds = total_ms % 1000;
    format!("{hours:02}:{minutes:02}:{seconds:02}{separator}{milliseconds:03}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{SegmentState, SourceTrack, TranscriptSegment};

    fn segments() -> Vec<TranscriptSegment> {
        vec![
            TranscriptSegment::new(
                "tx",
                0,
                65.125,
                67.5,
                "Привет".into(),
                SourceTrack::File,
                SegmentState::Final,
            )
            .unwrap(),
        ]
    }

    #[test]
    fn formats_timestamps() {
        assert_eq!(timestamp(3661.007, ','), "01:01:01,007");
        assert_eq!(timestamp(1.9996, '.'), "00:00:02.000");
    }

    #[test]
    fn exports_all_formats_from_same_segments() {
        let segments = segments();
        assert_eq!(export(&segments, ExportFormat::Txt).unwrap(), "Привет");
        assert!(
            export(&segments, ExportFormat::Srt)
                .unwrap()
                .contains("00:01:05,125 --> 00:01:07,500")
        );
        assert!(
            export(&segments, ExportFormat::Vtt)
                .unwrap()
                .starts_with("WEBVTT\n\n")
        );
        assert!(
            export(&segments, ExportFormat::Json)
                .unwrap()
                .contains("\"source_track\": \"file\"")
        );
    }
}
