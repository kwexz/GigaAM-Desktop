use crate::domain::{SourceTrack, TranscriptSegment};

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
    let multi_source = segments
        .iter()
        .any(|segment| segment.source_track == SourceTrack::Microphone)
        && segments
            .iter()
            .any(|segment| segment.source_track == SourceTrack::System);
    match format {
        ExportFormat::Txt => Ok(segments
            .iter()
            .map(|segment| {
                format!(
                    "[{}] {}",
                    timestamp(segment.start_seconds, '.'),
                    labeled_text(segment, multi_source)
                )
            })
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
                    labeled_text(segment, multi_source)
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
                    labeled_text(segment, multi_source)
                ))
                .collect::<Vec<_>>()
                .join("\n")
        )),
        ExportFormat::Json => serde_json::to_string_pretty(segments),
    }
}

fn labeled_text(segment: &TranscriptSegment, multi_source: bool) -> String {
    if !multi_source {
        return segment.text.clone();
    }
    let author = match segment.source_track {
        SourceTrack::Microphone => "Микрофон",
        SourceTrack::System => "Система",
        SourceTrack::File => return segment.text.clone(),
    };
    format!("{author}: {}", segment.text)
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
        assert_eq!(
            export(&segments, ExportFormat::Txt).unwrap(),
            "[00:01:05.125] Привет"
        );
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

    #[test]
    fn dual_source_export_includes_author() {
        let mut segments = segments();
        segments[0].source_track = SourceTrack::Microphone;
        segments.push(
            TranscriptSegment::new(
                "tx",
                1,
                68.0,
                69.0,
                "Ответ".into(),
                SourceTrack::System,
                SegmentState::Final,
            )
            .unwrap(),
        );
        let text = export(&segments, ExportFormat::Txt).unwrap();
        assert_eq!(
            text,
            "[00:01:05.125] Микрофон: Привет\n[00:01:08.000] Система: Ответ"
        );
        assert!(
            export(&segments, ExportFormat::Srt)
                .unwrap()
                .contains("Микрофон: Привет")
        );
    }
}
