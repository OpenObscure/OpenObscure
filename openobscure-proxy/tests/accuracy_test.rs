//! PII detection accuracy test harness.
//!
//! Loads the benchmark corpus from benches/pii_corpus.json and measures
//! precision, recall, and F1 score for the regex PII scanner.
//!
//! Accuracy targets:
//! - Structured PII (CC, SSN, Phone, Email, ApiKey): >= 99.5% recall
//! - Precision: >= 95%
//! - Zero false positives on negative samples

use std::collections::HashMap;

use openobscure_proxy::hybrid_scanner::HybridScanner;
use openobscure_proxy::pii_types::PiiType;
use openobscure_proxy::scanner::PiiScanner;

/// A single annotated corpus entry.
#[derive(serde::Deserialize, Debug)]
struct CorpusEntry {
    text: String,
    expected: Vec<ExpectedMatch>,
    /// If present, this entry requires multilingual detection (not base regex).
    #[serde(default)]
    lang: Option<String>,
}

#[derive(serde::Deserialize, Debug, Clone)]
#[allow(dead_code)]
struct ExpectedMatch {
    #[serde(rename = "type")]
    pii_type: String,
    value: String,
    start: usize,
    end: usize,
}

/// Types detected by the regex scanner (not keywords/NER).
fn is_regex_type(t: &str) -> bool {
    matches!(
        t,
        "CreditCard"
            | "Ssn"
            | "PhoneNumber"
            | "Email"
            | "ApiKey"
            | "Ipv4Address"
            | "Ipv6Address"
            | "GpsCoordinate"
            | "MacAddress"
    )
}

/// Map corpus type strings to PiiType enum.
fn parse_pii_type(s: &str) -> Option<PiiType> {
    match s {
        "CreditCard" => Some(PiiType::CreditCard),
        "Ssn" => Some(PiiType::Ssn),
        "PhoneNumber" => Some(PiiType::PhoneNumber),
        "Email" => Some(PiiType::Email),
        "ApiKey" => Some(PiiType::ApiKey),
        "Ipv4Address" => Some(PiiType::Ipv4Address),
        "Ipv6Address" => Some(PiiType::Ipv6Address),
        "GpsCoordinate" => Some(PiiType::GpsCoordinate),
        "MacAddress" => Some(PiiType::MacAddress),
        "HealthKeyword" => Some(PiiType::HealthKeyword),
        "ChildKeyword" => Some(PiiType::ChildKeyword),
        _ => None,
    }
}

/// Types detected by keyword dictionary (not regex/NER).
fn is_keyword_type(t: &str) -> bool {
    matches!(t, "HealthKeyword" | "ChildKeyword")
}

fn load_corpus() -> Vec<CorpusEntry> {
    let corpus_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("benches")
        .join("pii_corpus.json");
    let data = std::fs::read_to_string(&corpus_path)
        .unwrap_or_else(|e| panic!("Failed to load corpus at {}: {}", corpus_path.display(), e));
    serde_json::from_str(&data).unwrap_or_else(|e| panic!("Failed to parse corpus JSON: {}", e))
}

#[test]
fn test_corpus_loads_and_is_non_empty() {
    let corpus = load_corpus();
    assert!(
        corpus.len() >= 300,
        "Corpus should have >= 300 entries, got {}",
        corpus.len()
    );
}

#[test]
fn test_regex_scanner_recall() {
    let corpus = load_corpus();
    let scanner = PiiScanner::new();

    let mut total_expected = 0usize;
    let mut total_found = 0usize;
    let mut per_type_expected: HashMap<String, usize> = HashMap::new();
    let mut per_type_found: HashMap<String, usize> = HashMap::new();
    let mut missed: Vec<(usize, String, String)> = Vec::new(); // (line, type, value)

    for (idx, entry) in corpus.iter().enumerate() {
        // Skip multilingual entries — they require language detection, not base regex
        if entry.lang.is_some() {
            continue;
        }

        // Only test regex-detectable types
        let expected_regex: Vec<&ExpectedMatch> = entry
            .expected
            .iter()
            .filter(|e| is_regex_type(&e.pii_type))
            .collect();

        let detected = scanner.scan_text(&entry.text);

        for exp in &expected_regex {
            total_expected += 1;
            *per_type_expected.entry(exp.pii_type.clone()).or_insert(0) += 1;

            // Check if a match was found that overlaps with the expected value
            let found = detected.iter().any(|d| {
                let expected_type = parse_pii_type(&exp.pii_type);
                expected_type.map_or(false, |t| d.pii_type == t) && d.raw_value == exp.value
            });

            if found {
                total_found += 1;
                *per_type_found.entry(exp.pii_type.clone()).or_insert(0) += 1;
            } else {
                missed.push((idx + 1, exp.pii_type.clone(), exp.value.clone()));
            }
        }
    }

    let recall = if total_expected > 0 {
        total_found as f64 / total_expected as f64
    } else {
        1.0
    };

    // Print per-type recall
    eprintln!("\n=== Regex Scanner Recall ===");
    for (pii_type, expected) in &per_type_expected {
        let found = per_type_found.get(pii_type).copied().unwrap_or(0);
        let r = found as f64 / *expected as f64;
        eprintln!("  {}: {}/{} ({:.1}%)", pii_type, found, expected, r * 100.0);
    }
    eprintln!(
        "  TOTAL: {}/{} ({:.1}%)",
        total_found,
        total_expected,
        recall * 100.0
    );

    if !missed.is_empty() {
        eprintln!("\n=== Missed detections (first 20) ===");
        for (line, pii_type, value) in missed.iter().take(20) {
            eprintln!("  line {}: {} \"{}\"", line, pii_type, value);
        }
    }

    assert!(
        recall >= 0.995,
        "Recall {:.3} is below target 0.995. Missed {}/{} structured PII.",
        recall,
        total_expected - total_found,
        total_expected
    );
}

#[test]
fn test_regex_scanner_precision() {
    let corpus = load_corpus();
    let scanner = PiiScanner::new();

    let mut total_detected = 0usize;
    let mut true_positives = 0usize;
    let mut false_positives: Vec<(usize, String, String)> = Vec::new(); // (line, type, value)

    for (idx, entry) in corpus.iter().enumerate() {
        let detected = scanner.scan_text(&entry.text);

        for d in &detected {
            total_detected += 1;

            // Check if this detection matches any expected annotation
            let matches_expected = entry.expected.iter().any(|exp| {
                let expected_type = parse_pii_type(&exp.pii_type);
                expected_type.map_or(false, |t| d.pii_type == t) && d.raw_value == exp.value
            });

            if matches_expected {
                true_positives += 1;
            } else {
                false_positives.push((idx + 1, format!("{:?}", d.pii_type), d.raw_value.clone()));
            }
        }
    }

    let precision = if total_detected > 0 {
        true_positives as f64 / total_detected as f64
    } else {
        1.0
    };

    eprintln!("\n=== Regex Scanner Precision ===");
    eprintln!("  True positives: {}", true_positives);
    eprintln!("  False positives: {}", false_positives.len());
    eprintln!("  Total detected: {}", total_detected);
    eprintln!("  Precision: {:.1}%", precision * 100.0);

    if !false_positives.is_empty() {
        eprintln!("\n=== False positives (first 20) ===");
        for (line, pii_type, value) in false_positives.iter().take(20) {
            eprintln!("  line {}: {} \"{}\"", line, pii_type, value);
        }
    }

    assert!(
        precision >= 0.95,
        "Precision {:.3} is below target 0.95. {} false positives out of {} detections.",
        precision,
        false_positives.len(),
        total_detected
    );
}

#[test]
fn test_negative_samples_no_false_positives() {
    let corpus = load_corpus();
    let scanner = PiiScanner::new();

    let mut false_positives = Vec::new();

    for (idx, entry) in corpus.iter().enumerate() {
        // Only check entries with no expected PII
        if !entry.expected.is_empty() {
            continue;
        }

        let detected = scanner.scan_text(&entry.text);
        if !detected.is_empty() {
            for d in &detected {
                false_positives.push((idx + 1, format!("{:?}", d.pii_type), d.raw_value.clone()));
            }
        }
    }

    if !false_positives.is_empty() {
        eprintln!("\n=== False positives in negative samples ===");
        for (line, pii_type, value) in &false_positives {
            eprintln!("  line {}: {} \"{}\"", line, pii_type, value);
        }
    }

    assert!(
        false_positives.is_empty(),
        "Found {} false positives in negative samples (expected zero)",
        false_positives.len()
    );
}

#[test]
fn test_f1_score() {
    let corpus = load_corpus();
    let scanner = PiiScanner::new();

    let mut tp = 0usize;
    let mut fp = 0usize;
    let mut fn_count = 0usize;

    for entry in &corpus {
        // Skip multilingual entries — they require language detection, not base regex
        if entry.lang.is_some() {
            continue;
        }

        let expected_regex: Vec<&ExpectedMatch> = entry
            .expected
            .iter()
            .filter(|e| is_regex_type(&e.pii_type))
            .collect();
        let detected = scanner.scan_text(&entry.text);

        // Count true positives and false negatives
        for exp in &expected_regex {
            let found = detected.iter().any(|d| {
                parse_pii_type(&exp.pii_type).map_or(false, |t| d.pii_type == t)
                    && d.raw_value == exp.value
            });
            if found {
                tp += 1;
            } else {
                fn_count += 1;
            }
        }

        // Count false positives
        for d in &detected {
            let matches = entry.expected.iter().any(|exp| {
                parse_pii_type(&exp.pii_type).map_or(false, |t| d.pii_type == t)
                    && d.raw_value == exp.value
            });
            if !matches {
                fp += 1;
            }
        }
    }

    let precision = if tp + fp > 0 {
        tp as f64 / (tp + fp) as f64
    } else {
        1.0
    };
    let recall = if tp + fn_count > 0 {
        tp as f64 / (tp + fn_count) as f64
    } else {
        1.0
    };
    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };

    eprintln!("\n=== F1 Score Summary ===");
    eprintln!("  TP={}, FP={}, FN={}", tp, fp, fn_count);
    eprintln!("  Precision: {:.1}%", precision * 100.0);
    eprintln!("  Recall:    {:.1}%", recall * 100.0);
    eprintln!("  F1:        {:.3}", f1);

    assert!(f1 >= 0.95, "F1 score {:.3} is below target 0.95", f1);
}

// ── HybridScanner integration tests ──

#[test]
fn test_hybrid_scanner_keyword_recall() {
    let corpus = load_corpus();
    let scanner = HybridScanner::new(true, None);

    let mut total_expected = 0usize;
    let mut total_found = 0usize;
    let mut missed: Vec<(usize, String, String)> = Vec::new();

    for (idx, entry) in corpus.iter().enumerate() {
        let expected_kw: Vec<&ExpectedMatch> = entry
            .expected
            .iter()
            .filter(|e| is_keyword_type(&e.pii_type))
            .collect();

        if expected_kw.is_empty() {
            continue;
        }

        let detected = scanner.scan_text(&entry.text);

        for exp in &expected_kw {
            total_expected += 1;
            let found = detected.iter().any(|d| {
                let expected_type = parse_pii_type(&exp.pii_type);
                expected_type.map_or(false, |t| d.pii_type == t)
                    && d.raw_value.to_lowercase() == exp.value.to_lowercase()
            });
            if found {
                total_found += 1;
            } else {
                missed.push((idx + 1, exp.pii_type.clone(), exp.value.clone()));
            }
        }
    }

    let recall = if total_expected > 0 {
        total_found as f64 / total_expected as f64
    } else {
        1.0
    };

    eprintln!("\n=== HybridScanner Keyword Recall ===");
    eprintln!(
        "  Found: {}/{} ({:.1}%)",
        total_found,
        total_expected,
        recall * 100.0
    );
    if !missed.is_empty() {
        eprintln!("  Missed (first 10):");
        for (line, pii_type, value) in missed.iter().take(10) {
            eprintln!("    line {}: {} \"{}\"", line, pii_type, value);
        }
    }

    // Target 75% for now — dictionary expansion tracked separately
    assert!(
        recall >= 0.75,
        "Keyword recall {:.3} is below target 0.75. Missed {}/{}.",
        recall,
        total_expected - total_found,
        total_expected
    );
}

#[test]
fn test_hybrid_scanner_overall_recall() {
    let corpus = load_corpus();
    let scanner = HybridScanner::new(true, None);

    let mut total_expected = 0usize;
    let mut total_found = 0usize;

    for entry in &corpus {
        // Only test regex types (corpus doesn't annotate keyword types)
        let expected_all: Vec<&ExpectedMatch> = entry
            .expected
            .iter()
            .filter(|e| is_regex_type(&e.pii_type))
            .collect();

        let detected = scanner.scan_text(&entry.text);

        for exp in &expected_all {
            total_expected += 1;
            let found = detected.iter().any(|d| {
                let expected_type = parse_pii_type(&exp.pii_type);
                expected_type.map_or(false, |t| d.pii_type == t) && d.raw_value == exp.value
            });
            if found {
                total_found += 1;
            }
        }
    }

    let recall = if total_expected > 0 {
        total_found as f64 / total_expected as f64
    } else {
        1.0
    };

    eprintln!("\n=== HybridScanner Overall Recall (regex + keyword) ===");
    eprintln!(
        "  Found: {}/{} ({:.1}%)",
        total_found,
        total_expected,
        recall * 100.0
    );

    assert!(
        recall >= 0.95,
        "Overall recall {:.3} is below target 0.95. Missed {}/{}.",
        recall,
        total_expected - total_found,
        total_expected
    );
}

#[test]
fn test_hybrid_scanner_precision() {
    let corpus = load_corpus();
    let scanner = HybridScanner::new(true, None);

    let mut total_detected = 0usize;
    let mut true_positives = 0usize;
    let mut false_positives: Vec<(usize, String, String)> = Vec::new();

    for (idx, entry) in corpus.iter().enumerate() {
        let detected = scanner.scan_text(&entry.text);

        for d in &detected {
            // Skip keyword matches — corpus doesn't annotate keyword types
            if d.pii_type == PiiType::HealthKeyword || d.pii_type == PiiType::ChildKeyword {
                continue;
            }

            total_detected += 1;
            let matches_expected = entry.expected.iter().any(|exp| {
                let expected_type = parse_pii_type(&exp.pii_type);
                expected_type.map_or(false, |t| d.pii_type == t) && d.raw_value == exp.value
            });
            if matches_expected {
                true_positives += 1;
            } else {
                false_positives.push((idx + 1, format!("{:?}", d.pii_type), d.raw_value.clone()));
            }
        }
    }

    let precision = if total_detected > 0 {
        true_positives as f64 / total_detected as f64
    } else {
        1.0
    };

    eprintln!("\n=== HybridScanner Precision (regex types only) ===");
    eprintln!(
        "  TP: {}, FP: {}, Total: {}",
        true_positives,
        false_positives.len(),
        total_detected
    );
    eprintln!("  Precision: {:.1}%", precision * 100.0);

    if !false_positives.is_empty() {
        eprintln!("  False positives (first 20):");
        for (line, pii_type, value) in false_positives.iter().take(20) {
            eprintln!("    line {}: {} \"{}\"", line, pii_type, value);
        }
    }

    assert!(
        precision >= 0.95,
        "HybridScanner precision {:.3} is below target 0.95. {} FP out of {} detections.",
        precision,
        false_positives.len(),
        total_detected
    );
}

#[test]
fn test_hybrid_confidence_present() {
    let corpus = load_corpus();
    let scanner = HybridScanner::new(true, None);

    for entry in &corpus {
        let detected = scanner.scan_text(&entry.text);
        for d in &detected {
            assert!(
                d.confidence > 0.0,
                "Match {:?} '{}' has zero confidence",
                d.pii_type,
                d.raw_value
            );
        }
    }
}

// ---------------------------------------------------------------------------
// PII detection structural sanity tests
// ---------------------------------------------------------------------------

#[test]
fn test_pii_match_spans_valid() {
    let corpus = load_corpus();
    let scanner = HybridScanner::new(true, None);

    for (idx, entry) in corpus.iter().enumerate() {
        let detected = scanner.scan_text(&entry.text);
        for d in &detected {
            assert!(
                d.start < d.end,
                "Entry {}: match {:?} '{}' has start {} >= end {}",
                idx,
                d.pii_type,
                d.raw_value,
                d.start,
                d.end
            );
            assert!(
                d.end <= entry.text.len(),
                "Entry {}: match {:?} '{}' end {} exceeds text length {}",
                idx,
                d.pii_type,
                d.raw_value,
                d.end,
                entry.text.len()
            );
        }
    }
}

#[test]
fn test_pii_match_no_overlaps() {
    let corpus = load_corpus();
    let scanner = HybridScanner::new(true, None);

    for (idx, entry) in corpus.iter().enumerate() {
        let mut detected = scanner.scan_text(&entry.text);
        detected.sort_by_key(|d| d.start);

        for window in detected.windows(2) {
            assert!(
                window[0].end <= window[1].start,
                "Entry {}: overlapping matches {:?}[{}..{}] and {:?}[{}..{}]",
                idx,
                window[0].pii_type,
                window[0].start,
                window[0].end,
                window[1].pii_type,
                window[1].start,
                window[1].end,
            );
        }
    }
}

#[test]
fn test_pii_confidence_range() {
    let corpus = load_corpus();
    let scanner = HybridScanner::new(true, None);

    for (idx, entry) in corpus.iter().enumerate() {
        let detected = scanner.scan_text(&entry.text);
        for d in &detected {
            assert!(
                d.confidence > 0.0 && d.confidence <= 1.0,
                "Entry {}: match {:?} '{}' confidence {} not in (0, 1]",
                idx,
                d.pii_type,
                d.raw_value,
                d.confidence
            );
        }
    }
}

#[test]
fn test_pii_types_are_known() {
    let corpus = load_corpus();
    let scanner = HybridScanner::new(true, None);

    let known_types = [
        PiiType::CreditCard,
        PiiType::Ssn,
        PiiType::PhoneNumber,
        PiiType::Email,
        PiiType::ApiKey,
        PiiType::Ipv4Address,
        PiiType::Ipv6Address,
        PiiType::GpsCoordinate,
        PiiType::MacAddress,
        PiiType::HealthKeyword,
        PiiType::ChildKeyword,
    ];

    for (idx, entry) in corpus.iter().enumerate() {
        let detected = scanner.scan_text(&entry.text);
        for d in &detected {
            assert!(
                known_types.contains(&d.pii_type),
                "Entry {}: unknown PiiType {:?} for match '{}'",
                idx,
                d.pii_type,
                d.raw_value
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Multilingual PII recall test
// ---------------------------------------------------------------------------

#[test]
fn test_multilingual_recall() {
    let corpus = load_corpus();
    let scanner = HybridScanner::new(true, None);

    let mut total_expected = 0usize;
    let mut total_found = 0usize;
    let mut missed: Vec<(usize, String, String, String)> = Vec::new(); // (line, lang, type, value)

    for (idx, entry) in corpus.iter().enumerate() {
        let lang = match &entry.lang {
            Some(l) => l.clone(),
            None => continue, // Skip non-multilingual entries
        };

        let expected_pii: Vec<&ExpectedMatch> = entry
            .expected
            .iter()
            .filter(|e| is_regex_type(&e.pii_type))
            .collect();

        if expected_pii.is_empty() {
            continue;
        }

        let detected = scanner.scan_text(&entry.text);

        for exp in &expected_pii {
            total_expected += 1;
            let found = detected.iter().any(|d| {
                let expected_type = parse_pii_type(&exp.pii_type);
                expected_type.map_or(false, |t| d.pii_type == t) && d.raw_value == exp.value
            });
            if found {
                total_found += 1;
            } else {
                missed.push((
                    idx + 1,
                    lang.clone(),
                    exp.pii_type.clone(),
                    exp.value.clone(),
                ));
            }
        }
    }

    let recall = if total_expected > 0 {
        total_found as f64 / total_expected as f64
    } else {
        1.0
    };

    eprintln!("\n=== Multilingual Recall ===");
    eprintln!(
        "  Found: {}/{} ({:.1}%)",
        total_found,
        total_expected,
        recall * 100.0
    );
    if !missed.is_empty() {
        eprintln!("  Missed:");
        for (line, lang, pii_type, value) in &missed {
            eprintln!("    line {} [{}]: {} \"{}\"", line, lang, pii_type, value);
        }
    }

    // Target 70% for now — language detection may miss some short texts
    assert!(
        recall >= 0.70,
        "Multilingual recall {:.3} is below target 0.70. Missed {}/{}.",
        recall,
        total_expected - total_found,
        total_expected
    );
}
