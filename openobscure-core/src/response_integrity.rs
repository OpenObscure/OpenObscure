//! Response integrity scanner for cognitive firewall.
//!
//! Orchestrates a two-tier cascade:
//!   R1 — persuasion dictionary scanning (<1ms, runs on every response)
//!   R2 — TinyBERT multi-label classifier (~30ms, runs conditionally)
//!
//! R2 activation rules:
//!   - sensitivity=high → R2 on every response
//!   - R1 flags something → R2 to confirm/suppress/upgrade
//!   - sensitivity=medium + R1 clean → R2 on `ri_sample_rate` fraction (discovery)
//!   - sensitivity=low → R2 only when R1 flags
//!   - sensitivity=off → no scanning at all
//!
//! R2 roles on an R1-flagged response:
//!   - Confirm: R2 agrees with R1 → report stands
//!   - Suppress: R2 disagrees (all scores below threshold) → R1 was false positive, drop report
//!   - Upgrade: R2 finds additional Article 5 categories → severity may increase
//!   - Discover: R1 clean, R2 flags independently → new report (sample-rate path only)
//!
//! Operates on the response path only (after FPE decryption).

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use crate::persuasion_dict::{PersuasionCategory, PersuasionDict, PersuasionMatch};
use crate::ri_model::{RiModel, RiModelError, RiPrediction};

/// Scanner sensitivity level (parsed from config string).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensitivity {
    /// No scanning at all — R1 and R2 are both skipped even if `enabled = true`.
    Off,
    /// Only report WARNING/CAUTION severity (skip NOTICE). R2 only on R1 flags.
    Low,
    /// Report all detections including NOTICE. R2 on R1 flags + sample rate.
    Medium,
    /// Report all detections. R2 on every response.
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
            Self::Notice => write!(f, "Notice"),
            Self::Warning => write!(f, "Warning"),
            Self::Caution => write!(f, "Caution"),
        }
    }
}

/// How R2 interacted with the R1 result in a cascade scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum R2Role {
    /// R2 was not invoked (model unavailable or not triggered).
    NotUsed,
    /// R2 confirmed R1's detection.
    Confirm,
    /// R2 suppressed R1's detection (false positive).
    Suppress,
    /// R2 found additional categories beyond R1.
    Upgrade,
    /// R1 missed the detection; R2 discovered it.
    Discover,
}

impl std::fmt::Display for R2Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotUsed => write!(f, "not_used"),
            Self::Confirm => write!(f, "confirm"),
            Self::Suppress => write!(f, "suppress"),
            Self::Upgrade => write!(f, "upgrade"),
            Self::Discover => write!(f, "discover"),
        }
    }
}

/// Report from scanning a single response.
#[derive(Debug, Clone)]
pub struct ResponseIntegrityReport {
    /// Individual phrase matches found by R1.
    pub flags: Vec<PersuasionMatch>,
    /// Computed severity tier.
    pub severity: SeverityTier,
    /// Distinct categories detected by R1.
    pub categories: Vec<PersuasionCategory>,
    /// R1 scan duration in microseconds.
    pub scan_time_us: u64,
    /// R2 prediction (if R2 was invoked).
    pub r2_prediction: Option<RiPrediction>,
    /// Role R2 played in this scan result.
    pub r2_role: R2Role,
    /// Article 5 categories detected by R2.
    pub r2_categories: Vec<String>,
}

/// Response integrity scanner. Holds the persuasion dictionary, optional R2 model
/// (behind a Mutex for interior mutability), and sensitivity/sampling configuration.
///
/// The Mutex on the R2 model allows `scan()` to take `&self` so the scanner
/// can be shared via `Arc<ResponseIntegrityScanner>` across async tasks.
pub struct ResponseIntegrityScanner {
    dict: PersuasionDict,
    sensitivity: Sensitivity,
    r2_model: Mutex<Option<RiModel>>,
    sample_rate: f32,
}

impl ResponseIntegrityScanner {
    /// Create a new scanner with the given sensitivity level (R1 only, no R2 model).
    pub fn new(sensitivity: Sensitivity) -> Self {
        Self {
            dict: PersuasionDict::new(),
            sensitivity,
            r2_model: Mutex::new(None),
            sample_rate: 0.10,
        }
    }

    /// Create a new scanner with R2 model support.
    pub fn with_r2(sensitivity: Sensitivity, r2_model: Option<RiModel>, sample_rate: f32) -> Self {
        Self {
            dict: PersuasionDict::new(),
            sensitivity,
            r2_model: Mutex::new(r2_model),
            sample_rate,
        }
    }

    /// Try to load the R2 model from a directory. Returns Ok(true) if loaded, Ok(false) if
    /// model files not found (graceful degradation), Err on actual loading errors.
    pub fn load_r2(
        &self,
        model_dir: &Path,
        threshold: f32,
        early_exit_threshold: f32,
    ) -> Result<bool, RiModelError> {
        if !model_dir.exists() {
            oo_info!(crate::oo_log::modules::RESPONSE_INTEGRITY,
                "R2 model directory not found, running R1-only mode",
                model_dir = %model_dir.display());
            return Ok(false);
        }
        let model = RiModel::load(model_dir, threshold, early_exit_threshold)?;
        let mut guard = self.r2_model.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(model);
        Ok(true)
    }

    /// Whether R2 model is loaded and available.
    pub fn has_r2(&self) -> bool {
        self.r2_model
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Number of phrases in the R1 dictionary.
    pub fn dict_count(&self) -> usize {
        self.dict.total_count()
    }

    /// Warm up the R2 model (if loaded). Returns warm-up duration.
    pub fn warm_r2(&self) -> Option<std::time::Duration> {
        let mut guard = self.r2_model.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_mut().map(|m| m.warm())
    }

    /// Full cascade scan: R1 first, then R2 conditionally.
    ///
    /// Returns None if:
    /// - sensitivity is Off
    /// - no detections from either tier
    /// - severity is below sensitivity threshold and R2 doesn't flag
    /// - R2 suppressed R1's detection (false positive)
    pub fn scan(&self, text: &str) -> Option<ResponseIntegrityReport> {
        if self.sensitivity == Sensitivity::Off {
            return None;
        }

        // --- R1: Dictionary scan ---
        let start = Instant::now();
        let flags = self.dict.scan_text(text);
        let r1_time_us = start.elapsed().as_micros() as u64;

        let r1_flagged = !flags.is_empty();

        // Collect R1 categories
        let r1_category_set: HashSet<PersuasionCategory> =
            flags.iter().map(|m| m.category).collect();
        let categories: Vec<PersuasionCategory> = r1_category_set.into_iter().collect();

        oo_dbg!(
            "ri_scan: text_len={}, r1_us={}, r1_flagged={}, r1_categories={}",
            text.len(),
            r1_time_us,
            r1_flagged,
            categories.len()
        );

        // --- R2: Conditional model inference ---
        let has_r2 = self
            .r2_model
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        let should_run_r2 = has_r2 && self.should_invoke_r2(r1_flagged);

        let r1_cat_count = categories.len();
        #[cfg(feature = "debug-logs")]
        let r2_start = Instant::now();
        let (r2_prediction, r2_role, r2_categories) = if should_run_r2 {
            self.run_r2_cascade(text, r1_flagged, r1_cat_count)
        } else {
            (None, R2Role::NotUsed, Vec::new())
        };
        oo_dbg!(
            "ri_scan: r2_invoked={}, r2_role={:?}, r2_ms={:.1}",
            should_run_r2,
            r2_role,
            r2_start.elapsed().as_micros() as f64 / 1000.0
        );

        // --- Merge R1 + R2 results ---

        // If R2 suppressed R1's detection, return None
        if r2_role == R2Role::Suppress {
            return None;
        }

        // If neither R1 nor R2 found anything, return None
        if !r1_flagged && r2_categories.is_empty() {
            return None;
        }

        let severity = compute_severity(&flags, &categories, &r2_categories, &r2_prediction);

        // Apply sensitivity filter
        if self.sensitivity == Sensitivity::Low && severity == SeverityTier::Notice {
            return None;
        }

        Some(ResponseIntegrityReport {
            flags,
            severity,
            categories,
            scan_time_us: r1_time_us,
            r2_prediction,
            r2_role,
            r2_categories,
        })
    }

    /// Determine whether R2 should be invoked based on sensitivity and R1 result.
    fn should_invoke_r2(&self, r1_flagged: bool) -> bool {
        match self.sensitivity {
            Sensitivity::Off => false,
            Sensitivity::High => true,
            Sensitivity::Low => r1_flagged,
            Sensitivity::Medium => {
                if r1_flagged {
                    true
                } else {
                    // Random sampling for discovery
                    self.sample_rate > 0.0 && rand_f32() < self.sample_rate
                }
            }
        }
    }

    /// Run R2 inference and determine its role relative to R1.
    fn run_r2_cascade(
        &self,
        text: &str,
        r1_flagged: bool,
        r1_cat_count: usize,
    ) -> (Option<RiPrediction>, R2Role, Vec<String>) {
        let mut guard = self.r2_model.lock().unwrap_or_else(|e| e.into_inner());
        let model = match guard.as_mut() {
            Some(m) => m,
            None => return (None, R2Role::NotUsed, Vec::new()),
        };

        let prediction = match model.predict(text) {
            Ok(pred) => pred,
            Err(e) => {
                oo_warn!(crate::oo_log::modules::RESPONSE_INTEGRITY,
                    "R2 inference failed, falling back to R1-only", error = %e);
                return (None, R2Role::NotUsed, Vec::new());
            }
        };

        let r2_flagged = prediction.is_flagged();
        let r2_cats: Vec<String> = prediction
            .flagged_categories()
            .iter()
            .map(|s| s.to_string())
            .collect();

        let role = match (r1_flagged, r2_flagged) {
            (true, true) => {
                if r2_cats.is_empty() {
                    R2Role::Confirm
                } else {
                    R2Role::Upgrade
                }
            }
            (true, false) => {
                // R2 disagrees with R1. Only suppress if R1 evidence is weak
                // (single category). Multi-category R1 hits are strong enough
                // to stand on their own — pass through as Confirm.
                if r1_cat_count >= 2 {
                    R2Role::Confirm
                } else {
                    R2Role::Suppress
                }
            }
            (false, true) => R2Role::Discover,
            (false, false) => R2Role::NotUsed,
        };

        (Some(prediction), role, r2_cats)
    }

    /// Format a warning label to prepend to the response content.
    ///
    /// Labels are user-focused and avoid internal jargon. R2 role metadata
    /// is kept in structured logs only (not in the user-facing label).
    pub fn format_warning_label(report: &ResponseIntegrityReport) -> String {
        let mut category_names: Vec<String> =
            report.categories.iter().map(|c| c.to_string()).collect();
        // Map R2 Article 5 categories to plain English
        for cat in &report.r2_categories {
            let friendly = match cat.as_str() {
                "Art_5_1_a_Deceptive" => "Deceptive Practices",
                "Art_5_1_b_Age" => "Age-Based Targeting",
                "Art_5_1_b_SocioEcon" => "Socioeconomic Targeting",
                "Art_5_1_c_Social_Scoring" => "Social Scoring",
                other => other,
            };
            category_names.push(friendly.to_string());
        }
        category_names.sort();
        category_names.dedup();

        let tactics = category_names.join(" \u{2022} "); // bullet separator

        match report.severity {
            SeverityTier::Notice => {
                format!(
                    "--- OpenObscure WARNING ---\n\
                     Detected: {tactics}\n\
                     ---\n\n",
                )
            }
            SeverityTier::Warning => {
                format!(
                    "--- OpenObscure WARNING ---\n\
                     Detected: {tactics}\n\
                     This response contains language patterns associated with influence tactics.\n\
                     ---\n\n",
                )
            }
            SeverityTier::Caution => {
                format!(
                    "--- OpenObscure WARNING ---\n\
                     Detected: {tactics}\n\
                     Recommendation: Pause and verify with objective evidence before acting.\n\
                     ---\n\n",
                )
            }
        }
    }
}

/// Compute severity tier from R1 flags, R1 categories, R2 categories, and R2 prediction.
fn compute_severity(
    flags: &[PersuasionMatch],
    r1_categories: &[PersuasionCategory],
    r2_categories: &[String],
    r2_prediction: &Option<RiPrediction>,
) -> SeverityTier {
    let num_r1_categories = r1_categories.len();
    let num_r2_categories = r2_categories.len();
    let total_categories = num_r1_categories + num_r2_categories;
    let num_flags = flags.len();

    // Caution: 4+ total categories (R1 + R2 combined)
    if total_categories >= 4 {
        return SeverityTier::Caution;
    }

    // Caution: commercial combined with fear or urgency (R1 categories)
    let has_commercial = r1_categories.contains(&PersuasionCategory::Commercial);
    let has_fear = r1_categories.contains(&PersuasionCategory::Fear);
    let has_urgency = r1_categories.contains(&PersuasionCategory::Urgency);
    if has_commercial && (has_fear || has_urgency) {
        return SeverityTier::Caution;
    }

    // Caution: R2 has high-confidence multi-category detection
    if let Some(pred) = r2_prediction {
        if num_r2_categories >= 2 && pred.max_score() > 0.90 {
            return SeverityTier::Caution;
        }
    }

    // Warning: 2-3 total categories or 3+ R1 matches
    if total_categories >= 2 || num_flags >= 3 {
        return SeverityTier::Warning;
    }

    // Warning: R2-only discovery (no R1 flags but R2 found something)
    if num_r1_categories == 0 && num_r2_categories > 0 {
        return SeverityTier::Warning;
    }

    // Notice: 1 category, 1-2 matches
    SeverityTier::Notice
}

/// Simple thread-local random float in [0, 1) using xorshift64.
/// Used only for R2 sampling decisions — not cryptographic.
fn rand_f32() -> f32 {
    use std::cell::Cell;
    use std::time::SystemTime;

    thread_local! {
        static STATE: Cell<u64> = Cell::new(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64
        );
    }

    STATE.with(|state| {
        let mut s = state.get();
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        state.set(s);
        (s & 0x00FF_FFFF) as f32 / 0x0100_0000 as f32
    })
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
        let scanner = ResponseIntegrityScanner::with_r2(Sensitivity::Medium, None, 0.0);
        let result = scanner.scan("Here is a Python function that sorts a list.");
        assert!(result.is_none());
    }

    #[test]
    fn test_notice_severity() {
        let scanner = ResponseIntegrityScanner::with_r2(Sensitivity::Medium, None, 0.0);
        // Single category, few matches
        let result = scanner.scan("This is a smart choice for your project.");
        if let Some(report) = result {
            assert_eq!(report.severity, SeverityTier::Notice);
            assert_eq!(report.categories.len(), 1);
            assert_eq!(report.r2_role, R2Role::NotUsed);
        }
    }

    #[test]
    fn test_warning_severity_multi_category() {
        let scanner = ResponseIntegrityScanner::with_r2(Sensitivity::Medium, None, 0.0);
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
        let scanner = ResponseIntegrityScanner::with_r2(Sensitivity::Medium, None, 0.0);
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
        let scanner = ResponseIntegrityScanner::with_r2(Sensitivity::Medium, None, 0.0);
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
            r2_prediction: None,
            r2_role: R2Role::NotUsed,
            r2_categories: vec![],
        };
        let label = ResponseIntegrityScanner::format_warning_label(&report);
        assert!(label.contains("--- OpenObscure WARNING ---"));
        assert!(label.contains("Detected:"));
        assert!(label.contains("Urgency"));
        assert!(label.contains("Commercial"));
        assert!(label.contains("\u{2022}"), "Should use bullet separator");
        assert!(label.contains("language patterns associated with influence tactics"));
        assert!(
            label.ends_with("---\n\n"),
            "Should end with closing separator"
        );
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
            r2_prediction: None,
            r2_role: R2Role::NotUsed,
            r2_categories: vec![],
        };
        let label = ResponseIntegrityScanner::format_warning_label(&report);
        assert!(label.contains("--- OpenObscure WARNING ---"));
        assert!(label.contains("Pause and verify with objective evidence"));
        assert!(
            label.ends_with("---\n\n"),
            "Should end with closing separator"
        );
    }

    #[test]
    fn test_scan_timing_recorded() {
        let scanner = ResponseIntegrityScanner::with_r2(Sensitivity::Medium, None, 0.0);
        let result = scanner.scan("Act now! Experts agree this is the best deal.");
        if let Some(report) = result {
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
        assert_eq!(SeverityTier::Notice.to_string(), "Notice");
        assert_eq!(SeverityTier::Warning.to_string(), "Warning");
        assert_eq!(SeverityTier::Caution.to_string(), "Caution");
    }

    // --- R2 cascade tests ---

    #[test]
    fn test_r2_role_display() {
        assert_eq!(R2Role::NotUsed.to_string(), "not_used");
        assert_eq!(R2Role::Confirm.to_string(), "confirm");
        assert_eq!(R2Role::Suppress.to_string(), "suppress");
        assert_eq!(R2Role::Upgrade.to_string(), "upgrade");
        assert_eq!(R2Role::Discover.to_string(), "discover");
    }

    #[test]
    fn test_scanner_without_r2() {
        let scanner = ResponseIntegrityScanner::new(Sensitivity::Medium);
        assert!(!scanner.has_r2());
    }

    #[test]
    fn test_scanner_with_r2_none() {
        let scanner = ResponseIntegrityScanner::with_r2(Sensitivity::Medium, None, 0.10);
        assert!(!scanner.has_r2());
    }

    #[test]
    fn test_should_invoke_r2_off() {
        let scanner = ResponseIntegrityScanner::new(Sensitivity::Off);
        assert!(!scanner.should_invoke_r2(false));
        assert!(!scanner.should_invoke_r2(true));
    }

    #[test]
    fn test_should_invoke_r2_high() {
        let scanner = ResponseIntegrityScanner::new(Sensitivity::High);
        assert!(scanner.should_invoke_r2(false));
        assert!(scanner.should_invoke_r2(true));
    }

    #[test]
    fn test_should_invoke_r2_low_needs_r1_flag() {
        let scanner = ResponseIntegrityScanner::new(Sensitivity::Low);
        assert!(!scanner.should_invoke_r2(false));
        assert!(scanner.should_invoke_r2(true));
    }

    #[test]
    fn test_should_invoke_r2_medium_r1_flagged() {
        let scanner = ResponseIntegrityScanner::new(Sensitivity::Medium);
        assert!(scanner.should_invoke_r2(true));
    }

    #[test]
    fn test_should_invoke_r2_medium_zero_sample_rate() {
        let scanner = ResponseIntegrityScanner::with_r2(Sensitivity::Medium, None, 0.0);
        assert!(!scanner.should_invoke_r2(false));
    }

    #[test]
    fn test_should_invoke_r2_medium_full_sample_rate() {
        let scanner = ResponseIntegrityScanner::with_r2(Sensitivity::Medium, None, 1.0);
        assert!(scanner.should_invoke_r2(false));
    }

    #[test]
    fn test_r1_only_report_has_not_used_r2() {
        let scanner = ResponseIntegrityScanner::with_r2(Sensitivity::Medium, None, 0.0);
        let result = scanner.scan("Act now! Experts agree this is the best deal.");
        if let Some(report) = result {
            assert_eq!(report.r2_role, R2Role::NotUsed);
            assert!(report.r2_prediction.is_none());
            assert!(report.r2_categories.is_empty());
        }
    }

    #[test]
    fn test_load_r2_nonexistent_dir_graceful() {
        let scanner = ResponseIntegrityScanner::new(Sensitivity::Medium);
        let result = scanner.load_r2(Path::new("/nonexistent/r2_model"), 0.7, 0.3);
        assert!(result.is_ok());
        assert!(!result.unwrap());
        assert!(!scanner.has_r2());
    }

    #[test]
    fn test_warning_label_with_r2_categories() {
        let report = ResponseIntegrityReport {
            flags: vec![],
            severity: SeverityTier::Warning,
            categories: vec![PersuasionCategory::Urgency],
            scan_time_us: 42,
            r2_prediction: Some(RiPrediction {
                scores: [0.9, 0.1, 0.8, 0.1],
                labels: [true, false, true, false],
                early_exit: false,
                inference_time_us: 15000,
            }),
            r2_role: R2Role::Upgrade,
            r2_categories: vec!["Art_5_1_a_Deceptive".to_string()],
        };
        let label = ResponseIntegrityScanner::format_warning_label(&report);
        assert!(label.contains("--- OpenObscure WARNING ---"));
        assert!(label.contains("Urgency"));
        assert!(label.contains("Deceptive Practices")); // Art_5_1_a in plain English
        assert!(!label.contains("[R2:")); // R2 role not in user-facing label
    }

    #[test]
    fn test_rand_f32_range() {
        for _ in 0..100 {
            let r = rand_f32();
            assert!((0.0..1.0).contains(&r), "rand_f32 returned {}", r);
        }
    }

    #[test]
    fn test_compute_severity_r2_upgrade() {
        // R1 has 1 category, R2 adds 1 more → total 2 → Warning
        let severity = compute_severity(
            &[],
            &[PersuasionCategory::Urgency],
            &["Art_5_1_a_Deceptive".to_string()],
            &None,
        );
        assert_eq!(severity, SeverityTier::Warning);
    }

    #[test]
    fn test_compute_severity_r2_only_discovery() {
        // R1 clean, R2 found something → Warning
        let severity = compute_severity(
            &[],
            &[],
            &["Art_5_1_b_Age".to_string()],
            &Some(RiPrediction {
                scores: [0.1, 0.8, 0.1, 0.1],
                labels: [false, true, false, false],
                early_exit: false,
                inference_time_us: 0,
            }),
        );
        assert_eq!(severity, SeverityTier::Warning);
    }

    // --- P4: Token fragmentation tests ---

    #[test]
    fn test_compute_severity_r2_high_confidence_multi() {
        // R2 has 2+ categories at >0.90 → Caution
        let severity = compute_severity(
            &[],
            &[],
            &[
                "Art_5_1_a_Deceptive".to_string(),
                "Art_5_1_b_Age".to_string(),
            ],
            &Some(RiPrediction {
                scores: [0.95, 0.92, 0.1, 0.1],
                labels: [true, true, false, false],
                early_exit: false,
                inference_time_us: 0,
            }),
        );
        assert_eq!(severity, SeverityTier::Caution);
    }
}
