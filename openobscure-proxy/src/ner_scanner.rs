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

    /// Run NER inference on a text string.
    /// Returns PiiMatch results for detected entities above the confidence threshold.
    pub fn scan_text(&mut self, text: &str) -> Result<Vec<PiiMatch>, NerError> {
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
            let outputs = self
                .session
                .run(ort::inputs![
                    "input_ids" => input_ids_val,
                    "attention_mask" => attention_mask_val,
                    "token_type_ids" => token_type_ids_val,
                ])
                .map_err(|e| NerError::OnnxRuntime(e.to_string()))?;

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
}
