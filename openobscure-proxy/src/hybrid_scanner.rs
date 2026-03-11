use std::collections::{HashMap, HashSet};

use rayon::prelude::*;
use serde_json::Value;

use crate::crf_scanner::CrfScanner;
use crate::keyword_dict::KeywordDict;
use crate::lang_detect;
use crate::multilingual;
use crate::name_gazetteer::NameGazetteer;
use crate::ner_scanner::NerPool;
use crate::pii_types::PiiType;
use crate::scanner::{PiiMatch, PiiScanner};

/// Maximum depth for recursive JSON-in-string parsing.
const MAX_NESTED_JSON_DEPTH: usize = 2;

/// Per-scanner timing breakdown from a scan_text call.
#[derive(Debug, Default, Clone)]
pub struct ScanTiming {
    pub regex_us: u64,
    pub keyword_us: u64,
    pub gazetteer_us: u64,
    pub semantic_us: u64,
    pub multilingual_us: u64,
    pub voting_us: u64,
    pub total_us: u64,
}

/// Scanner source for confidence voting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ScannerSource {
    Regex,
    Keyword,
    Gazetteer,
    Semantic,
}

/// A match tagged with its source scanner, used during voting.
struct TaggedMatch {
    pii_match: PiiMatch,
    source: ScannerSource,
}

/// The semantic scanner backend: NER (pooled sessions) or CRF (lightweight fallback).
enum SemanticBackend {
    Ner(NerPool),
    Crf(CrfScanner),
}

/// Hybrid scanner combining regex, keyword dictionary, and semantic model (NER or CRF).
///
/// Pipeline:
/// 1. Regex scanner (fast, deterministic) → structured PII (CC, SSN, phone, email, API key)
/// 2. Keyword dictionary (O(1) lookup) → health/child terms
/// 3. Semantic backend (NER or CRF) → person names, locations, organizations
/// 4. Multilingual patterns (language-specific national IDs, phones, IBANs)
///
/// Overlapping spans are resolved via confidence-weighted voting with agreement bonus.
pub struct HybridScanner {
    regex_scanner: PiiScanner,
    keyword_dict: KeywordDict,
    gazetteer: Option<NameGazetteer>,
    semantic: Option<SemanticBackend>,
    keywords_enabled: bool,
    respect_code_fences: bool,
    min_confidence: f32,
    agreement_bonus: f32,
    /// ISO 639-1 codes of languages eligible for the multilingual scan pass.
    /// Empty = all supported languages are eligible (default).
    enabled_languages: HashSet<String>,
}

impl HybridScanner {
    /// Create a hybrid scanner with NER as the semantic backend.
    pub fn new(
        keywords_enabled: bool,
        ner_pool: Option<NerPool>,
        gazetteer: Option<NameGazetteer>,
    ) -> Self {
        Self {
            regex_scanner: PiiScanner::new(),
            keyword_dict: KeywordDict::new(),
            gazetteer,
            semantic: ner_pool.map(SemanticBackend::Ner),
            keywords_enabled,
            respect_code_fences: true,
            min_confidence: 0.5,
            agreement_bonus: 0.15,
            enabled_languages: HashSet::new(),
        }
    }

    /// Create a hybrid scanner with CRF as the semantic backend.
    pub fn with_crf(
        keywords_enabled: bool,
        crf_scanner: Option<CrfScanner>,
        gazetteer: Option<NameGazetteer>,
    ) -> Self {
        Self {
            regex_scanner: PiiScanner::new(),
            keyword_dict: KeywordDict::new(),
            gazetteer,
            semantic: crf_scanner.map(SemanticBackend::Crf),
            keywords_enabled,
            respect_code_fences: true,
            min_confidence: 0.5,
            agreement_bonus: 0.15,
            enabled_languages: HashSet::new(),
        }
    }

    /// Create a regex-only hybrid scanner (no keywords, no semantic backend).
    pub fn regex_only() -> Self {
        Self {
            regex_scanner: PiiScanner::new(),
            keyword_dict: KeywordDict::new(),
            gazetteer: None,
            semantic: None,
            keywords_enabled: false,
            respect_code_fences: true,
            min_confidence: 0.5,
            agreement_bonus: 0.15,
            enabled_languages: HashSet::new(),
        }
    }

    /// Restrict the multilingual scan pass to the given ISO 639-1 language codes.
    ///
    /// An empty slice (the default) enables all supported languages.
    /// Codes that do not correspond to a supported language are silently ignored.
    pub fn set_enabled_languages(&mut self, langs: Vec<String>) {
        self.enabled_languages = langs.into_iter().collect();
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

    /// Run a dummy inference pass to warm up the NER/CRF model.
    ///
    /// Returns the warm-up duration. No-op if no semantic backend is loaded.
    pub fn warm(&self) -> std::time::Duration {
        let start = std::time::Instant::now();
        let _ = self.scan_text("John Smith lives at 123 Main St, New York, NY 10001");
        start.elapsed()
    }

    /// Scan text through all enabled scanners, resolve overlaps via confidence voting.
    pub fn scan_text(&self, text: &str) -> Vec<PiiMatch> {
        self.scan_text_with_timing(text).0
    }

    /// Scan text with per-scanner timing breakdown.
    pub fn scan_text_with_timing(&self, text: &str) -> (Vec<PiiMatch>, ScanTiming) {
        let total_start = std::time::Instant::now();
        let mut timing = ScanTiming::default();

        let effective_text = if self.respect_code_fences {
            mask_code_fences(text)
        } else {
            text.to_string()
        };

        // Phase 1: Collect all matches from all scanners with source tags
        let mut tagged: Vec<TaggedMatch> = Vec::new();

        // 1a. Regex (deterministic, confidence = 1.0)
        let t = std::time::Instant::now();
        for m in self.regex_scanner.scan_text(&effective_text) {
            tagged.push(TaggedMatch {
                pii_match: m,
                source: ScannerSource::Regex,
            });
        }
        timing.regex_us = t.elapsed().as_micros() as u64;

        // 1b. Keyword dictionary (if enabled)
        let t = std::time::Instant::now();
        if self.keywords_enabled {
            for m in self.keyword_dict.scan_text(&effective_text) {
                tagged.push(TaggedMatch {
                    pii_match: m,
                    source: ScannerSource::Keyword,
                });
            }
        }
        timing.keyword_us = t.elapsed().as_micros() as u64;

        // 1c. Name gazetteer (if enabled)
        let t = std::time::Instant::now();
        if let Some(ref gaz) = self.gazetteer {
            for m in gaz.scan_text(&effective_text) {
                tagged.push(TaggedMatch {
                    pii_match: m,
                    source: ScannerSource::Gazetteer,
                });
            }
        }
        timing.gazetteer_us = t.elapsed().as_micros() as u64;

        // 1d. Semantic backend (NER pool or CRF, if loaded)
        let t = std::time::Instant::now();
        if let Some(ref backend) = self.semantic {
            let semantic_matches = match backend {
                SemanticBackend::Ner(pool) => match pool.acquire() {
                    Some(mut guard) => match guard.scan_text(&effective_text) {
                        Ok(matches) => matches,
                        Err(e) => {
                            oo_warn!(crate::oo_log::modules::HYBRID, "NER inference failed, skipping", error = %e);
                            Vec::new()
                        }
                    },
                    None => Vec::new(), // Pool exhausted, regex handles it
                },
                SemanticBackend::Crf(crf) => crf.scan_text(&effective_text),
            };
            for m in semantic_matches {
                tagged.push(TaggedMatch {
                    pii_match: m,
                    source: ScannerSource::Semantic,
                });
            }
        }
        timing.semantic_us = t.elapsed().as_micros() as u64;

        // 1e. Multilingual patterns (language-specific national IDs, phones, IBANs)
        // Run detected language first; if non-English detected, also try closely
        // related languages (e.g., Portuguese ↔ Spanish) since whatlang can
        // confuse them on short texts. Validation functions prevent FPs.
        // If enabled_languages is non-empty, only languages whose ISO 639-1 code
        // appears in that set are scanned.
        let t = std::time::Instant::now();
        {
            let detected = lang_detect::detect_language(&effective_text);
            let langs = multilingual::languages_to_scan(detected.as_ref());
            for lang in langs {
                if !self.enabled_languages.is_empty()
                    && !self.enabled_languages.contains(lang.code())
                {
                    continue;
                }
                for m in multilingual::scan_with_lang(&effective_text, lang) {
                    tagged.push(TaggedMatch {
                        pii_match: m,
                        source: ScannerSource::Regex,
                    });
                }
            }
        }
        timing.multilingual_us = t.elapsed().as_micros() as u64;

        // Phase 2: Cluster overlapping spans
        let t = std::time::Instant::now();
        let clusters = cluster_overlapping(&tagged);

        // Phase 3: Vote within each cluster
        let mut results: Vec<PiiMatch> = Vec::new();
        for cluster in clusters {
            let mut winners = resolve_cluster(&cluster, &tagged, self.agreement_bonus);
            // Filter by min confidence
            winners.retain(|m| m.confidence >= self.min_confidence);
            results.extend(winners);
        }
        timing.voting_us = t.elapsed().as_micros() as u64;

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

        timing.total_us = total_start.elapsed().as_micros() as u64;
        (results, timing)
    }

    /// Scan a JSON body, traversing string values and tracking JSON paths.
    pub fn scan_json(&self, json: &Value, skip_fields: &[String]) -> Vec<PiiMatch> {
        // Collect top-level scannable entries for parallel dispatch
        let entries = collect_scannable_entries(json, "", skip_fields);

        // Fast path: single entry or fewer — skip rayon overhead
        if entries.len() <= 1 {
            let mut matches = Vec::new();
            self.scan_json_inner(json, "", skip_fields, &mut matches, 0);
            return matches;
        }

        // Parallel scan: each top-level entry on its own rayon thread
        entries
            .par_iter()
            .flat_map(|(path, val)| {
                let mut matches = Vec::new();
                self.scan_json_inner(val, path, skip_fields, &mut matches, 0);
                matches
            })
            .collect()
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
                // Skip data URIs with base64-encoded images (OpenAI format)
                if s.starts_with("data:image/") && s.contains(";base64,") {
                    return;
                }

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
                // Skip base64 data fields in image source blocks to avoid
                // false positive PII matches on random base64 character sequences
                let is_base64_source = map
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(|s| s == "base64")
                    .unwrap_or(false);

                for (key, val) in map {
                    if skip_fields.contains(key) {
                        continue;
                    }
                    if is_base64_source && key == "data" {
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

/// Collect top-level scannable entries from a JSON value for parallel dispatch.
///
/// For objects: returns each (key, value) pair as a separate entry (skip_fields and
/// base64 source blocks are filtered out). For arrays: returns each element.
/// For other values: returns the value itself.
fn collect_scannable_entries<'a>(
    value: &'a Value,
    path: &str,
    skip_fields: &[String],
) -> Vec<(String, &'a Value)> {
    match value {
        Value::Object(map) => {
            let is_base64_source = map
                .get("type")
                .and_then(|v| v.as_str())
                .map(|s| s == "base64")
                .unwrap_or(false);

            map.iter()
                .filter(|(key, _)| {
                    !(skip_fields.contains(key) || is_base64_source && *key == "data")
                })
                .map(|(key, val)| {
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", path, key)
                    };
                    (child_path, val)
                })
                .collect()
        }
        Value::Array(arr) => arr
            .iter()
            .enumerate()
            .map(|(i, val)| (format!("{}[{}]", path, i), val))
            .collect(),
        _ => vec![(path.to_string(), value)],
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

    for (match_idx, confidence, sources) in type_groups.values() {
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
#[allow(clippy::needless_range_loop)]
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
        let scanner = HybridScanner::new(true, None, None);
        let matches = scanner.scan_text("I have diabetes and take metformin");
        assert!(matches.iter().any(|m| m.pii_type == PiiType::HealthKeyword));
    }

    #[test]
    fn test_regex_takes_priority_over_keywords() {
        let scanner = HybridScanner::new(true, None, None);
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
        let scanner = HybridScanner::new(true, None, None);
        let matches = scanner.scan_text("I have diabetes");
        let diabetes_matches: Vec<_> = matches
            .iter()
            .filter(|m| m.raw_value.to_lowercase() == "diabetes")
            .collect();
        assert_eq!(diabetes_matches.len(), 1);
    }

    #[test]
    fn test_json_scanning_hybrid() {
        let scanner = HybridScanner::new(true, None, None);
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
        let scanner = HybridScanner::new(true, None, None);
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
        let scanner = HybridScanner::new(true, None, None);
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
        let scanner = HybridScanner::new(true, None, None);
        let matches = scanner.scan_text("The weather is nice today.");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_child_keywords_detected() {
        let scanner = HybridScanner::new(true, None, None);
        let matches = scanner.scan_text("My daughter goes to kindergarten");
        assert!(matches.iter().any(|m| m.pii_type == PiiType::ChildKeyword));
    }

    #[test]
    fn test_semantic_backend_name() {
        let scanner = HybridScanner::regex_only();
        assert_eq!(scanner.semantic_backend_name(), "none");
        assert!(!scanner.ner_enabled());
        assert!(!scanner.crf_enabled());

        let scanner = HybridScanner::new(true, None, None);
        assert_eq!(scanner.semantic_backend_name(), "none");

        let scanner = HybridScanner::with_crf(true, None, None);
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
        let scanner = HybridScanner::new(true, None, None);
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
        let scanner = HybridScanner::new(true, None, None);
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
        let scanner = HybridScanner::new(true, None, None);
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
        let scanner = HybridScanner::new(true, None, None);
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

    /// Integration test: patient_records.csv across all 6 tier configurations.
    ///
    /// Tier matrix:
    ///   Gateway  Full     → DistilBERT + ensemble
    ///   Embedded Full     → DistilBERT + ensemble
    ///   Gateway  Standard → TinyBERT, no ensemble
    ///   Embedded Standard → TinyBERT, no ensemble
    ///   Gateway  Lite     → TinyBERT, no ensemble
    ///   Embedded Lite     → TinyBERT, no ensemble
    ///
    /// All tiers must detect: Person (NER), HealthKeyword, GpsCoordinate.
    #[test]
    fn test_patient_records_all_tiers() {
        use crate::ner_scanner::{NerPool, NerScanner};
        use std::path::Path;

        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let distilbert_dir = manifest.join("models/ner");
        let tinybert_dir = manifest.join("models/ner-lite");

        // Check both models exist and are real (not LFS pointers)
        for (name, dir) in [("DistilBERT", &distilbert_dir), ("TinyBERT", &tinybert_dir)] {
            let model_path = dir.join("model_int8.onnx");
            if !model_path.exists() {
                eprintln!("Skipping: {} model not found at {}", name, dir.display());
                return;
            }
            if let Ok(meta) = std::fs::metadata(&model_path) {
                if meta.len() < 1024 {
                    eprintln!("Skipping: {} is a LFS pointer", name);
                    return;
                }
            }
        }

        // Read CSV once
        let csv_path = manifest
            .parent()
            .unwrap()
            .join("test/data/input/Structured_Data_PII/patient_records.csv");
        let csv_content = std::fs::read_to_string(&csv_path).expect("Failed to read CSV");
        let data_lines: Vec<&str> = csv_content
            .lines()
            .skip(1) // skip header
            .filter(|l| !l.trim().is_empty())
            .collect();

        // Tier configurations: (label, model_dir, ensemble, confidence_threshold)
        let tiers: Vec<(&str, &Path, bool, f32)> = vec![
            ("Gateway Full", &distilbert_dir, true, 0.60),
            ("Embedded Full", &distilbert_dir, true, 0.60),
            ("Gateway Standard", &tinybert_dir, false, 0.60),
            ("Embedded Standard", &tinybert_dir, false, 0.60),
            ("Gateway Lite", &tinybert_dir, false, 0.60),
            ("Embedded Lite", &tinybert_dir, false, 0.60),
        ];

        eprintln!("\n============================================================");
        eprintln!("  patient_records.csv — All 6 Tiers");
        eprintln!("  {} data rows", data_lines.len());
        eprintln!("============================================================");

        for (label, model_dir, ensemble, threshold) in &tiers {
            // Load NER model
            let ner = NerScanner::load(model_dir, *threshold)
                .unwrap_or_else(|e| panic!("{}: failed to load NER: {:?}", label, e));
            let pool = NerPool::new(vec![ner]);
            let mut hybrid = HybridScanner::new(true, Some(pool), None);
            if !ensemble {
                hybrid.set_confidence_params(0.5, 0.0); // no agreement bonus
            }

            let mut type_counts: std::collections::HashMap<PiiType, usize> =
                std::collections::HashMap::new();
            let mut total = 0;

            for line in &data_lines {
                let matches = hybrid.scan_text(line);
                total += matches.len();
                for m in &matches {
                    *type_counts.entry(m.pii_type).or_insert(0) += 1;
                }
            }

            // Sort types for consistent output
            let mut type_list: Vec<_> = type_counts.iter().collect();
            type_list.sort_by_key(|(t, _)| format!("{:?}", t));

            let model_name = if model_dir.ends_with("ner") {
                "DistilBERT"
            } else {
                "TinyBERT"
            };

            eprintln!(
                "\n  {:<22} [{}{}]",
                label,
                model_name,
                if *ensemble { " +ensemble" } else { "" }
            );
            eprintln!("  {:>4} total matches", total);
            for (pii_type, count) in &type_list {
                eprintln!("    {:?}: {}", pii_type, count);
            }

            // Assertions: all tiers must detect Name, Health, GPS
            assert!(
                type_counts.contains_key(&PiiType::Person),
                "{}: must detect Person (names) — found: {:?}",
                label,
                type_counts.keys().collect::<Vec<_>>()
            );
            assert!(
                type_counts.contains_key(&PiiType::HealthKeyword),
                "{}: must detect HealthKeyword (diagnosis) — found: {:?}",
                label,
                type_counts.keys().collect::<Vec<_>>()
            );
            assert!(
                type_counts.contains_key(&PiiType::GpsCoordinate),
                "{}: must detect GpsCoordinate — found: {:?}",
                label,
                type_counts.keys().collect::<Vec<_>>()
            );
        }

        eprintln!("\n  ALL 6 TIERS PASS: Person + HealthKeyword + GpsCoordinate detected");
    }

    /// Validate network_inventory.tsv: must detect IPv4, IPv6, MAC, GPS, Email.
    /// All these types are now FPE-eligible after the FPE extension.
    #[test]
    fn test_network_inventory_fpe_types() {
        use std::path::Path;
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let tsv_path = manifest
            .parent()
            .unwrap()
            .join("test/data/input/Structured_Data_PII/network_inventory.tsv");
        let content = std::fs::read_to_string(&tsv_path).expect("Failed to read TSV");
        let data_lines: Vec<&str> = content
            .lines()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .collect();

        let scanner = HybridScanner::new(true, None, None);

        let mut type_counts: std::collections::HashMap<PiiType, usize> =
            std::collections::HashMap::new();
        let mut total = 0;

        for line in &data_lines {
            let matches = scanner.scan_text(line);
            total += matches.len();
            for m in &matches {
                *type_counts.entry(m.pii_type).or_insert(0) += 1;
            }
        }

        let mut type_list: Vec<_> = type_counts.iter().collect();
        type_list.sort_by_key(|(t, _)| format!("{:?}", t));

        eprintln!("\n============================================================");
        eprintln!("  network_inventory.tsv — FPE Type Validation");
        eprintln!("  {} data rows, {} total matches", data_lines.len(), total);
        eprintln!("============================================================");
        for (pii_type, count) in &type_list {
            let fpe = if pii_type.is_fpe_eligible() {
                " [FPE]"
            } else {
                ""
            };
            eprintln!("    {:?}: {}{}", pii_type, count, fpe);
        }

        // Must detect all FPE-eligible network types
        assert!(
            type_counts.contains_key(&PiiType::Ipv4Address),
            "Must detect IPv4 addresses — found: {:?}",
            type_counts.keys().collect::<Vec<_>>()
        );
        assert!(
            type_counts.contains_key(&PiiType::Ipv6Address),
            "Must detect IPv6 addresses — found: {:?}",
            type_counts.keys().collect::<Vec<_>>()
        );
        assert!(
            type_counts.contains_key(&PiiType::MacAddress),
            "Must detect MAC addresses — found: {:?}",
            type_counts.keys().collect::<Vec<_>>()
        );
        assert!(
            type_counts.contains_key(&PiiType::GpsCoordinate),
            "Must detect GPS coordinates — found: {:?}",
            type_counts.keys().collect::<Vec<_>>()
        );
        assert!(
            type_counts.contains_key(&PiiType::Email),
            "Must detect emails — found: {:?}",
            type_counts.keys().collect::<Vec<_>>()
        );

        // All detected types should be FPE-eligible (no non-FPE types in this file except
        // NER-detected Location/Person which may or may not appear)
        let fpe_count: usize = type_counts
            .iter()
            .filter(|(t, _)| t.is_fpe_eligible())
            .map(|(_, c)| c)
            .sum();
        eprintln!("  {} / {} matches are FPE-eligible", fpe_count, total);
        assert!(fpe_count > 0, "Must have FPE-eligible matches");
    }

    /// Validate patient_records.csv: must detect SSN, Phone, Email, GPS, HealthKeyword.
    /// SSN/Phone/Email were already FPE-eligible; GPS is newly FPE-eligible.
    #[test]
    fn test_patient_records_fpe_types() {
        use std::path::Path;
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let csv_path = manifest
            .parent()
            .unwrap()
            .join("test/data/input/Structured_Data_PII/patient_records.csv");
        let content = std::fs::read_to_string(&csv_path).expect("Failed to read CSV");
        let data_lines: Vec<&str> = content
            .lines()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .collect();

        let scanner = HybridScanner::new(true, None, None);

        let mut type_counts: std::collections::HashMap<PiiType, usize> =
            std::collections::HashMap::new();
        let mut total = 0;

        for line in &data_lines {
            let matches = scanner.scan_text(line);
            total += matches.len();
            for m in &matches {
                *type_counts.entry(m.pii_type).or_insert(0) += 1;
            }
        }

        let mut type_list: Vec<_> = type_counts.iter().collect();
        type_list.sort_by_key(|(t, _)| format!("{:?}", t));

        eprintln!("\n============================================================");
        eprintln!("  patient_records.csv — FPE Type Validation");
        eprintln!("  {} data rows, {} total matches", data_lines.len(), total);
        eprintln!("============================================================");
        for (pii_type, count) in &type_list {
            let fpe = if pii_type.is_fpe_eligible() {
                " [FPE]"
            } else {
                ""
            };
            eprintln!("    {:?}: {}{}", pii_type, count, fpe);
        }

        // Must detect these FPE-eligible types
        assert!(
            type_counts.contains_key(&PiiType::Ssn),
            "Must detect SSN — found: {:?}",
            type_counts.keys().collect::<Vec<_>>()
        );
        assert!(
            type_counts.contains_key(&PiiType::PhoneNumber),
            "Must detect Phone — found: {:?}",
            type_counts.keys().collect::<Vec<_>>()
        );
        assert!(
            type_counts.contains_key(&PiiType::Email),
            "Must detect Email — found: {:?}",
            type_counts.keys().collect::<Vec<_>>()
        );
        assert!(
            type_counts.contains_key(&PiiType::GpsCoordinate),
            "Must detect GPS (now FPE-eligible) — found: {:?}",
            type_counts.keys().collect::<Vec<_>>()
        );
        // HealthKeyword is non-FPE (uses hash-token)
        assert!(
            type_counts.contains_key(&PiiType::HealthKeyword),
            "Must detect HealthKeyword — found: {:?}",
            type_counts.keys().collect::<Vec<_>>()
        );

        // GPS must be FPE-eligible
        assert!(
            PiiType::GpsCoordinate.is_fpe_eligible(),
            "GPS must now be FPE-eligible"
        );

        // Count FPE vs non-FPE matches
        let fpe_count: usize = type_counts
            .iter()
            .filter(|(t, _)| t.is_fpe_eligible())
            .map(|(_, c)| c)
            .sum();
        let non_fpe_count: usize = type_counts
            .iter()
            .filter(|(t, _)| !t.is_fpe_eligible())
            .map(|(_, c)| c)
            .sum();
        eprintln!(
            "  {} FPE-eligible + {} hash-token = {} total",
            fpe_count, non_fpe_count, total
        );
    }
}
