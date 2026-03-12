//! Audio detection in JSON request bodies.
//!
//! Supports Anthropic and OpenAI audio block formats.
//! Shared between desktop (full pipeline) and mobile (detection-only).

use serde_json::Value;

/// Detected audio block with metadata.
#[derive(Debug, Clone)]
pub struct AudioBlock {
    /// Base64-encoded audio data.
    pub data: String,
    /// MIME type (e.g., "audio/wav", "audio/mp3").
    pub media_type: String,
    /// JSON path to the audio block (e.g., "messages[0].content[1]").
    pub json_path: String,
}

/// Audio format identified by magic bytes or MIME type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    Wav,
    Mp3,
    Ogg,
    Webm,
    Unknown,
}

impl AudioFormat {
    /// Identify format from decoded bytes (magic-byte sniffing).
    pub fn from_bytes(data: &[u8]) -> Self {
        if data.len() < 12 {
            return AudioFormat::Unknown;
        }
        if &data[..4] == b"RIFF" && &data[8..12] == b"WAVE" {
            AudioFormat::Wav
        } else if data[..3] == [0xFF, 0xFB, 0x90]
            || data[..3] == [0xFF, 0xFB, 0x80]
            || data[..2] == [0xFF, 0xFB]
            || data[..2] == [0xFF, 0xFA]
            || &data[..3] == b"ID3"
        {
            AudioFormat::Mp3
        } else if &data[..4] == b"OggS" {
            AudioFormat::Ogg
        } else if data[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
            // EBML header — WebM/Matroska
            AudioFormat::Webm
        } else {
            AudioFormat::Unknown
        }
    }

    /// Identify format from MIME type string.
    pub fn from_mime(mime: &str) -> Self {
        let lower = mime.to_lowercase();
        if lower.contains("wav") || lower.contains("wave") {
            AudioFormat::Wav
        } else if lower.contains("mp3") || lower.contains("mpeg") {
            AudioFormat::Mp3
        } else if lower.contains("ogg") || lower.contains("vorbis") {
            AudioFormat::Ogg
        } else if lower.contains("webm") || lower.contains("opus") {
            AudioFormat::Webm
        } else {
            AudioFormat::Unknown
        }
    }
}

/// Detect audio blocks in a JSON value.
///
/// Walks the JSON tree looking for Anthropic or OpenAI audio block patterns.
pub fn detect_audio_blocks(json: &Value) -> Vec<AudioBlock> {
    let mut blocks = Vec::new();
    walk_json(json, "", &mut blocks);
    blocks
}

fn walk_json(value: &Value, path: &str, blocks: &mut Vec<AudioBlock>) {
    match value {
        Value::Object(map) => {
            // Anthropic format:
            // {"type": "audio", "source": {"type": "base64", "media_type": "audio/wav", "data": "..."}}
            if map.get("type").and_then(|v| v.as_str()) == Some("audio") {
                if let Some(source) = map.get("source").and_then(|v| v.as_object()) {
                    if source.get("type").and_then(|v| v.as_str()) == Some("base64") {
                        if let (Some(media_type), Some(data)) = (
                            source.get("media_type").and_then(|v| v.as_str()),
                            source.get("data").and_then(|v| v.as_str()),
                        ) {
                            if is_audio_mime(media_type) {
                                blocks.push(AudioBlock {
                                    data: data.to_string(),
                                    media_type: media_type.to_string(),
                                    json_path: path.to_string(),
                                });
                                return;
                            }
                        }
                    }
                }
            }

            // OpenAI format:
            // {"type": "input_audio", "input_audio": {"data": "...", "format": "wav"}}
            if map.get("type").and_then(|v| v.as_str()) == Some("input_audio") {
                if let Some(audio) = map.get("input_audio").and_then(|v| v.as_object()) {
                    if let Some(data) = audio.get("data").and_then(|v| v.as_str()) {
                        let format = audio
                            .get("format")
                            .and_then(|v| v.as_str())
                            .unwrap_or("wav");
                        let media_type = format!("audio/{}", format);
                        blocks.push(AudioBlock {
                            data: data.to_string(),
                            media_type,
                            json_path: path.to_string(),
                        });
                        return;
                    }
                }
            }

            // Recurse into object fields
            for (key, val) in map {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", path, key)
                };
                walk_json(val, &child_path, blocks);
            }
        }
        Value::Array(arr) => {
            for (i, val) in arr.iter().enumerate() {
                let child_path = format!("{}[{}]", path, i);
                walk_json(val, &child_path, blocks);
            }
        }
        _ => {}
    }
}

/// Check if a MIME type is an audio type.
fn is_audio_mime(mime: &str) -> bool {
    let lower = mime.to_lowercase();
    lower.starts_with("audio/")
}

/// Check if raw bytes contain audio content (magic-byte sniffing).
pub fn is_audio_bytes(data: &[u8]) -> bool {
    AudioFormat::from_bytes(data) != AudioFormat::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_anthropic_audio() {
        let json: Value = serde_json::from_str(r#"{
            "messages": [{"content": [
                {"type": "text", "text": "transcribe this"},
                {"type": "audio", "source": {"type": "base64", "media_type": "audio/wav", "data": "UklGRg=="}}
            ]}]
        }"#).unwrap();
        let blocks = detect_audio_blocks(&json);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].media_type, "audio/wav");
        assert_eq!(blocks[0].data, "UklGRg==");
    }

    #[test]
    fn test_detect_openai_audio() {
        let json: Value = serde_json::from_str(
            r#"{
            "messages": [{"content": [
                {"type": "input_audio", "input_audio": {"data": "UklGRg==", "format": "mp3"}}
            ]}]
        }"#,
        )
        .unwrap();
        let blocks = detect_audio_blocks(&json);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].media_type, "audio/mp3");
    }

    #[test]
    fn test_no_audio_in_text_only() {
        let json: Value = serde_json::from_str(
            r#"{
            "messages": [{"content": [
                {"type": "text", "text": "hello world"}
            ]}]
        }"#,
        )
        .unwrap();
        let blocks = detect_audio_blocks(&json);
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_multiple_audio_blocks() {
        let json: Value = serde_json::from_str(r#"{
            "messages": [{"content": [
                {"type": "audio", "source": {"type": "base64", "media_type": "audio/wav", "data": "AAAA"}},
                {"type": "text", "text": "and also"},
                {"type": "audio", "source": {"type": "base64", "media_type": "audio/mp3", "data": "BBBB"}}
            ]}]
        }"#).unwrap();
        let blocks = detect_audio_blocks(&json);
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn test_audio_format_from_bytes_wav() {
        let wav_header = b"RIFF\x00\x00\x00\x00WAVEfmt ";
        assert_eq!(AudioFormat::from_bytes(wav_header), AudioFormat::Wav);
    }

    #[test]
    fn test_audio_format_from_bytes_mp3_id3() {
        let mp3_id3 = b"ID3\x04\x00\x00\x00\x00\x00\x00\x00\x00";
        assert_eq!(AudioFormat::from_bytes(mp3_id3), AudioFormat::Mp3);
    }

    #[test]
    fn test_audio_format_from_bytes_ogg() {
        let ogg_header = b"OggS\x00\x02\x00\x00\x00\x00\x00\x00";
        assert_eq!(AudioFormat::from_bytes(ogg_header), AudioFormat::Ogg);
    }

    #[test]
    fn test_audio_format_from_bytes_webm() {
        let ebml_header = [
            0x1A, 0x45, 0xDF, 0xA3, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F,
        ];
        assert_eq!(AudioFormat::from_bytes(&ebml_header), AudioFormat::Webm);
    }

    #[test]
    fn test_audio_format_from_bytes_unknown() {
        assert_eq!(
            AudioFormat::from_bytes(b"not audio data!"),
            AudioFormat::Unknown
        );
    }

    #[test]
    fn test_audio_format_from_bytes_too_short() {
        assert_eq!(AudioFormat::from_bytes(b"hi"), AudioFormat::Unknown);
    }

    #[test]
    fn test_audio_format_from_mime() {
        assert_eq!(AudioFormat::from_mime("audio/wav"), AudioFormat::Wav);
        assert_eq!(AudioFormat::from_mime("audio/wave"), AudioFormat::Wav);
        assert_eq!(AudioFormat::from_mime("audio/mp3"), AudioFormat::Mp3);
        assert_eq!(AudioFormat::from_mime("audio/mpeg"), AudioFormat::Mp3);
        assert_eq!(AudioFormat::from_mime("audio/ogg"), AudioFormat::Ogg);
        assert_eq!(AudioFormat::from_mime("audio/vorbis"), AudioFormat::Ogg);
        assert_eq!(AudioFormat::from_mime("audio/webm"), AudioFormat::Webm);
        assert_eq!(AudioFormat::from_mime("audio/opus"), AudioFormat::Webm);
        assert_eq!(AudioFormat::from_mime("text/plain"), AudioFormat::Unknown);
    }

    #[test]
    fn test_is_audio_bytes() {
        assert!(is_audio_bytes(b"RIFF\x00\x00\x00\x00WAVEfmt "));
        assert!(!is_audio_bytes(b"not audio"));
    }

    #[test]
    fn test_reject_image_as_audio() {
        // PNG header should not be detected as audio
        let png = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
        ];
        assert_eq!(AudioFormat::from_bytes(&png), AudioFormat::Unknown);
    }
}
