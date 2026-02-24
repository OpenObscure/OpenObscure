use std::path::Path;

use ndarray::Array2;
use ort::session::Session;

use crate::pii_types::PiiType;
use crate::scanner::PiiMatch;
use crate::wordpiece::WordPieceTokenizer;

/// BIO tag classification for a label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BioTag {
    O,
    B,
    I,
}

/// Mapping entry for a single label ID → (BIO tag, optional PII type).
#[derive(Debug, Clone)]
struct LabelInfo {
    tag: BioTag,
    pii_type: Option<PiiType>,
}

/// NER scanner using TinyBERT ONNX model via ONNX Runtime.
///
/// Loaded once at startup and shared via `Arc`. Runs inference on text
/// to detect semantic PII: person names, locations, organizations,
/// health references, and child references.
///
/// Label mapping is loaded dynamically from label_map.json so the scanner
/// works with any BIO-tagged NER model (dslim/bert-base-NER, custom fine-tuned, etc.).
pub struct NerScanner {
    session: Session,
    tokenizer: WordPieceTokenizer,
    confidence_threshold: f32,
    num_labels: usize,
    label_map: Vec<LabelInfo>,
}

/// A detected NER entity span before conversion to PiiMatch.
#[derive(Debug)]
struct NerEntity {
    pii_type: PiiType,
    byte_start: usize,
    byte_end: usize,
    raw_value: String,
    confidence: f32,
}

impl NerScanner {
    /// Load NER model and tokenizer from a directory containing:
    /// - model_int8.onnx (or model.onnx)
    /// - vocab.txt
    /// - label_map.json (optional — falls back to default 11-label schema)
    pub fn load(model_dir: &Path, confidence_threshold: f32) -> Result<Self, NerError> {
        let model_path = model_dir.join("model_int8.onnx");
        let model_path = if model_path.exists() {
            model_path
        } else {
            let fallback = model_dir.join("model.onnx");
            if fallback.exists() {
                fallback
            } else {
                return Err(NerError::ModelNotFound(model_dir.display().to_string()));
            }
        };

        let vocab_path = model_dir.join("vocab.txt");
        let tokenizer = WordPieceTokenizer::from_file(&vocab_path)
            .map_err(|e| NerError::Tokenizer(e.to_string()))?;

        // Load label map — determines num_labels and BIO→PiiType mapping
        let label_map_path = model_dir.join("label_map.json");
        let label_map = build_label_map(&label_map_path)?;
        let num_labels = label_map.len();

        let session = crate::ort_ep::build_session(&model_path)
            .map_err(|e| NerError::OnnxRuntime(e.to_string()))?;

        oo_info!(crate::oo_log::modules::NER, "NER scanner loaded",
            model = %model_path.display(),
            vocab_size = tokenizer.vocab_size(),
            num_labels = num_labels);

        Ok(Self {
            session,
            tokenizer,
            confidence_threshold,
            num_labels,
            label_map,
        })
    }

    /// Check if a label ID is a B-* (begin) tag.
    fn is_b_tag(&self, label_id: usize) -> bool {
        self.label_map
            .get(label_id)
            .is_some_and(|info| info.tag == BioTag::B)
    }

    /// Check if a label ID is an I-* (inside) tag.
    fn is_i_tag(&self, label_id: usize) -> bool {
        self.label_map
            .get(label_id)
            .is_some_and(|info| info.tag == BioTag::I)
    }

    /// Map a label ID to its PII type (if any).
    fn bio_to_pii_type(&self, label_id: usize) -> Option<PiiType> {
        self.label_map.get(label_id).and_then(|info| info.pii_type)
    }

    /// Maximum byte length for a single NER inference pass. Text beyond this is
    /// chunked with overlap to ensure full coverage. Set conservatively to stay
    /// within the 512-token context window even for token-dense text (proper nouns,
    /// multilingual names) where WordPiece expansion averages 3-4 subtokens/word.
    const MAX_CHUNK_BYTES: usize = 800;

    /// Overlap in bytes between adjacent chunks. Ensures entities at chunk
    /// boundaries are captured by at least one chunk even when the tokenizer
    /// truncates before the full chunk is processed.
    const CHUNK_OVERLAP_BYTES: usize = 150;

    /// Run NER inference on a text string.
    /// Returns PiiMatch results for detected entities above the confidence threshold.
    ///
    /// For long texts that exceed the 512-token context window, automatically splits
    /// into overlapping chunks at whitespace boundaries, runs NER on each chunk, and
    /// merges results with deduplication.
    pub fn scan_text(&mut self, text: &str) -> Result<Vec<PiiMatch>, NerError> {
        if text.is_empty() {
            return Ok(Vec::new());
        }

        if text.len() <= Self::MAX_CHUNK_BYTES {
            return self.scan_text_single_pass(text);
        }

        // Chunked path: split text at whitespace boundaries with overlap
        let chunks = split_into_chunks(text, Self::MAX_CHUNK_BYTES, Self::CHUNK_OVERLAP_BYTES);
        let mut all_matches: Vec<PiiMatch> = Vec::new();

        for (chunk_byte_offset, chunk_text) in &chunks {
            let chunk_matches = self.scan_text_single_pass(chunk_text)?;

            // Offset-adjust matches back to original text coordinates
            for mut m in chunk_matches {
                m.start += chunk_byte_offset;
                m.end += chunk_byte_offset;
                all_matches.push(m);
            }
        }

        // Deduplicate overlapping matches from chunk overlap regions
        all_matches.sort_by_key(|m| (m.start, m.end));
        let mut deduped: Vec<PiiMatch> = Vec::new();
        for m in all_matches {
            let is_dup = deduped.iter().any(|existing| {
                existing.pii_type == m.pii_type
                    && existing.start <= m.end
                    && m.start <= existing.end
            });
            if !is_dup {
                deduped.push(m);
            }
        }

        Ok(deduped)
    }

    /// Single-pass NER inference (no chunking). Expects text that fits within
    /// the 512-token context window.
    fn scan_text_single_pass(&mut self, text: &str) -> Result<Vec<PiiMatch>, NerError> {
        if text.is_empty() {
            return Ok(Vec::new());
        }

        // 1. Tokenize
        let encoded = self.tokenizer.tokenize(text);
        let seq_len = encoded.input_ids.len();

        // 2. Build input tensors
        let input_ids = Array2::from_shape_vec((1, seq_len), encoded.input_ids.clone())
            .map_err(|e| NerError::Shape(e.to_string()))?;

        let attention_mask = Array2::from_shape_vec((1, seq_len), encoded.attention_mask.clone())
            .map_err(|e| NerError::Shape(e.to_string()))?;

        let token_type_ids = Array2::from_shape_vec((1, seq_len), encoded.token_type_ids.clone())
            .map_err(|e| NerError::Shape(e.to_string()))?;

        // 3. Convert ndarray to ort Values
        let input_ids_val = ort::value::Value::from_array(input_ids)
            .map_err(|e| NerError::OnnxRuntime(e.to_string()))?;
        let attention_mask_val = ort::value::Value::from_array(attention_mask)
            .map_err(|e| NerError::OnnxRuntime(e.to_string()))?;
        let token_type_ids_val = ort::value::Value::from_array(token_type_ids)
            .map_err(|e| NerError::OnnxRuntime(e.to_string()))?;

        // 4. Run inference and extract token labels (scoped to drop outputs before self borrow)
        let token_labels = {
            let outputs = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.session.run(ort::inputs![
                    "input_ids" => input_ids_val,
                    "attention_mask" => attention_mask_val,
                    "token_type_ids" => token_type_ids_val,
                ])
            })) {
                Ok(Ok(out)) => out,
                Ok(Err(e)) => return Err(NerError::OnnxRuntime(e.to_string())),
                Err(_) => {
                    return Err(NerError::OnnxRuntime(
                        "ONNX Runtime panicked during NER inference".to_string(),
                    ))
                }
            };

            let (logits_shape, logits_data) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e: ort::Error| NerError::OnnxRuntime(e.to_string()))?;
            let n_labels = logits_shape
                .last()
                .map(|&d| d as usize)
                .unwrap_or(self.num_labels);

            let mut labels: Vec<(usize, f32)> = Vec::with_capacity(seq_len);
            for i in 0..seq_len {
                if encoded.attention_mask[i] == 0 {
                    // Padding token — assign label 0 (filtered by word_ids anyway)
                    labels.push((0, 0.0));
                    continue;
                }

                let offset = i * n_labels;
                let mut max_idx = 0;
                let mut max_val = f32::NEG_INFINITY;
                for j in 0..n_labels {
                    let val = logits_data[offset + j];
                    if val > max_val {
                        max_val = val;
                        max_idx = j;
                    }
                }
                let mut exp_sum = 0.0f32;
                for j in 0..n_labels {
                    exp_sum += (logits_data[offset + j] - max_val).exp();
                }
                let confidence = 1.0 / exp_sum;
                labels.push((max_idx, confidence));
            }
            labels
        }; // outputs dropped here, releasing mutable borrow

        // 5. Decode BIO tags into entity spans, using word_ids alignment
        let entities = self.decode_bio_tags(&token_labels, &encoded.word_ids, &encoded.words, text);

        // 6. Convert to PiiMatch
        let matches = entities
            .into_iter()
            .filter(|e| e.confidence >= self.confidence_threshold)
            .map(|e| PiiMatch {
                pii_type: e.pii_type,
                start: e.byte_start,
                end: e.byte_end,
                raw_value: e.raw_value,
                json_path: None,
                confidence: e.confidence,
            })
            .collect();

        Ok(matches)
    }

    /// Decode BIO-tagged tokens into entity spans.
    fn decode_bio_tags(
        &self,
        token_labels: &[(usize, f32)],
        word_ids: &[Option<usize>],
        words: &[crate::wordpiece::WordSpan],
        original_text: &str,
    ) -> Vec<NerEntity> {
        let mut entities = Vec::new();
        let mut current_entity: Option<(PiiType, usize, usize, f32, usize)> = None;
        // (type, word_start_idx, word_end_idx, total_confidence, count)

        for (token_idx, &(label_id, confidence)) in token_labels.iter().enumerate() {
            let word_idx = match word_ids.get(token_idx).copied().flatten() {
                Some(idx) => idx,
                None => {
                    // Special token — flush current entity
                    if let Some((pii_type, ws, we, total_conf, count)) = current_entity.take() {
                        if let Some(entity) = build_entity(
                            pii_type,
                            ws,
                            we,
                            total_conf / count as f32,
                            words,
                            original_text,
                        ) {
                            entities.push(entity);
                        }
                    }
                    continue;
                }
            };

            // Only process first sub-token of each word for B/I decisions;
            // continuation sub-tokens inherit the same word's label.
            let is_first_subtoken =
                token_idx == 0 || word_ids.get(token_idx - 1).copied().flatten() != Some(word_idx);

            if !is_first_subtoken {
                continue; // Skip continuation sub-tokens
            }

            let entity_type = self.bio_to_pii_type(label_id);

            match (label_id, &mut current_entity) {
                // B-* tag: start new entity (flush previous if any)
                (l, _) if self.is_b_tag(l) => {
                    if let Some((pii_type, ws, we, total_conf, count)) = current_entity.take() {
                        if let Some(entity) = build_entity(
                            pii_type,
                            ws,
                            we,
                            total_conf / count as f32,
                            words,
                            original_text,
                        ) {
                            entities.push(entity);
                        }
                    }
                    if let Some(pii_type) = entity_type {
                        current_entity = Some((pii_type, word_idx, word_idx, confidence, 1));
                    }
                }
                // I-* tag: continue current entity if same type
                (l, Some((cur_type, _, ref mut we, ref mut total_conf, ref mut count)))
                    if self.is_i_tag(l) && entity_type == Some(*cur_type) =>
                {
                    *we = word_idx;
                    *total_conf += confidence;
                    *count += 1;
                }
                // I-* tag but no current entity or type mismatch → treat as B
                (l, _) if self.is_i_tag(l) => {
                    if let Some((pii_type, ws, we, total_conf, count)) = current_entity.take() {
                        if let Some(entity) = build_entity(
                            pii_type,
                            ws,
                            we,
                            total_conf / count as f32,
                            words,
                            original_text,
                        ) {
                            entities.push(entity);
                        }
                    }
                    if let Some(pii_type) = entity_type {
                        current_entity = Some((pii_type, word_idx, word_idx, confidence, 1));
                    }
                }
                // O tag: flush
                _ => {
                    if let Some((pii_type, ws, we, total_conf, count)) = current_entity.take() {
                        if let Some(entity) = build_entity(
                            pii_type,
                            ws,
                            we,
                            total_conf / count as f32,
                            words,
                            original_text,
                        ) {
                            entities.push(entity);
                        }
                    }
                }
            }
        }

        // Flush remaining entity
        if let Some((pii_type, ws, we, total_conf, count)) = current_entity.take() {
            if let Some(entity) = build_entity(
                pii_type,
                ws,
                we,
                total_conf / count as f32,
                words,
                original_text,
            ) {
                entities.push(entity);
            }
        }

        entities
    }
}

/// Parse a BIO label name (e.g. "B-PER", "I-LOC", "O") into a LabelInfo.
fn parse_label_name(name: &str) -> LabelInfo {
    if name == "O" {
        return LabelInfo {
            tag: BioTag::O,
            pii_type: None,
        };
    }
    if let Some(entity) = name.strip_prefix("B-") {
        return LabelInfo {
            tag: BioTag::B,
            pii_type: entity_to_pii_type(entity),
        };
    }
    if let Some(entity) = name.strip_prefix("I-") {
        return LabelInfo {
            tag: BioTag::I,
            pii_type: entity_to_pii_type(entity),
        };
    }
    // Unknown label — treat as O
    LabelInfo {
        tag: BioTag::O,
        pii_type: None,
    }
}

/// Map a NER entity suffix (e.g. "PER", "LOC") to a PiiType.
fn entity_to_pii_type(entity: &str) -> Option<PiiType> {
    match entity {
        "PER" => Some(PiiType::Person),
        "LOC" => Some(PiiType::Location),
        "ORG" => Some(PiiType::Organization),
        "HEALTH" => Some(PiiType::HealthKeyword),
        "CHILD" => Some(PiiType::ChildKeyword),
        _ => None, // MISC, etc. — no PII mapping
    }
}

/// Build a label map from label_map.json, falling back to default schema.
fn build_label_map(label_map_path: &Path) -> Result<Vec<LabelInfo>, NerError> {
    if label_map_path.exists() {
        let content =
            std::fs::read_to_string(label_map_path).map_err(|e| NerError::Io(e.to_string()))?;
        let map: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| NerError::Io(e.to_string()))?;

        // Prefer "labels" array (ordered), fall back to "id2label" map
        if let Some(labels) = map.get("labels").and_then(|v| v.as_array()) {
            return Ok(labels
                .iter()
                .map(|v| parse_label_name(v.as_str().unwrap_or("O")))
                .collect());
        }
        if let Some(id2label) = map.get("id2label").and_then(|v| v.as_object()) {
            let max_id = id2label
                .keys()
                .filter_map(|k| k.parse::<usize>().ok())
                .max()
                .unwrap_or(0);
            let mut result = vec![
                LabelInfo {
                    tag: BioTag::O,
                    pii_type: None,
                };
                max_id + 1
            ];
            for (k, v) in id2label {
                if let Ok(id) = k.parse::<usize>() {
                    result[id] = parse_label_name(v.as_str().unwrap_or("O"));
                }
            }
            return Ok(result);
        }
    }
    // Default: 11-label schema for backward compatibility
    Ok(default_label_map())
}

/// Default 11-label schema matching the original hardcoded constants.
fn default_label_map() -> Vec<LabelInfo> {
    [
        "O", "B-PER", "I-PER", "B-LOC", "I-LOC", "B-ORG", "I-ORG", "B-HEALTH", "I-HEALTH",
        "B-CHILD", "I-CHILD",
    ]
    .iter()
    .map(|name| parse_label_name(name))
    .collect()
}

/// Build a NerEntity from a span of word indices.
fn build_entity(
    pii_type: PiiType,
    word_start: usize,
    word_end: usize,
    avg_confidence: f32,
    words: &[crate::wordpiece::WordSpan],
    original_text: &str,
) -> Option<NerEntity> {
    let start_word = words.get(word_start)?;
    let end_word = words.get(word_end)?;
    let byte_start = start_word.byte_start;
    let byte_end = end_word.byte_end;

    if byte_end <= byte_start || byte_end > original_text.len() {
        return None;
    }

    let raw_value = original_text[byte_start..byte_end].to_string();

    Some(NerEntity {
        pii_type,
        byte_start,
        byte_end,
        raw_value,
        confidence: avg_confidence,
    })
}

/// Split text into overlapping chunks at whitespace boundaries.
/// Returns `(byte_offset, chunk_slice)` pairs where `byte_offset` is the position
/// of the chunk start in the original text.
///
/// Each chunk is at most `max_bytes` long (breaking at the last whitespace before
/// the limit). Adjacent chunks overlap by approximately `overlap_bytes` to ensure
/// entities spanning a chunk boundary are captured by at least one chunk.
fn split_into_chunks(text: &str, max_bytes: usize, overlap_bytes: usize) -> Vec<(usize, &str)> {
    let mut chunks = Vec::new();
    let text_len = text.len();
    let mut start = 0;

    while start < text_len {
        let mut end = (start + max_bytes).min(text_len);

        // Snap end backward to a valid UTF-8 char boundary
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }

        // If we're not at the end, find the last whitespace before the limit
        if end < text_len {
            if let Some(ws_pos) = text[start..end].rfind(char::is_whitespace) {
                // Split at the whitespace (don't include it in this chunk to keep
                // a clean boundary — the next chunk's overlap will cover it)
                end = start + ws_pos;
            }
            // If no whitespace found in the entire chunk, just cut at max_bytes
            // (very unlikely with natural language text)
        }

        // Ensure we make forward progress even for degenerate inputs
        if end <= start {
            end = (start + max_bytes).min(text_len);
            // Also snap this fallback to a char boundary
            while end > start && !text.is_char_boundary(end) {
                end -= 1;
            }
            if end <= start {
                // Advance past at least one character
                end = start + text[start..].chars().next().map_or(1, |c| c.len_utf8());
            }
        }

        chunks.push((start, &text[start..end]));

        if end >= text_len {
            break;
        }

        // Next chunk starts `overlap_bytes` before the end of this chunk
        let next_start = if end > overlap_bytes {
            end - overlap_bytes
        } else {
            end
        };
        // Snap next_start forward to a valid UTF-8 char boundary
        let mut next_start = next_start;
        while next_start < end && !text.is_char_boundary(next_start) {
            next_start += 1;
        }
        // Snap next_start forward to a whitespace boundary to avoid splitting mid-word
        if let Some(ws_pos) = text[next_start..end].find(char::is_whitespace) {
            start = next_start + ws_pos;
            // Skip the whitespace character itself (may be multi-byte)
            if let Some(ws_char) = text[start..].chars().next() {
                start += ws_char.len_utf8();
            }
        } else {
            start = end;
        }
    }

    chunks
}

#[derive(Debug, thiserror::Error)]
pub enum NerError {
    #[error("Model not found in: {0}")]
    ModelNotFound(String),
    #[error("ONNX Runtime error: {0}")]
    OnnxRuntime(String),
    #[error("Tokenizer error: {0}")]
    Tokenizer(String),
    #[error("Tensor shape error: {0}")]
    Shape(String),
    #[error("IO error: {0}")]
    Io(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_label_name() {
        let info = parse_label_name("B-PER");
        assert_eq!(info.tag, BioTag::B);
        assert_eq!(info.pii_type, Some(PiiType::Person));

        let info = parse_label_name("I-LOC");
        assert_eq!(info.tag, BioTag::I);
        assert_eq!(info.pii_type, Some(PiiType::Location));

        let info = parse_label_name("O");
        assert_eq!(info.tag, BioTag::O);
        assert_eq!(info.pii_type, None);

        let info = parse_label_name("B-MISC");
        assert_eq!(info.tag, BioTag::B);
        assert_eq!(info.pii_type, None); // MISC has no PII mapping

        let info = parse_label_name("I-ORG");
        assert_eq!(info.tag, BioTag::I);
        assert_eq!(info.pii_type, Some(PiiType::Organization));
    }

    #[test]
    fn test_entity_to_pii_type() {
        assert_eq!(entity_to_pii_type("PER"), Some(PiiType::Person));
        assert_eq!(entity_to_pii_type("LOC"), Some(PiiType::Location));
        assert_eq!(entity_to_pii_type("ORG"), Some(PiiType::Organization));
        assert_eq!(entity_to_pii_type("HEALTH"), Some(PiiType::HealthKeyword));
        assert_eq!(entity_to_pii_type("CHILD"), Some(PiiType::ChildKeyword));
        assert_eq!(entity_to_pii_type("MISC"), None);
        assert_eq!(entity_to_pii_type("UNKNOWN"), None);
    }

    #[test]
    fn test_default_label_map() {
        let map = default_label_map();
        assert_eq!(map.len(), 11);
        assert_eq!(map[0].tag, BioTag::O);
        assert_eq!(map[0].pii_type, None);
        assert_eq!(map[1].tag, BioTag::B);
        assert_eq!(map[1].pii_type, Some(PiiType::Person));
        assert_eq!(map[2].tag, BioTag::I);
        assert_eq!(map[2].pii_type, Some(PiiType::Person));
        assert_eq!(map[3].tag, BioTag::B);
        assert_eq!(map[3].pii_type, Some(PiiType::Location));
        assert_eq!(map[4].tag, BioTag::I);
        assert_eq!(map[4].pii_type, Some(PiiType::Location));
        assert_eq!(map[5].tag, BioTag::B);
        assert_eq!(map[5].pii_type, Some(PiiType::Organization));
    }

    #[test]
    fn test_build_label_map_from_labels_array() {
        let dir = std::env::temp_dir().join("oo_test_label_map_array");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("label_map.json");
        std::fs::write(
            &path,
            r#"{"labels": ["O", "B-MISC", "I-MISC", "B-PER", "I-PER", "B-ORG", "I-ORG", "B-LOC", "I-LOC"]}"#,
        )
        .unwrap();

        let map = build_label_map(&path).unwrap();
        assert_eq!(map.len(), 9);
        assert_eq!(map[0].tag, BioTag::O);
        assert_eq!(map[1].tag, BioTag::B);
        assert_eq!(map[1].pii_type, None); // B-MISC → no PII mapping
        assert_eq!(map[3].tag, BioTag::B);
        assert_eq!(map[3].pii_type, Some(PiiType::Person)); // B-PER
        assert_eq!(map[5].tag, BioTag::B);
        assert_eq!(map[5].pii_type, Some(PiiType::Organization)); // B-ORG
        assert_eq!(map[7].tag, BioTag::B);
        assert_eq!(map[7].pii_type, Some(PiiType::Location)); // B-LOC

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_build_label_map_from_id2label() {
        let dir = std::env::temp_dir().join("oo_test_label_map_id2label");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("label_map.json");
        std::fs::write(
            &path,
            r#"{"id2label": {"0": "O", "1": "B-PER", "2": "I-PER", "3": "B-LOC", "4": "I-LOC"}}"#,
        )
        .unwrap();

        let map = build_label_map(&path).unwrap();
        assert_eq!(map.len(), 5);
        assert_eq!(map[0].tag, BioTag::O);
        assert_eq!(map[1].pii_type, Some(PiiType::Person));
        assert_eq!(map[3].pii_type, Some(PiiType::Location));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_build_label_map_missing_file() {
        let path = Path::new("/tmp/nonexistent_label_map_12345.json");
        let map = build_label_map(path).unwrap();
        // Falls back to default 11-label schema
        assert_eq!(map.len(), 11);
    }

    #[test]
    fn test_build_entity() {
        let words = vec![
            crate::wordpiece::WordSpan {
                text: "john".to_string(),
                byte_start: 0,
                byte_end: 4,
            },
            crate::wordpiece::WordSpan {
                text: "smith".to_string(),
                byte_start: 5,
                byte_end: 10,
            },
        ];
        let original = "John Smith";
        let entity = build_entity(PiiType::Person, 0, 1, 0.95, &words, original);
        assert!(entity.is_some());
        let e = entity.unwrap();
        assert_eq!(e.pii_type, PiiType::Person);
        assert_eq!(e.raw_value, "John Smith");
        assert_eq!(e.byte_start, 0);
        assert_eq!(e.byte_end, 10);
    }

    /// Integration test: loads mock model and runs inference.
    #[test]
    fn test_catch_unwind_ner_panic_to_error() {
        let result: Result<(), NerError> = match std::panic::catch_unwind(
            std::panic::AssertUnwindSafe(|| -> Result<(), String> {
                panic!("simulated NER panic");
            }),
        ) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(NerError::OnnxRuntime(e)),
            Err(_) => Err(NerError::OnnxRuntime(
                "ONNX Runtime panicked during NER inference".to_string(),
            )),
        };
        assert!(result.is_err());
        assert!(matches!(&result, Err(NerError::OnnxRuntime(msg)) if msg.contains("panicked")));
    }

    /// Only runs if the mock model exists (generated by Python pipeline).
    #[test]
    fn test_mock_model_inference() {
        let mock_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("openobscure-ner/models/mock");

        if !mock_dir.join("model_int8.onnx").exists() {
            eprintln!("Skipping mock model test: {} not found", mock_dir.display());
            return;
        }

        let mut scanner = NerScanner::load(&mock_dir, 0.0).expect("Failed to load mock NER model");
        let matches = scanner
            .scan_text("John Smith has diabetes")
            .expect("Inference failed");

        // Mock model has random weights, so we don't assert specific entities.
        // Just verify it runs without error and returns results.
        eprintln!("Mock model returned {} matches", matches.len());
    }

    #[test]
    fn test_split_into_chunks_short_text() {
        let text = "Hello world";
        let chunks = split_into_chunks(text, 1500, 200);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], (0, "Hello world"));
    }

    #[test]
    fn test_split_into_chunks_exact_boundary() {
        let text = "Hello world"; // 11 bytes
        let chunks = split_into_chunks(text, 11, 3);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], (0, "Hello world"));
    }

    #[test]
    fn test_split_into_chunks_two_chunks() {
        // Build text that exceeds max_bytes so it needs two chunks
        let text = "aaa bbb ccc ddd eee fff ggg hhh iii jjj";
        // max_bytes=20, overlap=5 — first chunk ends around byte 20
        let chunks = split_into_chunks(text, 20, 5);
        assert!(
            chunks.len() >= 2,
            "Expected at least 2 chunks, got {}",
            chunks.len()
        );
        // First chunk starts at 0
        assert_eq!(chunks[0].0, 0);
        // All chunks combined should cover the full text
        let last = chunks.last().unwrap();
        assert_eq!(last.0 + last.1.len(), text.len());
    }

    #[test]
    fn test_split_into_chunks_overlap_coverage() {
        // Ensure overlap region is covered by both adjacent chunks
        let words: Vec<String> = (0..50).map(|i| format!("word{:03}", i)).collect();
        let text = words.join(" ");
        let chunks = split_into_chunks(&text, 100, 30);

        // Verify every byte in the text is covered by at least one chunk
        let mut covered = vec![false; text.len()];
        for (offset, chunk) in &chunks {
            for i in 0..chunk.len() {
                covered[offset + i] = true;
            }
        }
        // Allow trailing whitespace to be uncovered (it's between chunks)
        for (i, &c) in covered.iter().enumerate() {
            if !c {
                assert!(
                    text.as_bytes()[i].is_ascii_whitespace(),
                    "Byte {} ('{}') not covered by any chunk",
                    i,
                    text.as_bytes()[i] as char,
                );
            }
        }
    }

    #[test]
    fn test_split_into_chunks_whitespace_boundaries() {
        // Chunks should not split mid-word
        let text = "alpha bravo charlie delta echo foxtrot golf hotel india juliet";
        let chunks = split_into_chunks(text, 25, 8);
        for (_, chunk) in &chunks {
            // No chunk should start or end mid-word (no partial words)
            assert!(
                !chunk.starts_with(' '),
                "Chunk starts with space: {:?}",
                chunk
            );
            assert!(!chunk.ends_with(' '), "Chunk ends with space: {:?}", chunk);
        }
    }

    #[test]
    fn test_split_into_chunks_empty() {
        let chunks = split_into_chunks("", 100, 20);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_split_into_chunks_single_long_word() {
        // A single word longer than max_bytes — should still produce a chunk
        let text = "a".repeat(200);
        let chunks = split_into_chunks(&text, 100, 20);
        assert!(!chunks.is_empty());
        // Combined chunks should cover the full text
        let total_covered: usize = chunks.iter().map(|(_, c)| c.len()).sum();
        assert!(total_covered >= text.len());
    }

    #[test]
    fn test_split_into_chunks_multibyte_utf8() {
        // Arabic text with multi-byte characters (2-3 bytes each).
        // Must not panic on char boundary issues.
        let text = "مرحبا بالعالم هذا نص اختبار طويل يحتوي على كلمات عربية كثيرة \
                    لاختبار التقسيم إلى أجزاء متداخلة بشكل صحيح ونتأكد \
                    أن الحدود لا تقع في وسط حرف متعدد البايت";
        // Use a small chunk size to force multiple chunks through multi-byte text
        let chunks = split_into_chunks(text, 50, 15);
        assert!(chunks.len() > 1, "Should produce multiple chunks");
        for (offset, chunk) in &chunks {
            // Every chunk must be valid UTF-8 (implicit: &str guarantees this)
            assert!(
                text.is_char_boundary(*offset),
                "Chunk offset {} not on char boundary",
                offset
            );
            assert!(
                text.is_char_boundary(offset + chunk.len()),
                "Chunk end {} not on char boundary",
                offset + chunk.len()
            );
        }
    }
}
