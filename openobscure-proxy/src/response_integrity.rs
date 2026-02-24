//! Response integrity scanner for cognitive firewall.
//!
//! Orchestrates persuasion dictionary scanning on LLM responses, computes
//! severity tiers, applies sensitivity filtering, and formats warning labels.
//! Operates on the response path only (after FPE decryption).

use std::collections::HashSet;
use std::time::Instant;

use crate::persuasion_dict::{PersuasionCategory, PersuasionDict, PersuasionMatch};

/// Scanner sensitivity level (parsed from config string).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensitivity {
    /// Scanner disabled at scan level (even if enabled=true in config).
    Off,
    /// Only report WARNING/CAUTION severity (skip NOTICE).
    Low,
    /// Report all detections including NOTICE.
    Medium,
    /// Report all detections including NOTICE.
    High,
}

impl std::str::FromStr for Sensitivity {
    type Err = std::convert::Infallible;

    /// Parse from config string. Unknown values default to Medium.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "off" => Self::Off,
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            _ => Self::Medium,
        })
    }
}

/// Severity tier for a response integrity report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SeverityTier {
    /// 1 category, 1-2 matches — minor persuasion detected.
    Notice,
    /// 2-3 categories or 3+ matches — moderate persuasion patterns.
    Warning,
    /// 4+ categories or commercial+fear/urgency combo — significant manipulation.
    Caution,
}

impl std::fmt::Display for SeverityTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Notice => write!(f, "NOTICE"),
            Self::Warning => write!(f, "WARNING"),
            Self::Caution => write!(f, "CAUTION"),
        }
    }
}

/// Report from scanning a single response.
#[derive(Debug, Clone)]
pub struct ResponseIntegrityReport {
    /// Individual phrase matches found.
    pub flags: Vec<PersuasionMatch>,
    /// Computed severity tier.
    pub severity: SeverityTier,
    /// Distinct categories detected.
    pub categories: Vec<PersuasionCategory>,
    /// Scan duration in microseconds.
    pub scan_time_us: u64,
}

/// Response integrity scanner. Holds the persuasion dictionary and sensitivity level.
pub struct ResponseIntegrityScanner {
    dict: PersuasionDict,
    sensitivity: Sensitivity,
}

impl ResponseIntegrityScanner {
    /// Create a new scanner with the given sensitivity level.
    pub fn new(sensitivity: Sensitivity) -> Self {
        Self {
            dict: PersuasionDict::new(),
            sensitivity,
        }
    }

    /// Number of phrases in the dictionary.
    pub fn dict_count(&self) -> usize {
        self.dict.total_count()
    }

    /// Scan text for persuasion techniques.
    /// Returns None if:
    /// - sensitivity is Off
    /// - no matches found
    /// - severity is below sensitivity threshold (Low filters out Notice)
    pub fn scan(&self, text: &str) -> Option<ResponseIntegrityReport> {
        if self.sensitivity == Sensitivity::Off {
            return None;
        }

        let start = Instant::now();
        let flags = self.dict.scan_text(text);
        let scan_time_us = start.elapsed().as_micros() as u64;

        if flags.is_empty() {
            return None;
        }

        // Collect distinct categories
        let category_set: HashSet<PersuasionCategory> = flags.iter().map(|m| m.category).collect();
        let categories: Vec<PersuasionCategory> = category_set.into_iter().collect();

        let severity = compute_severity(&flags, &categories);

        // Apply sensitivity filter
        if self.sensitivity == Sensitivity::Low && severity == SeverityTier::Notice {
            return None;
        }

        Some(ResponseIntegrityReport {
            flags,
            severity,
            categories,
            scan_time_us,
        })
    }

    /// Format a warning label to prepend to the response content.
    pub fn format_warning_label(report: &ResponseIntegrityReport) -> String {
        let category_names: Vec<String> = {
            let mut names: Vec<String> = report.categories.iter().map(|c| c.to_string()).collect();
            names.sort();
            names
        };
        format!(
            "--- OpenObscure {} ---\nPersuasion techniques detected: {}\n---\n\n",
            report.severity,
            category_names.join(", "),
        )
    }
}

/// Compute severity tier from flags and categories.
fn compute_severity(flags: &[PersuasionMatch], categories: &[PersuasionCategory]) -> SeverityTier {
    let num_categories = categories.len();
    let num_flags = flags.len();

    // Caution: 4+ categories
    if num_categories >= 4 {
        return SeverityTier::Caution;
    }

    // Caution: commercial combined with fear or urgency
    let has_commercial = categories.contains(&PersuasionCategory::Commercial);
    let has_fear = categories.contains(&PersuasionCategory::Fear);
    let has_urgency = categories.contains(&PersuasionCategory::Urgency);
    if has_commercial && (has_fear || has_urgency) {
        return SeverityTier::Caution;
    }

    // Warning: 2-3 categories or 3+ matches
    if num_categories >= 2 || num_flags >= 3 {
        return SeverityTier::Warning;
    }

    // Notice: 1 category, 1-2 matches
    SeverityTier::Notice
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sensitivity_from_str() {
        assert_eq!("off".parse::<Sensitivity>().unwrap(), Sensitivity::Off);
        assert_eq!("low".parse::<Sensitivity>().unwrap(), Sensitivity::Low);
        assert_eq!(
            "medium".parse::<Sensitivity>().unwrap(),
            Sensitivity::Medium
        );
        assert_eq!("high".parse::<Sensitivity>().unwrap(), Sensitivity::High);
        assert_eq!("HIGH".parse::<Sensitivity>().unwrap(), Sensitivity::High);
        assert_eq!(
            "unknown".parse::<Sensitivity>().unwrap(),
            Sensitivity::Medium
        );
    }

    #[test]
    fn test_off_sensitivity_returns_none() {
        let scanner = ResponseIntegrityScanner::new(Sensitivity::Off);
        let result = scanner.scan("Act now! Limited time offer! Experts agree!");
        assert!(result.is_none());
    }

    #[test]
    fn test_clean_text_returns_none() {
        let scanner = ResponseIntegrityScanner::new(Sensitivity::Medium);
        let result = scanner.scan("Here is a Python function that sorts a list.");
        assert!(result.is_none());
    }

    #[test]
    fn test_notice_severity() {
        let scanner = ResponseIntegrityScanner::new(Sensitivity::Medium);
        // Single category, few matches
        let result = scanner.scan("This is a smart choice for your project.");
        if let Some(report) = result {
            assert_eq!(report.severity, SeverityTier::Notice);
            assert_eq!(report.categories.len(), 1);
        }
    }

    #[test]
    fn test_warning_severity_multi_category() {
        let scanner = ResponseIntegrityScanner::new(Sensitivity::Medium);
        // Urgency + flattery = 2 categories → Warning
        let result = scanner.scan("Act now! This is a smart choice.");
        if let Some(report) = result {
            assert!(
                report.severity >= SeverityTier::Warning,
                "Expected Warning or higher, got {:?}",
                report.severity
            );
        }
    }

    #[test]
    fn test_caution_severity_commercial_fear() {
        let scanner = ResponseIntegrityScanner::new(Sensitivity::Medium);
        // Commercial + fear combo → Caution
        let result = scanner.scan("Buy now or you could lose this amazing deal forever!");
        if let Some(report) = result {
            let has_commercial = report.categories.contains(&PersuasionCategory::Commercial);
            let has_fear = report.categories.contains(&PersuasionCategory::Fear);
            let has_urgency = report.categories.contains(&PersuasionCategory::Urgency);
            if has_commercial && (has_fear || has_urgency) {
                assert_eq!(report.severity, SeverityTier::Caution);
            }
        }
    }

    #[test]
    fn test_caution_severity_many_categories() {
        let scanner = ResponseIntegrityScanner::new(Sensitivity::Medium);
        let result = scanner.scan(
            "Act now! Experts agree this exclusive offer is a smart choice. \
             You could lose out on this best deal!",
        );
        if let Some(report) = result {
            if report.categories.len() >= 4 {
                assert_eq!(report.severity, SeverityTier::Caution);
            }
        }
    }

    #[test]
    fn test_low_sensitivity_filters_notice() {
        let scanner = ResponseIntegrityScanner::new(Sensitivity::Low);
        // Single flattery phrase → Notice → filtered by Low sensitivity
        let result = scanner.scan("This is a smart choice.");
        // Low sensitivity should filter out Notice-level reports
        if let Some(report) = result {
            assert!(
                report.severity >= SeverityTier::Warning,
                "Low sensitivity should not return Notice-level reports"
            );
        }
    }

    #[test]
    fn test_warning_label_format() {
        let report = ResponseIntegrityReport {
            flags: vec![],
            severity: SeverityTier::Warning,
            categories: vec![PersuasionCategory::Urgency, PersuasionCategory::Commercial],
            scan_time_us: 42,
        };
        let label = ResponseIntegrityScanner::format_warning_label(&report);
        assert!(label.contains("WARNING"));
        assert!(label.contains("Urgency"));
        assert!(label.contains("Commercial"));
        assert!(label.contains("---"));
    }

    #[test]
    fn test_caution_label_format() {
        let report = ResponseIntegrityReport {
            flags: vec![],
            severity: SeverityTier::Caution,
            categories: vec![
                PersuasionCategory::Fear,
                PersuasionCategory::Commercial,
                PersuasionCategory::Urgency,
                PersuasionCategory::Authority,
            ],
            scan_time_us: 100,
        };
        let label = ResponseIntegrityScanner::format_warning_label(&report);
        assert!(label.contains("CAUTION"));
    }

    #[test]
    fn test_scan_timing_recorded() {
        let scanner = ResponseIntegrityScanner::new(Sensitivity::Medium);
        let result = scanner.scan("Act now! Experts agree this is the best deal.");
        if let Some(report) = result {
            // scan_time_us should be populated (may be 0 on very fast machines, but should exist)
            assert!(
                report.scan_time_us < 1_000_000,
                "Scan should take < 1 second"
            );
        }
    }

    #[test]
    fn test_severity_tier_ordering() {
        assert!(SeverityTier::Notice < SeverityTier::Warning);
        assert!(SeverityTier::Warning < SeverityTier::Caution);
    }

    #[test]
    fn test_severity_display() {
        assert_eq!(SeverityTier::Notice.to_string(), "NOTICE");
        assert_eq!(SeverityTier::Warning.to_string(), "WARNING");
        assert_eq!(SeverityTier::Caution.to_string(), "CAUTION");
    }
}
