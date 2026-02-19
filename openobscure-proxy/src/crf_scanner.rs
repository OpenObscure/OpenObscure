use std::collections::HashMap;
use std::path::Path;

use crate::keyword_dict::KeywordDict;
use crate::pii_types::PiiType;
use crate::scanner::PiiMatch;

/// BIO label indices — same schema as NER scanner.
#[allow(dead_code)] // Used in tests; documents BIO label set
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

#[allow(dead_code)] // Used in tests; documents model schema
const NUM_LABELS: usize = 11;

/// CRF-based NER scanner — lightweight fallback for <200MB RAM devices.
///
/// Uses a linear-chain CRF with hand-crafted features:
/// word shape, prefix/suffix, capitalization, gazetteer membership, context window.
/// Inference via Viterbi decoding (~2ms, <10MB RAM).
pub struct CrfScanner {
    model: CrfModel,
    gazetteer_health: std::collections::HashSet<String>,
    gazetteer_child: std::collections::HashSet<String>,
    confidence_threshold: f32,
}

/// Trained CRF model weights.
struct CrfModel {
    /// Feature name → score vector (one score per label).
    state_features: HashMap<String, Vec<f64>>,
    /// Transition weights: transitions[from_label][to_label].
    transitions: Vec<Vec<f64>>,
    num_labels: usize,
}

/// A word token with byte offsets for span reconstruction.
#[derive(Debug)]
struct CrfToken {
    text: String,
    lower: String,
    byte_start: usize,
    byte_end: usize,
}

impl CrfScanner {
    /// Load a CRF model from a directory containing `crf_model.json`.
    pub fn load(model_dir: &Path, confidence_threshold: f32) -> Result<Self, CrfError> {
        let model_path = model_dir.join("crf_model.json");
        if !model_path.exists() {
            return Err(CrfError::ModelNotFound(model_dir.display().to_string()));
        }

        let content =
            std::fs::read_to_string(&model_path).map_err(|e| CrfError::Io(e.to_string()))?;
        let model = CrfModel::from_json(&content)?;

        // Build gazetteers from keyword dict for feature extraction
        let dict = KeywordDict::new();
        let gazetteer_health = dict.health_terms_clone();
        let gazetteer_child = dict.child_terms_clone();

        oo_info!(
            crate::oo_log::modules::CRF,
            "CRF model loaded",
            features = model.state_features.len(),
            labels = model.num_labels
        );

        Ok(Self {
            model,
            gazetteer_health,
            gazetteer_child,
            confidence_threshold,
        })
    }

    /// Run CRF inference on text, returning detected PII entities.
    pub fn scan_text(&self, text: &str) -> Vec<PiiMatch> {
        if text.is_empty() {
            return Vec::new();
        }

        let tokens = tokenize_for_crf(text);
        if tokens.is_empty() {
            return Vec::new();
        }

        // Extract features for each token
        let features: Vec<Vec<String>> = (0..tokens.len())
            .map(|i| self.extract_features(&tokens, i))
            .collect();

        // Run Viterbi decoding
        let (labels, scores) = self.viterbi(&features);

        // Decode BIO labels into entity spans
        self.decode_entities(&tokens, &labels, &scores, text)
    }

    /// Extract features for a token at position `idx` in the token sequence.
    fn extract_features(&self, tokens: &[CrfToken], idx: usize) -> Vec<String> {
        let mut feats = Vec::with_capacity(24);
        let token = &tokens[idx];
        let word = &token.lower;

        // Current word features
        feats.push(format!("word={}", word));
        feats.push(format!("shape={}", word_shape(&token.text)));

        // Prefix/suffix features (1-3 chars)
        if !word.is_empty() {
            feats.push(format!("p1={}", &word[..1]));
            feats.push(format!("s1={}", &word[word.len() - 1..]));
        }
        if word.len() >= 2 {
            feats.push(format!("p2={}", &word[..2]));
            feats.push(format!("s2={}", &word[word.len() - 2..]));
        }
        if word.len() >= 3 {
            feats.push(format!("p3={}", &word[..3]));
            feats.push(format!("s3={}", &word[word.len() - 3..]));
        }

        // Capitalization features
        if token.text.chars().all(|c| c.is_uppercase()) {
            feats.push("isupper".to_string());
        }
        if token.text.chars().next().is_some_and(|c| c.is_uppercase())
            && token.text.chars().skip(1).all(|c| c.is_lowercase())
        {
            feats.push("istitle".to_string());
        }
        if token.text.chars().all(|c| c.is_ascii_digit()) {
            feats.push("isdigit".to_string());
        }
        if token.text.chars().all(|c| c.is_alphabetic()) {
            feats.push("isalpha".to_string());
        }

        // Word length bucket
        let len_bucket = match word.len() {
            0..=2 => "short",
            3..=5 => "medium",
            6..=10 => "long",
            _ => "vlong",
        };
        feats.push(format!("len={}", len_bucket));

        // Gazetteer membership
        if self.gazetteer_health.contains(word) {
            feats.push("gaz=health".to_string());
        }
        if self.gazetteer_child.contains(word) {
            feats.push("gaz=child".to_string());
        }

        // Context features (±1 window)
        if idx == 0 {
            feats.push("BOS".to_string());
        } else {
            let prev = &tokens[idx - 1];
            feats.push(format!("-1:word={}", prev.lower));
            feats.push(format!("-1:shape={}", word_shape(&prev.text)));
        }
        if idx == tokens.len() - 1 {
            feats.push("EOS".to_string());
        } else {
            let next = &tokens[idx + 1];
            feats.push(format!("+1:word={}", next.lower));
            feats.push(format!("+1:shape={}", word_shape(&next.text)));
        }

        feats
    }

    /// Viterbi decoding over the feature sequence.
    /// Returns (best_labels, marginal_scores) for each token.
    #[allow(clippy::needless_range_loop)]
    fn viterbi(&self, features: &[Vec<String>]) -> (Vec<usize>, Vec<f64>) {
        let n = features.len();
        let nl = self.model.num_labels;

        // viterbi[t][j] = best score arriving at label j at time t
        let mut viterbi_scores = vec![vec![f64::NEG_INFINITY; nl]; n];
        let mut backpointers = vec![vec![0usize; nl]; n];

        // Compute state scores for first token
        let state_scores_0 = self.compute_state_scores(&features[0]);
        viterbi_scores[0][..nl].copy_from_slice(&state_scores_0[..nl]);

        // Forward pass
        for t in 1..n {
            let state_scores = self.compute_state_scores(&features[t]);
            for j in 0..nl {
                let mut best_score = f64::NEG_INFINITY;
                let mut best_prev = 0;
                for i in 0..nl {
                    let score = viterbi_scores[t - 1][i] + self.model.transitions[i][j];
                    if score > best_score {
                        best_score = score;
                        best_prev = i;
                    }
                }
                viterbi_scores[t][j] = best_score + state_scores[j];
                backpointers[t][j] = best_prev;
            }
        }

        // Backward pass — find best final label
        let mut best_label = 0;
        let mut best_score = f64::NEG_INFINITY;
        for j in 0..nl {
            if viterbi_scores[n - 1][j] > best_score {
                best_score = viterbi_scores[n - 1][j];
                best_label = j;
            }
        }

        let mut labels = vec![0usize; n];
        let mut scores = vec![0.0f64; n];
        labels[n - 1] = best_label;
        scores[n - 1] = viterbi_scores[n - 1][best_label];
        for t in (0..n - 1).rev() {
            labels[t] = backpointers[t + 1][labels[t + 1]];
            scores[t] = viterbi_scores[t][labels[t]];
        }

        (labels, scores)
    }

    /// Sum state feature weights for a set of features, returning per-label scores.
    fn compute_state_scores(&self, features: &[String]) -> Vec<f64> {
        let mut scores = vec![0.0f64; self.model.num_labels];
        for feat in features {
            if let Some(weights) = self.model.state_features.get(feat) {
                for (j, &w) in weights.iter().enumerate() {
                    if j < scores.len() {
                        scores[j] += w;
                    }
                }
            }
        }
        scores
    }

    /// Convert Viterbi label sequence into PiiMatch entities.
    fn decode_entities(
        &self,
        tokens: &[CrfToken],
        labels: &[usize],
        scores: &[f64],
        original_text: &str,
    ) -> Vec<PiiMatch> {
        let mut entities = Vec::new();
        let mut current: Option<(PiiType, usize, usize, f64, usize)> = None;
        // (type, token_start, token_end, total_score, count)

        for (i, &label) in labels.iter().enumerate() {
            let pii_type = bio_to_pii_type(label);

            match (label, &mut current) {
                // B-* tag: start new entity
                (l, _) if is_b_tag(l) => {
                    if let Some((pt, ts, te, total, count)) = current.take() {
                        self.push_entity(
                            &mut entities,
                            tokens,
                            pt,
                            ts,
                            te,
                            total / count as f64,
                            original_text,
                        );
                    }
                    if let Some(pt) = pii_type {
                        current = Some((pt, i, i, scores[i], 1));
                    }
                }
                // I-* tag continuing same type
                (l, Some((cur_type, _, ref mut te, ref mut total, ref mut count)))
                    if is_i_tag(l) && pii_type == Some(*cur_type) =>
                {
                    *te = i;
                    *total += scores[i];
                    *count += 1;
                }
                // I-* tag but wrong type or no current → treat as B
                (l, _) if is_i_tag(l) => {
                    if let Some((pt, ts, te, total, count)) = current.take() {
                        self.push_entity(
                            &mut entities,
                            tokens,
                            pt,
                            ts,
                            te,
                            total / count as f64,
                            original_text,
                        );
                    }
                    if let Some(pt) = pii_type {
                        current = Some((pt, i, i, scores[i], 1));
                    }
                }
                // O tag: flush
                _ => {
                    if let Some((pt, ts, te, total, count)) = current.take() {
                        self.push_entity(
                            &mut entities,
                            tokens,
                            pt,
                            ts,
                            te,
                            total / count as f64,
                            original_text,
                        );
                    }
                }
            }
        }

        // Flush remaining
        if let Some((pt, ts, te, total, count)) = current.take() {
            self.push_entity(
                &mut entities,
                tokens,
                pt,
                ts,
                te,
                total / count as f64,
                original_text,
            );
        }

        entities
    }

    #[allow(clippy::too_many_arguments)]
    fn push_entity(
        &self,
        entities: &mut Vec<PiiMatch>,
        tokens: &[CrfToken],
        pii_type: PiiType,
        token_start: usize,
        token_end: usize,
        avg_score: f64,
        original_text: &str,
    ) {
        // Simple confidence: sigmoid of average score
        let confidence = 1.0 / (1.0 + (-avg_score).exp());
        if confidence < self.confidence_threshold as f64 {
            return;
        }

        if let (Some(start_tok), Some(end_tok)) = (tokens.get(token_start), tokens.get(token_end)) {
            let byte_start = start_tok.byte_start;
            let byte_end = end_tok.byte_end;
            if byte_end <= byte_start || byte_end > original_text.len() {
                return;
            }
            entities.push(PiiMatch {
                pii_type,
                start: byte_start,
                end: byte_end,
                raw_value: original_text[byte_start..byte_end].to_string(),
                json_path: None,
                confidence: confidence as f32,
            });
        }
    }
}

impl CrfModel {
    fn from_json(json_str: &str) -> Result<Self, CrfError> {
        let value: serde_json::Value =
            serde_json::from_str(json_str).map_err(|e| CrfError::ModelParse(e.to_string()))?;

        // Parse state features
        let state_features_val = value
            .get("state_features")
            .ok_or_else(|| CrfError::ModelParse("missing 'state_features'".to_string()))?;
        let state_features: HashMap<String, Vec<f64>> =
            serde_json::from_value(state_features_val.clone())
                .map_err(|e| CrfError::ModelParse(format!("state_features: {}", e)))?;

        // Parse transitions
        let transitions_val = value
            .get("transitions")
            .ok_or_else(|| CrfError::ModelParse("missing 'transitions'".to_string()))?;
        let transitions: Vec<Vec<f64>> = serde_json::from_value(transitions_val.clone())
            .map_err(|e| CrfError::ModelParse(format!("transitions: {}", e)))?;

        let num_labels = transitions.len();
        if num_labels == 0 {
            return Err(CrfError::ModelParse("empty transitions matrix".to_string()));
        }
        for row in &transitions {
            if row.len() != num_labels {
                return Err(CrfError::ModelParse(
                    "transitions matrix not square".to_string(),
                ));
            }
        }

        Ok(Self {
            state_features,
            transitions,
            num_labels,
        })
    }
}

/// Tokenize text into words with byte offsets, splitting on whitespace/punctuation.
fn tokenize_for_crf(text: &str) -> Vec<CrfToken> {
    let mut tokens = Vec::new();
    let mut start = None;

    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() || c == '\'' || c == '-' {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start {
            let word = &text[s..i];
            let trimmed = word.trim_matches(|c: char| c == '-' || c == '\'');
            if !trimmed.is_empty() {
                let trim_offset = word.find(trimmed).unwrap_or(0);
                tokens.push(CrfToken {
                    text: text[s + trim_offset..s + trim_offset + trimmed.len()].to_string(),
                    lower: trimmed.to_lowercase(),
                    byte_start: s + trim_offset,
                    byte_end: s + trim_offset + trimmed.len(),
                });
            }
            start = None;
        }
    }

    if let Some(s) = start {
        let word = &text[s..];
        let trimmed = word.trim_matches(|c: char| c == '-' || c == '\'');
        if !trimmed.is_empty() {
            let trim_offset = word.find(trimmed).unwrap_or(0);
            tokens.push(CrfToken {
                text: text[s + trim_offset..s + trim_offset + trimmed.len()].to_string(),
                lower: trimmed.to_lowercase(),
                byte_start: s + trim_offset,
                byte_end: s + trim_offset + trimmed.len(),
            });
        }
    }

    tokens
}

/// Generate a word shape string:
/// - Uppercase → 'X', lowercase → 'x', digit → 'd', other → itself
/// - Consecutive same-type chars are collapsed (e.g., "John" → "Xx", "123-456" → "d-d")
fn word_shape(word: &str) -> String {
    let mut shape = String::new();
    let mut last_type = ' ';

    for c in word.chars() {
        let t = if c.is_uppercase() {
            'X'
        } else if c.is_lowercase() {
            'x'
        } else if c.is_ascii_digit() {
            'd'
        } else {
            c
        };

        if t != last_type {
            shape.push(t);
            last_type = t;
        }
    }

    shape
}

fn is_b_tag(label: usize) -> bool {
    matches!(
        label,
        LABEL_B_PER | LABEL_B_LOC | LABEL_B_ORG | LABEL_B_HEALTH | LABEL_B_CHILD
    )
}

fn is_i_tag(label: usize) -> bool {
    matches!(
        label,
        LABEL_I_PER | LABEL_I_LOC | LABEL_I_ORG | LABEL_I_HEALTH | LABEL_I_CHILD
    )
}

fn bio_to_pii_type(label: usize) -> Option<PiiType> {
    match label {
        LABEL_B_PER | LABEL_I_PER => Some(PiiType::Person),
        LABEL_B_LOC | LABEL_I_LOC => Some(PiiType::Location),
        LABEL_B_ORG | LABEL_I_ORG => Some(PiiType::Organization),
        LABEL_B_HEALTH | LABEL_I_HEALTH => Some(PiiType::HealthKeyword),
        LABEL_B_CHILD | LABEL_I_CHILD => Some(PiiType::ChildKeyword),
        _ => None,
    }
}

/// Get available system RAM in MB. Returns None if unavailable.
pub fn available_ram_mb() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        // Use vm_stat to get free + inactive pages, multiply by page size
        let output = std::process::Command::new("vm_stat").output().ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let mut free_pages: u64 = 0;
        for line in text.lines() {
            if line.starts_with("Pages free:") || line.starts_with("Pages inactive:") {
                let val: String = line.chars().filter(|c| c.is_ascii_digit()).collect();
                free_pages += val.parse::<u64>().unwrap_or(0);
            }
        }
        // macOS page size is 16384 on Apple Silicon, 4096 on Intel
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
        Some(free_pages * page_size / (1024 * 1024))
    }
    #[cfg(target_os = "linux")]
    {
        let content = std::fs::read_to_string("/proc/meminfo").ok()?;
        for line in content.lines() {
            if line.starts_with("MemAvailable:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let kb = parts[1].parse::<u64>().ok()?;
                    return Some(kb / 1024);
                }
            }
        }
        None
    }
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status = MEMORYSTATUSEX::default();
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut status as *mut MEMORYSTATUSEX) }.is_ok() {
            return Some(status.ullAvailPhys / (1024 * 1024));
        }
        None
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CrfError {
    #[error("CRF model not found in: {0}")]
    ModelNotFound(String),
    #[error("CRF model parse error: {0}")]
    ModelParse(String),
    #[error("IO error: {0}")]
    Io(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_shape() {
        assert_eq!(word_shape("John"), "Xx");
        assert_eq!(word_shape("USA"), "X");
        assert_eq!(word_shape("hello"), "x");
        assert_eq!(word_shape("123"), "d");
        assert_eq!(word_shape("123-456"), "d-d");
        assert_eq!(word_shape("McDonald's"), "XxXx'x");
    }

    #[test]
    fn test_tokenize_for_crf() {
        let tokens = tokenize_for_crf("John Smith has diabetes");
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[0].text, "John");
        assert_eq!(tokens[0].lower, "john");
        assert_eq!(tokens[0].byte_start, 0);
        assert_eq!(tokens[0].byte_end, 4);
        assert_eq!(tokens[1].text, "Smith");
        assert_eq!(tokens[1].byte_start, 5);
    }

    #[test]
    fn test_tokenize_preserves_hyphens() {
        let tokens = tokenize_for_crf("8-year-old child");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].text, "8-year-old");
        assert_eq!(tokens[1].text, "child");
    }

    #[test]
    fn test_bio_helpers() {
        assert!(is_b_tag(LABEL_B_PER));
        assert!(is_b_tag(LABEL_B_LOC));
        assert!(!is_b_tag(LABEL_I_PER));
        assert!(!is_b_tag(LABEL_O));

        assert!(is_i_tag(LABEL_I_PER));
        assert!(!is_i_tag(LABEL_B_PER));

        assert_eq!(bio_to_pii_type(LABEL_B_PER), Some(PiiType::Person));
        assert_eq!(bio_to_pii_type(LABEL_O), None);
    }

    #[test]
    fn test_crf_model_parse() {
        let json = mock_model_json();
        let model = CrfModel::from_json(&json).unwrap();
        assert_eq!(model.num_labels, NUM_LABELS);
        assert!(!model.state_features.is_empty());
    }

    #[test]
    fn test_crf_viterbi_with_mock_model() {
        let json = mock_model_json();
        let model = CrfModel::from_json(&json).unwrap();
        let scanner = CrfScanner {
            model,
            gazetteer_health: std::collections::HashSet::new(),
            gazetteer_child: std::collections::HashSet::new(),
            confidence_threshold: 0.0, // Accept everything for testing
        };

        let matches = scanner.scan_text("John Smith has diabetes");
        // Mock model has weak weights; just verify it runs without panic
        // and produces valid output
        for m in &matches {
            assert!(m.start < m.end);
            assert!(m.end <= "John Smith has diabetes".len());
        }
    }

    #[test]
    fn test_crf_empty_text() {
        let json = mock_model_json();
        let model = CrfModel::from_json(&json).unwrap();
        let scanner = CrfScanner {
            model,
            gazetteer_health: std::collections::HashSet::new(),
            gazetteer_child: std::collections::HashSet::new(),
            confidence_threshold: 0.5,
        };
        let matches = scanner.scan_text("");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_feature_extraction() {
        let json = mock_model_json();
        let model = CrfModel::from_json(&json).unwrap();
        let scanner = CrfScanner {
            model,
            gazetteer_health: {
                let mut s = std::collections::HashSet::new();
                s.insert("diabetes".to_string());
                s
            },
            gazetteer_child: std::collections::HashSet::new(),
            confidence_threshold: 0.5,
        };

        let tokens = tokenize_for_crf("John has diabetes");
        let features = scanner.extract_features(&tokens, 0);
        assert!(features.contains(&"word=john".to_string()));
        assert!(features.contains(&"shape=Xx".to_string()));
        assert!(features.contains(&"istitle".to_string()));
        assert!(features.contains(&"BOS".to_string()));

        let features_last = scanner.extract_features(&tokens, 2);
        assert!(features_last.contains(&"word=diabetes".to_string()));
        assert!(features_last.contains(&"gaz=health".to_string()));
        assert!(features_last.contains(&"EOS".to_string()));
    }

    #[test]
    fn test_available_ram() {
        // Just verify it doesn't panic; result depends on platform
        let ram = available_ram_mb();
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            // Should return Some on supported platforms
            assert!(ram.is_some(), "Expected Some(ram_mb) on this platform");
        }
    }

    #[test]
    fn test_load_generated_mock_model() {
        // Load the mock CRF model generated by generate_mock_crf_model.py
        let model_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("openobscure-ner/models/crf_mock");
        if !model_dir.join("crf_model.json").exists() {
            // Skip if mock model hasn't been generated yet
            eprintln!(
                "Skipping: mock CRF model not found at {}",
                model_dir.display()
            );
            return;
        }

        let scanner = CrfScanner::load(&model_dir, 0.5).expect("Failed to load mock CRF model");

        // Test with health + person text
        let matches = scanner.scan_text("John Smith has diabetes and hypertension");
        // The mock model has strong enough weights to detect these
        let types: Vec<_> = matches.iter().map(|m| m.pii_type).collect();
        // At minimum, gazetteer-backed features should trigger health matches
        assert!(
            matches.iter().any(|m| m.pii_type == PiiType::HealthKeyword),
            "Expected at least one HEALTH match, got: {:?}",
            types
        );

        // Verify all matches have valid spans
        for m in &matches {
            assert!(m.start < m.end);
            assert!(m.end <= "John Smith has diabetes and hypertension".len());
        }
    }

    /// Generate a minimal mock CRF model JSON for testing.
    fn mock_model_json() -> String {
        let mut state_features: HashMap<String, Vec<f64>> = HashMap::new();

        // Add some features that bias toward B-PER for title-cased words
        state_features.insert("istitle".to_string(), {
            let mut v = vec![0.0; NUM_LABELS];
            v[LABEL_B_PER] = 1.5;
            v
        });
        state_features.insert("word=john".to_string(), {
            let mut v = vec![0.0; NUM_LABELS];
            v[LABEL_B_PER] = 2.0;
            v
        });
        state_features.insert("word=smith".to_string(), {
            let mut v = vec![0.0; NUM_LABELS];
            v[LABEL_I_PER] = 1.5;
            v[LABEL_B_PER] = 0.5;
            v
        });
        state_features.insert("gaz=health".to_string(), {
            let mut v = vec![0.0; NUM_LABELS];
            v[LABEL_B_HEALTH] = 2.0;
            v
        });
        state_features.insert("gaz=child".to_string(), {
            let mut v = vec![0.0; NUM_LABELS];
            v[LABEL_B_CHILD] = 2.0;
            v
        });

        // Transition matrix: favor O→B and B→I of same type
        let mut transitions = vec![vec![0.0; NUM_LABELS]; NUM_LABELS];
        // O→O is slightly positive (common)
        transitions[LABEL_O][LABEL_O] = 0.5;
        // O→B-* is neutral
        // B→I of same type is positive
        transitions[LABEL_B_PER][LABEL_I_PER] = 1.0;
        transitions[LABEL_B_LOC][LABEL_I_LOC] = 1.0;
        transitions[LABEL_B_ORG][LABEL_I_ORG] = 1.0;
        transitions[LABEL_B_HEALTH][LABEL_I_HEALTH] = 1.0;
        transitions[LABEL_B_CHILD][LABEL_I_CHILD] = 1.0;
        // I→I of same type is positive
        transitions[LABEL_I_PER][LABEL_I_PER] = 0.5;
        transitions[LABEL_I_LOC][LABEL_I_LOC] = 0.5;
        // Cross-type transitions are negative
        for i in 1..NUM_LABELS {
            for j in 1..NUM_LABELS {
                if transitions[i][j] == 0.0 && i != j {
                    transitions[i][j] = -0.5;
                }
            }
        }

        serde_json::json!({
            "state_features": state_features,
            "transitions": transitions,
        })
        .to_string()
    }
}
