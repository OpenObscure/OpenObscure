//! Voice PII detection pipeline — KWS-gated selective strip.
//!
//! Detects audio blocks in JSON request bodies, decodes to PCM,
//! runs keyword spotting for PII trigger phrases, and strips only
//! audio blocks where PII keywords are detected.
//!
//! When KWS engine is not available, audio passes through unchanged.

use serde_json::Value;
use uuid::Uuid;

use crate::kws_engine::KwsEngine;
use crate::voice_detect::{detect_audio_blocks, AudioBlock, AudioFormat};

/// Result of scanning audio blocks for PII.
#[derive(Debug)]
pub struct VoiceScanResult {
    /// Number of audio blocks scanned.
    pub blocks_scanned: usize,
    /// Number of audio blocks stripped (PII detected).
    pub blocks_stripped: usize,
    /// PII keywords that triggered stripping.
    pub keywords_found: Vec<String>,
    /// Total voice pipeline time (milliseconds).
    pub total_ms: u64,
    /// Audio decode time (milliseconds).
    pub decode_ms: u64,
    /// KWS inference time (milliseconds).
    pub kws_ms: u64,
}

/// Scan audio blocks for PII keywords and strip blocks where PII is detected.
///
/// For each audio block found:
/// 1. Decode base64 audio to PCM f32 at 16kHz
/// 2. Run keyword spotter for PII trigger phrases
/// 3. If PII found: replace audio with text notice including detected keywords
/// 4. If no PII: leave audio block unchanged (pass through)
pub fn scan_and_strip_audio_blocks(
    json: &mut Value,
    kws_engine: &KwsEngine,
    inspect: bool,
    request_id: &Uuid,
) -> VoiceScanResult {
    let blocks = detect_audio_blocks(json);
    if blocks.is_empty() {
        return VoiceScanResult {
            blocks_scanned: 0,
            blocks_stripped: 0,
            keywords_found: Vec::new(),
            total_ms: 0,
            decode_ms: 0,
            kws_ms: 0,
        };
    }

    oo_info!(
        crate::oo_log::modules::VOICE,
        "Audio blocks detected — scanning for PII keywords",
        count = blocks.len()
    );

    let pipeline_start = std::time::Instant::now();
    let mut scanned = 0;
    let mut stripped = 0;
    let mut all_keywords = Vec::new();
    let mut total_decode_ms: u64 = 0;
    let mut total_kws_ms: u64 = 0;

    for block in &blocks {
        scanned += 1;

        match scan_single_block(block, kws_engine) {
            Ok((Some(keywords), decode_ms, kws_ms)) => {
                total_decode_ms += decode_ms;
                total_kws_ms += kws_ms;
                // PII detected — strip this audio block
                let notice = format!(
                    "[AUDIO_PII_DETECTED: keywords={{{}}} — audio stripped]",
                    keywords.join(", ")
                );
                if inspect {
                    crate::inspect::save_audio(
                        request_id,
                        scanned - 1,
                        &block.data,
                        Some(&notice),
                        &block.media_type,
                    );
                }
                if replace_audio_with_notice(json, &block.json_path, &notice) {
                    stripped += 1;
                    all_keywords.extend(keywords);
                }
            }
            Ok((None, decode_ms, kws_ms)) => {
                total_decode_ms += decode_ms;
                total_kws_ms += kws_ms;
                if inspect {
                    crate::inspect::save_audio(
                        request_id,
                        scanned - 1,
                        &block.data,
                        None,
                        &block.media_type,
                    );
                }
                // No PII — leave audio block unchanged
                oo_debug!(
                    crate::oo_log::modules::VOICE,
                    "No PII keywords in audio block — passing through",
                    path = &block.json_path
                );
            }
            Err(e) => {
                // Decode/inference error — log and pass through (fail-open)
                oo_warn!(
                    crate::oo_log::modules::VOICE,
                    "Audio PII scan failed, passing through",
                    error = %e,
                    path = &block.json_path
                );
            }
        }
    }

    if stripped > 0 {
        oo_info!(
            crate::oo_log::modules::VOICE,
            "Audio PII detected and stripped",
            stripped = stripped,
            keywords = %all_keywords.join(", ")
        );
    }

    VoiceScanResult {
        blocks_scanned: scanned,
        blocks_stripped: stripped,
        keywords_found: all_keywords,
        total_ms: pipeline_start.elapsed().as_millis() as u64,
        decode_ms: total_decode_ms,
        kws_ms: total_kws_ms,
    }
}

/// Scan a single audio block for PII keywords.
/// Returns Ok((Some(keywords), decode_ms, kws_ms)) if PII detected,
/// Ok((None, decode_ms, kws_ms)) if clean.
fn scan_single_block(
    block: &AudioBlock,
    kws_engine: &KwsEngine,
) -> Result<(Option<Vec<String>>, u64, u64), String> {
    // Determine audio format from MIME type
    let format = AudioFormat::from_mime(&block.media_type);

    // Decode base64 audio to PCM f32 mono at original sample rate
    let decode_start = std::time::Instant::now();
    let (pcm, sample_rate) = crate::audio_decode::decode_audio_to_pcm(&block.data, format)
        .map_err(|e| format!("audio decode: {}", e))?;
    let decode_ms = decode_start.elapsed().as_millis() as u64;

    if pcm.is_empty() {
        return Ok((None, decode_ms, 0));
    }

    // Run keyword spotter (sherpa-onnx resamples internally if needed)
    let result = kws_engine
        .detect_pii(pcm, sample_rate)
        .map_err(|e| format!("KWS: {}", e))?;

    let kws_ms = result.inference_ms;
    if result.pii_detected {
        Ok((Some(result.keywords_found), decode_ms, kws_ms))
    } else {
        Ok((None, decode_ms, kws_ms))
    }
}

/// Detect audio blocks without scanning (for metrics/detection-only mode).
pub fn detect_audio(json: &Value) -> Vec<AudioBlock> {
    detect_audio_blocks(json)
}

/// Replace an audio block at the given JSON path with a text notice.
fn replace_audio_with_notice(json: &mut Value, json_path: &str, notice: &str) -> bool {
    let parts: Vec<&str> = json_path.split('.').collect();
    let mut current = json;

    for part in &parts {
        if let Some(bracket_pos) = part.find('[') {
            let key = &part[..bracket_pos];
            let idx_str = &part[bracket_pos + 1..part.len() - 1];
            if let Ok(idx) = idx_str.parse::<usize>() {
                current = &mut current[key][idx];
            }
        } else {
            current = &mut current[part];
        }
    }

    // Replace Anthropic format: {"type": "audio", "source": {...}}
    if current.get("type").and_then(|v| v.as_str()) == Some("audio") {
        *current = serde_json::json!({
            "type": "text",
            "text": notice
        });
        return true;
    }

    // Replace OpenAI format: {"type": "input_audio", "input_audio": {...}}
    if current.get("type").and_then(|v| v.as_str()) == Some("input_audio") {
        *current = serde_json::json!({
            "type": "text",
            "text": notice
        });
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_no_audio() {
        // Without KWS engine, we can't scan — but no audio means no-op anyway
        let json: Value = serde_json::from_str(
            r#"{
            "messages": [{"content": [
                {"type": "text", "text": "hello"}
            ]}]
        }"#,
        )
        .unwrap();

        // Can't construct a real KwsEngine without models, so test detection
        let blocks = detect_audio(&json);
        assert!(blocks.is_empty());

        // Verify text unchanged
        assert_eq!(
            json["messages"][0]["content"][0]["text"].as_str().unwrap(),
            "hello"
        );
    }

    #[test]
    fn test_detect_audio_blocks() {
        let json: Value = serde_json::from_str(
            r#"{
            "messages": [{"content": [
                {"type": "audio", "source": {"type": "base64", "media_type": "audio/wav", "data": "UklGRg=="}}
            ]}]
        }"#,
        )
        .unwrap();
        let blocks = detect_audio(&json);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].media_type, "audio/wav");
    }

    #[test]
    fn test_replace_anthropic_audio() {
        let mut json: Value = serde_json::from_str(
            r#"{
            "messages": [{"content": [
                {"type": "audio", "source": {"type": "base64", "media_type": "audio/wav", "data": "UklGRg=="}}
            ]}]
        }"#,
        )
        .unwrap();
        let replaced = replace_audio_with_notice(
            &mut json,
            "messages[0].content[0]",
            "[AUDIO_PII_DETECTED: keywords={social security} — audio stripped]",
        );
        assert!(replaced);
        let block = &json["messages"][0]["content"][0];
        assert_eq!(block["type"].as_str().unwrap(), "text");
        assert!(block["text"]
            .as_str()
            .unwrap()
            .contains("AUDIO_PII_DETECTED"));
        assert!(block["text"].as_str().unwrap().contains("social security"));
    }

    #[test]
    fn test_replace_openai_audio() {
        let mut json: Value = serde_json::from_str(
            r#"{
            "messages": [{"content": [
                {"type": "input_audio", "input_audio": {"data": "UklGRg==", "format": "wav"}}
            ]}]
        }"#,
        )
        .unwrap();
        let replaced = replace_audio_with_notice(
            &mut json,
            "messages[0].content[0]",
            "[AUDIO_PII_DETECTED: keywords={credit card} — audio stripped]",
        );
        assert!(replaced);
        assert_eq!(
            json["messages"][0]["content"][0]["type"].as_str().unwrap(),
            "text"
        );
    }

    #[test]
    fn test_replace_mixed_content_preserves_text() {
        let mut json: Value = serde_json::from_str(
            r#"{
            "messages": [{"content": [
                {"type": "text", "text": "transcribe this"},
                {"type": "audio", "source": {"type": "base64", "media_type": "audio/wav", "data": "UklGRg=="}},
                {"type": "text", "text": "and summarize"}
            ]}]
        }"#,
        )
        .unwrap();
        let replaced = replace_audio_with_notice(
            &mut json,
            "messages[0].content[1]",
            "[AUDIO_PII_DETECTED: keywords={phone number} — audio stripped]",
        );
        assert!(replaced);
        // Text blocks preserved
        assert_eq!(
            json["messages"][0]["content"][0]["text"].as_str().unwrap(),
            "transcribe this"
        );
        assert_eq!(
            json["messages"][0]["content"][2]["text"].as_str().unwrap(),
            "and summarize"
        );
        // Audio replaced
        assert_eq!(
            json["messages"][0]["content"][1]["type"].as_str().unwrap(),
            "text"
        );
    }

    #[test]
    fn test_replace_non_audio_noop() {
        let mut json: Value = serde_json::from_str(
            r#"{
            "messages": [{"content": [
                {"type": "text", "text": "hello"}
            ]}]
        }"#,
        )
        .unwrap();
        let replaced =
            replace_audio_with_notice(&mut json, "messages[0].content[0]", "[AUDIO_PII_DETECTED]");
        assert!(!replaced);
        // Text unchanged
        assert_eq!(
            json["messages"][0]["content"][0]["text"].as_str().unwrap(),
            "hello"
        );
    }

    #[test]
    fn test_voice_scan_result_defaults() {
        let result = VoiceScanResult {
            blocks_scanned: 0,
            blocks_stripped: 0,
            keywords_found: Vec::new(),
            total_ms: 0,
            decode_ms: 0,
            kws_ms: 0,
        };
        assert_eq!(result.blocks_scanned, 0);
        assert_eq!(result.blocks_stripped, 0);
        assert!(result.keywords_found.is_empty());
    }
}
