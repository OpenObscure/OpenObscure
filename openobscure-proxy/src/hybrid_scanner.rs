use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use serde_json::Value;

use crate::crf_scanner::CrfScanner;
use crate::keyword_dict::KeywordDict;
use crate::ner_scanner::NerScanner;
use crate::pii_types::PiiType;
use crate::scanner::{PiiMatch, PiiScanner};

/// Maximum depth for recursive JSON-in-string parsing.
const MAX_NESTED_JSON_DEPTH: usize = 2;

/// Scanner source for confidence voting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ScannerSource {
    Regex,
    Keyword,
    Semantic,
}

/// A match tagged with its source scanner, used during voting.
struct TaggedMatch {
    pii_match: PiiMatch,
    source: ScannerSource,
}

/// The semantic scanner backend: NER (TinyBERT) or CRF (lightweight fallback).
enum SemanticBackend {
    Ner(Mutex<NerScanner>),
    Crf(CrfScanner),
}

/// Hybrid scanner combining regex, keyword dictionary, and semantic model (NER or CRF).
///
/// Pipeline:
/// 1. Regex scanner (fast, deterministic) → structured PII (CC, SSN, phone, email, API key)
/// 2. Keyword dictionary (O(1) lookup) → health/child terms
/// 3. Semantic backend (NER or CRF) → person names, locations, organizations
///
/// Overlapping spans are resolved via confidence-weighted voting with agreement bonus.
pub struct HybridScanner {
    regex_scanner: PiiScanner,
    keyword_dict: KeywordDict,
    semantic: Option<SemanticBackend>,
    keywords_enabled: bool,
    respect_code_fences: bool,
    min_confidence: f32,
    agreement_bonus: f32,
}

impl HybridScanner {
    /// Create a hybrid scanner with NER as the semantic backend.
    pub fn new(keywords_enabled: bool, ner_scanner: Option<NerScanner>) -> Self {
        Self {
            regex_scanner: PiiScanner::new(),
            keyword_dict: KeywordDict::new(),
            semantic: ner_scanner.map(|n| SemanticBackend::Ner(Mutex::new(n))),
            keywords_enabled,
            respect_code_fences: true,
            min_confidence: 0.5,
            agreement_bonus: 0.15,
        }
    }

    /// Create a hybrid scanner with CRF as the semantic backend.
    pub fn with_crf(keywords_enabled: bool, crf_scanner: Option<CrfScanner>) -> Self {
        Self {
            regex_scanner: PiiScanner::new(),
            keyword_dict: KeywordDict::new(),
            semantic: crf_scanner.map(SemanticBackend::Crf),
            keywords_enabled,
            respect_code_fences: true,
            min_confidence: 0.5,
            agreement_bonus: 0.15,
        }
    }

    /// Create a regex-only hybrid scanner (no keywords, no semantic backend).
    pub fn regex_only() -> Self {
        Self {
            regex_scanner: PiiScanner::new(),
            keyword_dict: KeywordDict::new(),
            semantic: None,
            keywords_enabled: false,
            respect_code_fences: true,
            min_confidence: 0.5,
            agreement_bonus: 0.15,
        }
    }

    /// Set confidence voting parameters.
    pub fn set_confidence_params(&mut self, min_confidence: f32, agreement_bonus: f32) {
        self.min_confidence = min_confidence;
        self.agreement_bonus = agreement_bonus;
    }

    /// Set whether to respect markdown code fences (skip scanning inside them).
    pub fn set_respect_code_fences(&mut self, respect: bool) {
        self.respect_code_fences = respect;
    }

    /// Scan text through all enabled scanners, resolve overlaps via confidence voting.
    pub fn scan_text(&self, text: &str) -> Vec<PiiMatch> {
        let effective_text = if self.respect_code_fences {
            mask_code_fences(text)
        } else {
            text.to_string()
        };

        // Phase 1: Collect all matches from all scanners with source tags
        let mut tagged: Vec<TaggedMatch> = Vec::new();

        // 1a. Regex (deterministic, confidence = 1.0)
        for m in self.regex_scanner.scan_text(&effective_text) {
            tagged.push(TaggedMatch {
                pii_match: m,
                source: ScannerSource::Regex,
            });
        }

        // 1b. Keyword dictionary (if enabled)
        if self.keywords_enabled {
            for m in self.keyword_dict.scan_text(&effective_text) {
                tagged.push(TaggedMatch {
                    pii_match: m,
                    source: ScannerSource::Keyword,
                });
            }
        }

        // 1c. Semantic backend (NER or CRF, if loaded)
        if let Some(ref backend) = self.semantic {
            let semantic_matches = match backend {
                SemanticBackend::Ner(ner_mutex) => {
                    if let Ok(mut ner) = ner_mutex.lock() {
                        match ner.scan_text(&effective_text) {
                            Ok(matches) => matches,
                            Err(e) => {
                                oo_warn!(crate::oo_log::modules::HYBRID, "NER inference failed, skipping", error = %e);
                                Vec::new()
                            }
                        }
                    } else {
                        Vec::new()
                    }
                }
                SemanticBackend::Crf(crf) => crf.scan_text(&effective_text),
            };
            for m in semantic_matches {
                tagged.push(TaggedMatch {
                    pii_match: m,
                    source: ScannerSource::Semantic,
                });
            }
        }

        // Phase 2: Cluster overlapping spans
        let clusters = cluster_overlapping(&tagged);

        // Phase 3: Vote within each cluster
        let mut results: Vec<PiiMatch> = Vec::new();
        for cluster in clusters {
            let mut winners = resolve_cluster(&cluster, &tagged, self.agreement_bonus);
            // Filter by min confidence
            winners.retain(|m| m.confidence >= self.min_confidence);
            results.extend(winners);
        }

        // Restore raw_value from original text (masking replaced chars with spaces)
        if self.respect_code_fences {
            for m in &mut results {
                if m.start < text.len() && m.end <= text.len() {
                    m.raw_value = text[m.start..m.end].to_string();
                }
            }
        }

        // Sort by start offset
        results.sort_by_key(|m| (m.start, std::cmp::Reverse(m.end)));
        results
    }

    /// Scan a JSON body, traversing string values and tracking JSON paths.
    pub fn scan_json(&self, json: &Value, skip_fields: &[String]) -> Vec<PiiMatch> {
        let mut matches = Vec::new();
        self.scan_json_inner(json, "", skip_fields, &mut matches, 0);
        matches
    }

    fn scan_json_inner(
        &self,
        value: &Value,
        path: &str,
        skip_fields: &[String],
        matches: &mut Vec<PiiMatch>,
        depth: usize,
    ) {
        match value {
            Value::String(s) => {
                // 1. Scan the raw string text (original behavior)
                let mut text_matches = self.scan_text(s);
                for m in &mut text_matches {
                    m.json_path = Some(path.to_string());
                }
                matches.extend(text_matches);

                // 2. If string looks like JSON, try to parse and scan recursively
                if depth < MAX_NESTED_JSON_DEPTH {
                    let trimmed = s.trim();
                    let looks_like_json = (trimmed.starts_with('{') && trimmed.ends_with('}'))
                        || (trimmed.starts_with('[') && trimmed.ends_with(']'));

                    if looks_like_json {
                        if let Ok(inner) = serde_json::from_str::<Value>(trimmed) {
                            let mut inner_matches = Vec::new();
                            self.scan_json_inner(
                                &inner,
                                "",
                                skip_fields,
                                &mut inner_matches,
                                depth + 1,
                            );

                            // Remap inner matches to byte offsets in the outer string
                            for inner_m in inner_matches {
                                // Find the raw_value in the outer string to get correct offsets
                                if let Some(pos) = s.find(&inner_m.raw_value) {
                                    // Only add if not already found by the raw text scan
                                    if !overlaps_any(matches, pos, pos + inner_m.raw_value.len()) {
                                        matches.push(PiiMatch {
                                            pii_type: inner_m.pii_type,
                                            start: pos,
                                            end: pos + inner_m.raw_value.len(),
                                            raw_value: inner_m.raw_value,
                                            json_path: Some(path.to_string()),
                                            confidence: inner_m.confidence,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Value::Object(map) => {
                for (key, val) in map {
                    if skip_fields.contains(key) {
                        continue;
                    }
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", path, key)
                    };
                    self.scan_json_inner(val, &child_path, skip_fields, matches, depth);
                }
            }
            Value::Array(arr) => {
                for (i, val) in arr.iter().enumerate() {
                    let child_path = format!("{}[{}]", path, i);
                    self.scan_json_inner(val, &child_path, skip_fields, matches, depth);
                }
            }
            _ => {}
        }
    }

    pub fn keywords_enabled(&self) -> bool {
        self.keywords_enabled
    }

    pub fn ner_enabled(&self) -> bool {
        matches!(self.semantic, Some(SemanticBackend::Ner(_)))
    }

    pub fn crf_enabled(&self) -> bool {
        matches!(self.semantic, Some(SemanticBackend::Crf(_)))
    }

    pub fn semantic_backend_name(&self) -> &'static str {
        match &self.semantic {
            Some(SemanticBackend::Ner(_)) => "ner",
            Some(SemanticBackend::Crf(_)) => "crf",
            None => "none",
        }
    }
}

/// Group overlapping spans into clusters using union-find.
/// Returns Vec of clusters, where each cluster is a Vec of indices into `tagged`.
fn cluster_overlapping(tagged: &[TaggedMatch]) -> Vec<Vec<usize>> {
    let n = tagged.len();
    if n == 0 {
        return Vec::new();
    }

    // Union-find with path compression
    let mut parent: Vec<usize> = (0..n).collect();

    fn find(parent: &mut [usize], i: usize) -> usize {
        if parent[i] != i {
            parent[i] = find(parent, parent[i]);
        }
        parent[i]
    }

    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[rb] = ra;
        }
    }

    // Merge overlapping spans
    for i in 0..n {
        for j in (i + 1)..n {
            let a = &tagged[i].pii_match;
            let b = &tagged[j].pii_match;
            if a.start < b.end && b.start < a.end {
                union(&mut parent, i, j);
            }
        }
    }

    // Collect clusters
    let mut clusters: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        clusters.entry(root).or_default().push(i);
    }

    clusters.into_values().collect()
}

/// Resolve a cluster of overlapping matches via confidence voting.
///
/// 1. Group matches by pii_type
/// 2. For each type, find the highest confidence match
/// 3. If ≥2 distinct scanner sources detected that type, add agreement_bonus (cap 1.0)
/// 4. The type with the highest adjusted confidence wins
/// 5. Return the winning match
fn resolve_cluster(
    cluster: &[usize],
    tagged: &[TaggedMatch],
    agreement_bonus: f32,
) -> Vec<PiiMatch> {
    if cluster.len() == 1 {
        return vec![tagged[cluster[0]].pii_match.clone()];
    }

    // Group by pii_type: (best_match_index, best_raw_confidence, set of sources)
    let mut type_groups: HashMap<PiiType, (usize, f32, HashSet<ScannerSource>)> = HashMap::new();

    for &idx in cluster {
        let tm = &tagged[idx];
        let entry = type_groups
            .entry(tm.pii_match.pii_type)
            .or_insert((idx, 0.0, HashSet::new()));
        entry.2.insert(tm.source);
        if tm.pii_match.confidence > entry.1 {
            entry.0 = idx;
            entry.1 = tm.pii_match.confidence;
        }
    }

    // Find the winning type (highest adjusted confidence)
    let mut best_idx = 0;
    let mut best_score = f32::NEG_INFINITY;

    for (_, (match_idx, confidence, sources)) in &type_groups {
        let adjusted = if sources.len() >= 2 {
            (*confidence + agreement_bonus).min(1.0)
        } else {
            *confidence
        };
        if adjusted > best_score {
            best_score = adjusted;
            best_idx = *match_idx;
        }
    }

    // Build winner with adjusted confidence
    let mut winner = tagged[best_idx].pii_match.clone();
    if let Some((_, _, sources)) = type_groups.get(&winner.pii_type) {
        if sources.len() >= 2 {
            winner.confidence = (winner.confidence + agreement_bonus).min(1.0);
        }
    }

    vec![winner]
}

/// Check if a span [start, end) overlaps with any existing match.
fn overlaps_any(matches: &[PiiMatch], start: usize, end: usize) -> bool {
    matches.iter().any(|m| start < m.end && end > m.start)
}

/// Mask content inside markdown code fences and inline code backticks.
/// Replaces fenced/inline code content with spaces to preserve byte offsets.
fn mask_code_fences(text: &str) -> String {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut masked = text.to_string();
    // Safety: we only replace ASCII characters with spaces (same byte length)
    let out = unsafe { masked.as_bytes_mut() };

    let mut i = 0;

    while i < len {
        // Check for fenced code block (``` or ~~~ at start of potential fence)
        if i + 2 < len && bytes[i] == b'`' && bytes[i + 1] == b'`' && bytes[i + 2] == b'`' {
            // Find the end of the opening fence line (skip the ``` and any language tag)
            let fence_start = i;
            i += 3;
            // Skip rest of opening fence line
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            if i < len {
                i += 1; // skip the newline
            }
            // Mask content until closing ```
            let content_start = i;
            let mut found_close = false;
            while i + 2 < len {
                if bytes[i] == b'`' && bytes[i + 1] == b'`' && bytes[i + 2] == b'`' {
                    // Mask from content_start to i (the closing fence)
                    for j in content_start..i {
                        if out[j] != b'\n' {
                            out[j] = b' ';
                        }
                    }
                    // Also mask the closing fence itself
                    i += 3;
                    while i < len && bytes[i] != b'\n' {
                        i += 1;
                    }
                    // Mask the opening fence markers too
                    for j in fence_start..content_start {
                        if out[j] != b'\n' {
                            out[j] = b' ';
                        }
                    }
                    found_close = true;
                    break;
                }
                i += 1;
            }
            if !found_close {
                // Unclosed fence — don't mask anything, revert to after opening ```
                i = fence_start + 3;
            }
        }
        // Check for inline code (single backtick)
        else if bytes[i] == b'`' {
            let start = i;
            i += 1;
            // Find closing backtick
            let mut found_close = false;
            while i < len {
                if bytes[i] == b'`' {
                    // Mask content between backticks (not the backticks themselves, for simplicity mask all)
                    for j in start..=i {
                        if out[j] != b'\n' {
                            out[j] = b' ';
                        }
                    }
                    i += 1;
                    found_close = true;
                    break;
                }
                if bytes[i] == b'\n' {
                    // Inline code doesn't cross newlines
                    break;
                }
                i += 1;
            }
            if !found_close {
                i = start + 1; // Not a code span, move past the backtick
            }
        } else {
            i += 1;
        }
    }

    masked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pii_types::PiiType;

    #[test]
    fn test_regex_only_still_works() {
        let scanner = HybridScanner::regex_only();
        let matches = scanner.scan_text("My SSN is 123-45-6789");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::Ssn);
    }

    #[test]
    fn test_keywords_added() {
        let scanner = HybridScanner::new(true, None);
        let matches = scanner.scan_text("I have diabetes and take metformin");
        assert!(matches.iter().any(|m| m.pii_type == PiiType::HealthKeyword));
    }

    #[test]
    fn test_regex_takes_priority_over_keywords() {
        let scanner = HybridScanner::new(true, None);
        let matches = scanner.scan_text("My SSN is 123-45-6789 and I have diabetes");
        let ssn = matches.iter().find(|m| m.pii_type == PiiType::Ssn);
        let health = matches
            .iter()
            .find(|m| m.pii_type == PiiType::HealthKeyword);
        assert!(ssn.is_some(), "SSN should be detected by regex");
        assert!(health.is_some(), "Diabetes should be detected by keywords");
    }

    #[test]
    fn test_no_duplicate_spans() {
        let scanner = HybridScanner::new(true, None);
        let matches = scanner.scan_text("I have diabetes");
        let diabetes_matches: Vec<_> = matches
            .iter()
            .filter(|m| m.raw_value.to_lowercase() == "diabetes")
            .collect();
        assert_eq!(diabetes_matches.len(), 1);
    }

    #[test]
    fn test_json_scanning_hybrid() {
        let scanner = HybridScanner::new(true, None);
        let json: serde_json::Value = serde_json::json!({
            "messages": [{
                "role": "user",
                "content": "My SSN is 123-45-6789 and I have diabetes"
            }]
        });
        let skip = vec!["model".to_string()];
        let matches = scanner.scan_json(&json, &skip);
        assert!(matches.iter().any(|m| m.pii_type == PiiType::Ssn));
        assert!(matches.iter().any(|m| m.pii_type == PiiType::HealthKeyword));
        assert!(matches.iter().all(|m| m.json_path.is_some()));
    }

    #[test]
    fn test_skip_fields_hybrid() {
        let scanner = HybridScanner::new(true, None);
        let json: serde_json::Value = serde_json::json!({
            "model": "I have diabetes",
            "content": "I have diabetes"
        });
        let skip = vec!["model".to_string()];
        let matches = scanner.scan_json(&json, &skip);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].json_path.as_deref(), Some("content"));
    }

    #[test]
    fn test_mixed_fpe_and_redaction_types() {
        let scanner = HybridScanner::new(true, None);
        let matches = scanner.scan_text("Call me at (555) 123-4567, I have asthma");
        let phone = matches.iter().find(|m| m.pii_type == PiiType::PhoneNumber);
        let health = matches
            .iter()
            .find(|m| m.pii_type == PiiType::HealthKeyword);
        assert!(phone.is_some(), "Phone should be detected");
        assert!(health.is_some(), "Health keyword should be detected");
        assert!(phone.unwrap().pii_type.is_fpe_eligible());
        assert!(!health.unwrap().pii_type.is_fpe_eligible());
    }

    #[test]
    fn test_clean_text_no_matches() {
        let scanner = HybridScanner::new(true, None);
        let matches = scanner.scan_text("The weather is nice today.");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_child_keywords_detected() {
        let scanner = HybridScanner::new(true, None);
        let matches = scanner.scan_text("My daughter goes to kindergarten");
        assert!(matches.iter().any(|m| m.pii_type == PiiType::ChildKeyword));
    }

    #[test]
    fn test_semantic_backend_name() {
        let scanner = HybridScanner::regex_only();
        assert_eq!(scanner.semantic_backend_name(), "none");
        assert!(!scanner.ner_enabled());
        assert!(!scanner.crf_enabled());

        let scanner = HybridScanner::new(true, None);
        assert_eq!(scanner.semantic_backend_name(), "none");

        let scanner = HybridScanner::with_crf(true, None);
        assert_eq!(scanner.semantic_backend_name(), "none");
    }

    // ── Nested JSON scanning tests ──

    #[test]
    fn test_nested_json_string_scanned() {
        let scanner = HybridScanner::regex_only();
        let json: Value = serde_json::json!({
            "content": "{\"data\": \"My SSN is 123-45-6789\"}"
        });
        let matches = scanner.scan_json(&json, &[]);
        assert!(
            matches.iter().any(|m| m.pii_type == PiiType::Ssn),
            "SSN inside nested JSON string should be detected, got: {:?}",
            matches
        );
    }

    #[test]
    fn test_nested_json_depth_limit() {
        let scanner = HybridScanner::regex_only();
        // 3 levels of nesting — innermost should NOT be scanned (depth limit is 2)
        let inner = serde_json::json!({"ssn": "123-45-6789"}).to_string();
        let middle = serde_json::json!({"nested": inner}).to_string();
        let json: Value = serde_json::json!({"content": middle});

        let matches = scanner.scan_json(&json, &[]);
        // The SSN is 3 levels deep: content (string) → nested (string) → ssn (string)
        // With max depth=2, the middle string is parsed (depth 0→1), but the inner
        // string within it would need depth 2→3 which exceeds the limit.
        // However, the raw text scan at each level may still find it.
        // The key test: it should not panic or infinite-loop.
        assert!(matches.len() <= 2, "Should not produce excessive matches");
    }

    #[test]
    fn test_nested_json_non_json_ignored() {
        let scanner = HybridScanner::regex_only();
        let json: Value = serde_json::json!({
            "content": "{not valid json at all"
        });
        // Should not panic or error
        let matches = scanner.scan_json(&json, &[]);
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_nested_json_multiple_pii() {
        let scanner = HybridScanner::regex_only();
        let json: Value = serde_json::json!({
            "content": "{\"info\": \"Card 4532015112830366 and email test@example.com\"}"
        });
        let matches = scanner.scan_json(&json, &[]);
        let has_cc = matches.iter().any(|m| m.pii_type == PiiType::CreditCard);
        let has_email = matches.iter().any(|m| m.pii_type == PiiType::Email);
        assert!(has_cc, "Credit card in nested JSON should be detected");
        assert!(has_email, "Email in nested JSON should be detected");
    }

    // ── Code fence detection tests ──

    #[test]
    fn test_code_fence_blocks_scanning() {
        let scanner = HybridScanner::regex_only();
        let text = "Here is code:\n```\nTest SSN: 123-45-6789\n```\nEnd.";
        let matches = scanner.scan_text(text);
        assert!(
            !matches.iter().any(|m| m.pii_type == PiiType::Ssn),
            "SSN inside code fence should NOT be detected, got: {:?}",
            matches
        );
    }

    #[test]
    fn test_inline_code_blocks_scanning() {
        let scanner = HybridScanner::regex_only();
        let text = "Use this SSN: `123-45-6789` in the test.";
        let matches = scanner.scan_text(text);
        assert!(
            !matches.iter().any(|m| m.pii_type == PiiType::Ssn),
            "SSN inside inline code should NOT be detected, got: {:?}",
            matches
        );
    }

    #[test]
    fn test_pii_outside_fence_still_detected() {
        let scanner = HybridScanner::regex_only();
        let text = "My SSN is 123-45-6789\n```\nsome code here\n```\nEnd.";
        let matches = scanner.scan_text(text);
        assert!(
            matches.iter().any(|m| m.pii_type == PiiType::Ssn),
            "SSN outside code fence should be detected"
        );
    }

    #[test]
    fn test_unclosed_fence_scans_normally() {
        let scanner = HybridScanner::regex_only();
        let text = "```\nunclosed fence\nMy SSN is 123-45-6789";
        let matches = scanner.scan_text(text);
        // Unclosed fence → no masking, SSN should be found
        assert!(
            matches.iter().any(|m| m.pii_type == PiiType::Ssn),
            "SSN after unclosed fence should still be detected"
        );
    }

    #[test]
    fn test_code_fence_disabled() {
        let mut scanner = HybridScanner::regex_only();
        scanner.set_respect_code_fences(false);
        let text = "Here:\n```\nSSN: 123-45-6789\n```\nEnd.";
        let matches = scanner.scan_text(text);
        assert!(
            matches.iter().any(|m| m.pii_type == PiiType::Ssn),
            "SSN inside fence should be detected when fence respect is disabled"
        );
    }

    // ── Code fence masking unit tests ──

    #[test]
    fn test_mask_code_fences_fenced_block() {
        let text = "before\n```\nsecret 123\n```\nafter";
        let masked = mask_code_fences(text);
        assert!(!masked.contains("secret"));
        assert!(masked.contains("before"));
        assert!(masked.contains("after"));
        assert_eq!(masked.len(), text.len());
    }

    #[test]
    fn test_mask_code_fences_inline() {
        let text = "use `secret123` here";
        let masked = mask_code_fences(text);
        assert!(!masked.contains("secret123"));
        assert!(masked.contains("use "));
        assert!(masked.contains(" here"));
        assert_eq!(masked.len(), text.len());
    }

    #[test]
    fn test_mask_code_fences_preserves_length() {
        let text = "a\n```python\nline1\nline2\n```\nb";
        let masked = mask_code_fences(text);
        assert_eq!(
            masked.len(),
            text.len(),
            "Masking must preserve byte length"
        );
    }

    // ── Confidence voting tests ──

    #[test]
    fn test_confidence_field_propagated() {
        let scanner = HybridScanner::regex_only();
        let matches = scanner.scan_text("My SSN is 123-45-6789");
        assert_eq!(matches.len(), 1);
        assert!(matches[0].confidence > 0.0, "confidence should be set");
    }

    #[test]
    fn test_regex_confidence_always_one() {
        let scanner = HybridScanner::regex_only();
        let matches =
            scanner.scan_text("SSN 123-45-6789, email test@example.com, card 4532015112830366");
        assert!(matches.len() >= 3);
        for m in &matches {
            assert_eq!(
                m.confidence, 1.0,
                "{:?} should have confidence 1.0",
                m.pii_type
            );
        }
    }

    #[test]
    fn test_keyword_confidence_always_one() {
        let scanner = HybridScanner::new(true, None);
        let matches = scanner.scan_text("I have diabetes and asthma");
        let health: Vec<_> = matches
            .iter()
            .filter(|m| m.pii_type == PiiType::HealthKeyword)
            .collect();
        assert!(health.len() >= 2);
        for m in &health {
            assert_eq!(
                m.confidence, 1.0,
                "keyword match should have confidence 1.0"
            );
        }
    }

    #[test]
    fn test_non_overlapping_matches_unchanged() {
        let scanner = HybridScanner::new(true, None);
        let matches = scanner.scan_text("SSN 123-45-6789 and I have diabetes");
        let ssn = matches.iter().find(|m| m.pii_type == PiiType::Ssn);
        let health = matches
            .iter()
            .find(|m| m.pii_type == PiiType::HealthKeyword);
        assert!(ssn.is_some(), "SSN should survive voting");
        assert!(health.is_some(), "Health keyword should survive voting");
    }

    #[test]
    fn test_min_confidence_filters() {
        // Set min_confidence very high so only perfect matches pass
        let mut scanner = HybridScanner::regex_only();
        scanner.set_confidence_params(1.1, 0.15); // nothing passes 1.1
        let matches = scanner.scan_text("SSN 123-45-6789");
        assert_eq!(
            matches.len(),
            0,
            "All matches should be filtered by min_confidence > 1.0"
        );
    }

    #[test]
    fn test_min_confidence_default_passes_regex() {
        // Default min_confidence 0.5 should pass regex (confidence 1.0)
        let scanner = HybridScanner::regex_only();
        let matches = scanner.scan_text("SSN 123-45-6789");
        assert_eq!(
            matches.len(),
            1,
            "Regex match at 1.0 should pass default 0.5 threshold"
        );
    }

    #[test]
    fn test_cluster_overlapping_no_overlap() {
        let tagged = vec![
            TaggedMatch {
                pii_match: PiiMatch {
                    pii_type: PiiType::Ssn,
                    start: 0,
                    end: 11,
                    raw_value: "123-45-6789".into(),
                    json_path: None,
                    confidence: 1.0,
                },
                source: ScannerSource::Regex,
            },
            TaggedMatch {
                pii_match: PiiMatch {
                    pii_type: PiiType::Email,
                    start: 20,
                    end: 36,
                    raw_value: "test@example.com".into(),
                    json_path: None,
                    confidence: 1.0,
                },
                source: ScannerSource::Regex,
            },
        ];
        let clusters = cluster_overlapping(&tagged);
        assert_eq!(
            clusters.len(),
            2,
            "Non-overlapping spans should be separate clusters"
        );
    }

    #[test]
    fn test_cluster_overlapping_with_overlap() {
        let tagged = vec![
            TaggedMatch {
                pii_match: PiiMatch {
                    pii_type: PiiType::Ssn,
                    start: 0,
                    end: 11,
                    raw_value: "123-45-6789".into(),
                    json_path: None,
                    confidence: 1.0,
                },
                source: ScannerSource::Regex,
            },
            TaggedMatch {
                pii_match: PiiMatch {
                    pii_type: PiiType::Person,
                    start: 0,
                    end: 11,
                    raw_value: "123-45-6789".into(),
                    json_path: None,
                    confidence: 0.6,
                },
                source: ScannerSource::Semantic,
            },
        ];
        let clusters = cluster_overlapping(&tagged);
        assert_eq!(
            clusters.len(),
            1,
            "Overlapping spans should merge into one cluster"
        );
        assert_eq!(clusters[0].len(), 2);
    }

    #[test]
    fn test_resolve_cluster_single_match() {
        let tagged = vec![TaggedMatch {
            pii_match: PiiMatch {
                pii_type: PiiType::Ssn,
                start: 0,
                end: 11,
                raw_value: "123-45-6789".into(),
                json_path: None,
                confidence: 1.0,
            },
            source: ScannerSource::Regex,
        }];
        let winners = resolve_cluster(&[0], &tagged, 0.15);
        assert_eq!(winners.len(), 1);
        assert_eq!(winners[0].pii_type, PiiType::Ssn);
        assert_eq!(winners[0].confidence, 1.0); // no bonus for single source
    }

    #[test]
    fn test_resolve_cluster_higher_confidence_wins() {
        let tagged = vec![
            TaggedMatch {
                pii_match: PiiMatch {
                    pii_type: PiiType::Ssn,
                    start: 0,
                    end: 11,
                    raw_value: "123-45-6789".into(),
                    json_path: None,
                    confidence: 1.0,
                },
                source: ScannerSource::Regex,
            },
            TaggedMatch {
                pii_match: PiiMatch {
                    pii_type: PiiType::Person,
                    start: 0,
                    end: 11,
                    raw_value: "123-45-6789".into(),
                    json_path: None,
                    confidence: 0.6,
                },
                source: ScannerSource::Semantic,
            },
        ];
        let winners = resolve_cluster(&[0, 1], &tagged, 0.15);
        assert_eq!(winners.len(), 1);
        assert_eq!(
            winners[0].pii_type,
            PiiType::Ssn,
            "Higher confidence type should win"
        );
    }

    #[test]
    fn test_agreement_bonus_applied() {
        // Two scanners agree on SSN type → agreement bonus applied
        let tagged = vec![
            TaggedMatch {
                pii_match: PiiMatch {
                    pii_type: PiiType::Ssn,
                    start: 0,
                    end: 11,
                    raw_value: "123-45-6789".into(),
                    json_path: None,
                    confidence: 0.8,
                },
                source: ScannerSource::Regex,
            },
            TaggedMatch {
                pii_match: PiiMatch {
                    pii_type: PiiType::Ssn,
                    start: 0,
                    end: 11,
                    raw_value: "123-45-6789".into(),
                    json_path: None,
                    confidence: 0.7,
                },
                source: ScannerSource::Semantic,
            },
        ];
        let winners = resolve_cluster(&[0, 1], &tagged, 0.15);
        assert_eq!(winners.len(), 1);
        assert_eq!(winners[0].pii_type, PiiType::Ssn);
        // Best raw = 0.8, adjusted = 0.8 + 0.15 = 0.95
        assert!(
            (winners[0].confidence - 0.95).abs() < 0.001,
            "Agreement bonus should boost to 0.95, got {}",
            winners[0].confidence
        );
    }

    #[test]
    fn test_agreement_bonus_capped_at_one() {
        let tagged = vec![
            TaggedMatch {
                pii_match: PiiMatch {
                    pii_type: PiiType::Ssn,
                    start: 0,
                    end: 11,
                    raw_value: "123-45-6789".into(),
                    json_path: None,
                    confidence: 1.0,
                },
                source: ScannerSource::Regex,
            },
            TaggedMatch {
                pii_match: PiiMatch {
                    pii_type: PiiType::Ssn,
                    start: 0,
                    end: 11,
                    raw_value: "123-45-6789".into(),
                    json_path: None,
                    confidence: 0.9,
                },
                source: ScannerSource::Keyword,
            },
        ];
        let winners = resolve_cluster(&[0, 1], &tagged, 0.15);
        assert_eq!(
            winners[0].confidence, 1.0,
            "Agreement bonus should cap at 1.0"
        );
    }

    #[test]
    fn test_voting_preserves_code_fence_masking() {
        let scanner = HybridScanner::new(true, None);
        let text = "I have diabetes\n```\nSSN: 123-45-6789\n```\nand asthma";
        let matches = scanner.scan_text(text);
        // SSN inside fence should be masked
        assert!(
            !matches.iter().any(|m| m.pii_type == PiiType::Ssn),
            "SSN inside code fence should not be detected with voting"
        );
        // Keywords outside fence should still be found
        assert!(
            matches.iter().any(|m| m.pii_type == PiiType::HealthKeyword),
            "Health keywords outside fence should survive voting"
        );
    }

    #[test]
    fn test_confidence_in_json_scanning() {
        let scanner = HybridScanner::new(true, None);
        let json: serde_json::Value = serde_json::json!({
            "content": "My SSN is 123-45-6789"
        });
        let matches = scanner.scan_json(&json, &[]);
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0].confidence, 1.0,
            "JSON-scanned match should have confidence"
        );
    }

    #[test]
    fn test_cluster_overlapping_transitive() {
        // A overlaps B, B overlaps C, but A doesn't overlap C → all in one cluster
        let tagged = vec![
            TaggedMatch {
                pii_match: PiiMatch {
                    pii_type: PiiType::Ssn,
                    start: 0,
                    end: 10,
                    raw_value: "0123456789".into(),
                    json_path: None,
                    confidence: 1.0,
                },
                source: ScannerSource::Regex,
            },
            TaggedMatch {
                pii_match: PiiMatch {
                    pii_type: PiiType::Person,
                    start: 5,
                    end: 15,
                    raw_value: "5678901234".into(),
                    json_path: None,
                    confidence: 0.6,
                },
                source: ScannerSource::Semantic,
            },
            TaggedMatch {
                pii_match: PiiMatch {
                    pii_type: PiiType::PhoneNumber,
                    start: 12,
                    end: 20,
                    raw_value: "23456789".into(),
                    json_path: None,
                    confidence: 0.8,
                },
                source: ScannerSource::Regex,
            },
        ];
        let clusters = cluster_overlapping(&tagged);
        assert_eq!(
            clusters.len(),
            1,
            "Transitive overlaps should merge into one cluster"
        );
        assert_eq!(clusters[0].len(), 3);
    }
}
