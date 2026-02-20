use std::path::Path;

use ndarray::Array2;
use ort::session::Session;

use crate::pii_types::PiiType;
use crate::scanner::PiiMatch;
use crate::wordpiece::WordPieceTokenizer;

/// BIO label IDs matching the training pipeline's label_map.json.
const LABEL_O: usize = 0;
const LABEL_B_PER: usize = 1;
const LABEL_I_PER: usize = 2;
const LABEL_B_LOC: usize = 3;
const LABEL_I_LOC: usize = 4;
const LABEL_B_ORG: usize = 5;
const LABEL_I_ORG: usize = 6;
const LABEL_B_HEALTH: usize = 7;
const LABEL_I_HEALTH: usize = 8;
const LABEL_B_CHILD: usize = 9;
const LABEL_I_CHILD: usize = 10;

/// NER scanner using TinyBERT ONNX model via ONNX Runtime.
///
/// Loaded once at startup and shared via `Arc`. Runs inference on text
/// to detect semantic PII: person names, locations, organizations,
/// health references, and child references.
pub struct NerScanner {
    session: Session,
    tokenizer: WordPieceTokenizer,
    confidence_threshold: f32,
    num_labels: usize,
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

        // Load label map to determine num_labels
        let label_map_path = model_dir.join("label_map.json");
        let num_labels = if label_map_path.exists() {
            let content = std::fs::read_to_string(&label_map_path)
                .map_err(|e| NerError::Io(e.to_string()))?;
            let map: serde_json::Value =
                serde_json::from_str(&content).map_err(|e| NerError::Io(e.to_string()))?;
            map.get("labels")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(11)
        } else {
            11 // Default: our standard 11-label schema
        };

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
        })
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
                    labels.push((LABEL_O, 0.0));
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

        // 7. Convert to PiiMatch
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
        // (type, word_start_idx, word_end_idx, min_confidence, count)

        for (token_idx, &(label_id, confidence)) in token_labels.iter().enumerate() {
            let word_idx = match word_ids.get(token_idx).copied().flatten() {
                Some(idx) => idx,
                None => {
                    // Special token — flush current entity
                    if let Some((pii_type, ws, we, min_conf, count)) = current_entity.take() {
                        if let Some(entity) = build_entity(
                            pii_type,
                            ws,
                            we,
                            min_conf / count as f32,
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

            let entity_type = bio_to_pii_type(label_id);

            match (label_id, &mut current_entity) {
                // B-* tag: start new entity (flush previous if any)
                (l, _) if is_b_tag(l) => {
                    if let Some((pii_type, ws, we, min_conf, count)) = current_entity.take() {
                        if let Some(entity) = build_entity(
                            pii_type,
                            ws,
                            we,
                            min_conf / count as f32,
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
                    if is_i_tag(l) && entity_type == Some(*cur_type) =>
                {
                    *we = word_idx;
                    *total_conf += confidence;
                    *count += 1;
                }
                // I-* tag but no current entity or type mismatch → treat as B
                (l, _) if is_i_tag(l) => {
                    if let Some((pii_type, ws, we, min_conf, count)) = current_entity.take() {
                        if let Some(entity) = build_entity(
                            pii_type,
                            ws,
                            we,
                            min_conf / count as f32,
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
                    if let Some((pii_type, ws, we, min_conf, count)) = current_entity.take() {
                        if let Some(entity) = build_entity(
                            pii_type,
                            ws,
                            we,
                            min_conf / count as f32,
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
        if let Some((pii_type, ws, we, min_conf, count)) = current_entity.take() {
            if let Some(entity) = build_entity(
                pii_type,
                ws,
                we,
                min_conf / count as f32,
                words,
                original_text,
            ) {
                entities.push(entity);
            }
        }

        entities
    }
}

fn is_b_tag(label_id: usize) -> bool {
    matches!(
        label_id,
        LABEL_B_PER | LABEL_B_LOC | LABEL_B_ORG | LABEL_B_HEALTH | LABEL_B_CHILD
    )
}

fn is_i_tag(label_id: usize) -> bool {
    matches!(
        label_id,
        LABEL_I_PER | LABEL_I_LOC | LABEL_I_ORG | LABEL_I_HEALTH | LABEL_I_CHILD
    )
}

fn bio_to_pii_type(label_id: usize) -> Option<PiiType> {
    match label_id {
        LABEL_B_PER | LABEL_I_PER => Some(PiiType::Person),
        LABEL_B_LOC | LABEL_I_LOC => Some(PiiType::Location),
        LABEL_B_ORG | LABEL_I_ORG => Some(PiiType::Organization),
        LABEL_B_HEALTH | LABEL_I_HEALTH => Some(PiiType::HealthKeyword),
        LABEL_B_CHILD | LABEL_I_CHILD => Some(PiiType::ChildKeyword),
        _ => None,
    }
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
    fn test_bio_to_pii_type() {
        assert_eq!(bio_to_pii_type(LABEL_B_PER), Some(PiiType::Person));
        assert_eq!(bio_to_pii_type(LABEL_I_PER), Some(PiiType::Person));
        assert_eq!(bio_to_pii_type(LABEL_B_LOC), Some(PiiType::Location));
        assert_eq!(bio_to_pii_type(LABEL_B_ORG), Some(PiiType::Organization));
        assert_eq!(
            bio_to_pii_type(LABEL_B_HEALTH),
            Some(PiiType::HealthKeyword)
        );
        assert_eq!(bio_to_pii_type(LABEL_B_CHILD), Some(PiiType::ChildKeyword));
        assert_eq!(bio_to_pii_type(LABEL_O), None);
    }

    #[test]
    fn test_is_b_tag() {
        assert!(is_b_tag(LABEL_B_PER));
        assert!(is_b_tag(LABEL_B_LOC));
        assert!(!is_b_tag(LABEL_I_PER));
        assert!(!is_b_tag(LABEL_O));
    }

    #[test]
    fn test_is_i_tag() {
        assert!(is_i_tag(LABEL_I_PER));
        assert!(is_i_tag(LABEL_I_LOC));
        assert!(!is_i_tag(LABEL_B_PER));
        assert!(!is_i_tag(LABEL_O));
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
