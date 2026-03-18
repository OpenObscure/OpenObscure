//! Device Hardware Profiler — runtime capability detection and feature budgeting.
//!
//! Detects total RAM, available RAM, and CPU cores at startup, classifies the
//! device into a capability tier (Full / Standard / Lite), and derives a
//! `FeatureBudget` that determines which scanners, image pipeline, and ensemble
//! voting to enable.
//!
//! Both the gateway binary (`main.rs`) and mobile library (`lib_mobile.rs`)
//! use the same profiler, so a phone with 12GB RAM gets the same PII detection
//! efficacy as a desktop server.

use serde::Serialize;
use std::fmt;

// ── Types ────────────────────────────────────────────────────────────────

/// Hardware profile detected at startup.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceProfile {
    /// Total physical RAM in MB.
    pub total_ram_mb: u64,
    /// Available (free + reclaimable) RAM in MB, if detectable.
    pub available_ram_mb: Option<u64>,
    /// Number of logical CPU cores.
    pub cpu_cores: usize,
    /// True when running as an embedded library (mobile), false for gateway.
    pub embedded: bool,
}

/// Device capability tier based on total RAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityTier {
    /// ≥4 GB physical RAM — full feature set: NER (DistilBERT), SCRFD face, PP-OCRv4, NSFW, ensemble voting.
    Full,
    /// 2–4 GB physical RAM — budget-gated features; DistilBERT if budget ≥120 MB, else TinyBERT.
    Standard,
    /// <2 GB physical RAM — TinyBERT + basic image pipeline; no NSFW, voice, or RI.
    Lite,
}

impl fmt::Display for CapabilityTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapabilityTier::Full => write!(f, "full"),
            CapabilityTier::Standard => write!(f, "standard"),
            CapabilityTier::Lite => write!(f, "lite"),
        }
    }
}

/// Feature budget derived from capability tier + device profile.
///
/// Determines which scanners, image pipeline, and ensemble voting are enabled.
/// Intentionally has no `Default` impl — every field must be explicitly set by
/// `FeatureBudget::for_tier()` so that adding a new feature triggers a compile
/// error until the caller decides whether to enable it per tier.
#[derive(Debug, Clone, Serialize)]
pub struct FeatureBudget {
    /// Capability tier that produced this budget.
    pub tier: CapabilityTier,
    /// Maximum RAM (MB) OpenObscure should consume.
    pub max_ram_mb: u64,
    /// Enable NER scanner.
    pub ner_enabled: bool,
    /// NER model variant: "tinybert" (default, fast) or "distilbert" (opt-in, higher accuracy).
    pub ner_model: String,
    /// Enable CRF fallback scanner.
    pub crf_enabled: bool,
    /// Enable ensemble confidence voting (agreement bonus).
    pub ensemble_enabled: bool,
    /// Enable image pipeline (face redaction, OCR redaction, NSFW).
    pub image_pipeline_enabled: bool,
    /// OCR processing tier: "full_recognition" or "detect_and_fill".
    pub ocr_tier: String,
    /// Enable NSFW/nudity detection model.
    pub nsfw_enabled: bool,
    /// Enable screenshot detection heuristics.
    pub screen_guard_enabled: bool,
    /// Face detection model: "scrfd" (Full/Standard) or "ultralight" (Lite).
    pub face_model: String,
    /// Model idle timeout before eviction (seconds).
    pub model_idle_timeout_secs: u64,
    /// Enable voice KWS pipeline for audio PII detection.
    pub voice_enabled: bool,
    /// Enable response integrity scanning (R2 model gating).
    pub ri_enabled: bool,
    /// Enable name gazetteer for person name detection.
    pub gazetteer_enabled: bool,
    /// Enable keyword dictionary for health/child term detection.
    pub keywords_enabled: bool,
    /// Maximum NER pool size (number of concurrent model instances).
    pub ner_pool_size: usize,
}

// ── Hardware Detection ───────────────────────────────────────────────────

/// Detect total physical RAM in MB. Returns `None` if detection fails.
pub fn total_ram_mb() -> Option<u64> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        // sysctl hw.memsize returns total physical memory in bytes
        use std::mem;
        let mut mib = [libc::CTL_HW, HW_MEMSIZE];
        let mut memsize: u64 = 0;
        let mut len = mem::size_of::<u64>();
        let ret = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                2,
                &mut memsize as *mut u64 as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if ret == 0 {
            Some(memsize / (1024 * 1024))
        } else {
            None
        }
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let content = std::fs::read_to_string("/proc/meminfo").ok()?;
        for line in content.lines() {
            if line.starts_with("MemTotal:") {
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
            return Some(status.ullTotalPhys / (1024 * 1024));
        }
        None
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "linux",
        target_os = "android",
        target_os = "windows"
    )))]
    {
        None
    }
}

/// Get available (free + reclaimable) system RAM in MB. Returns `None` if unavailable.
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
    #[cfg(any(target_os = "linux", target_os = "android"))]
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
    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "android",
        target_os = "windows"
    )))]
    {
        None
    }
}

/// Detect the number of logical CPU cores.
pub fn cpu_cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

// ── Profile + Tier + Budget ──────────────────────────────────────────────

/// Detect the full device hardware profile.
///
/// * `embedded` — `true` for mobile library, `false` for gateway binary.
pub fn detect(embedded: bool) -> DeviceProfile {
    DeviceProfile {
        total_ram_mb: total_ram_mb().unwrap_or(0),
        available_ram_mb: available_ram_mb(),
        cpu_cores: cpu_cores(),
        embedded,
    }
}

/// Classify a device profile into a capability tier based on total RAM.
///
/// Thresholds use reported RAM (not physical) — iOS/Android reserve 200–500 MB
/// for the kernel/GPU, so a physical 4 GB device reports ~3.5 GB.
pub fn tier_for_profile(profile: &DeviceProfile) -> CapabilityTier {
    match profile.total_ram_mb {
        ram if ram >= 3584 => CapabilityTier::Full, // physical ≥4 GB
        ram if ram >= 1536 => CapabilityTier::Standard, // physical ≥2 GB
        _ => CapabilityTier::Lite,                  // physical <2 GB
    }
}

/// Derive a feature budget from a capability tier and device profile.
pub fn budget_for_tier(tier: CapabilityTier, profile: &DeviceProfile) -> FeatureBudget {
    if profile.embedded {
        budget_for_embedded(tier, profile)
    } else {
        budget_for_gateway(tier)
    }
}

/// Gateway budgets: fixed per tier, no RAM-proportional scaling needed.
fn budget_for_gateway(tier: CapabilityTier) -> FeatureBudget {
    match tier {
        CapabilityTier::Full => FeatureBudget {
            tier,
            max_ram_mb: 275,
            ner_enabled: true,
            ner_model: "distilbert".to_string(),
            crf_enabled: true,
            ensemble_enabled: true,
            image_pipeline_enabled: true,
            ocr_tier: "full_recognition".to_string(),
            nsfw_enabled: true,
            screen_guard_enabled: true,
            face_model: "scrfd".to_string(),
            model_idle_timeout_secs: 300,
            voice_enabled: true,
            ri_enabled: true,
            gazetteer_enabled: true,
            keywords_enabled: true,
            ner_pool_size: 2,
        },
        CapabilityTier::Standard => FeatureBudget {
            tier,
            max_ram_mb: 200,
            ner_enabled: true,
            ner_model: "tinybert".to_string(),
            crf_enabled: true,
            ensemble_enabled: false,
            image_pipeline_enabled: true,
            ocr_tier: "full_recognition".to_string(),
            nsfw_enabled: true,
            screen_guard_enabled: true,
            face_model: "scrfd".to_string(),
            model_idle_timeout_secs: 120,
            voice_enabled: true,
            ri_enabled: true,
            gazetteer_enabled: true,
            keywords_enabled: true,
            ner_pool_size: 1,
        },
        CapabilityTier::Lite => FeatureBudget {
            tier,
            max_ram_mb: 80,
            ner_enabled: true,
            ner_model: "tinybert".to_string(),
            crf_enabled: true,
            ensemble_enabled: false,
            image_pipeline_enabled: true,
            ocr_tier: "detect_and_fill".to_string(),
            nsfw_enabled: false,
            screen_guard_enabled: false,
            face_model: "ultralight".to_string(),
            model_idle_timeout_secs: 60,
            voice_enabled: false,
            ri_enabled: false,
            gazetteer_enabled: true,
            keywords_enabled: true,
            ner_pool_size: 1,
        },
    }
}

/// Embedded (mobile) budgets: 20% of total RAM, capped at 275MB.
/// Feature selection based on available budget within the tier.
fn budget_for_embedded(tier: CapabilityTier, profile: &DeviceProfile) -> FeatureBudget {
    let proportional = profile.total_ram_mb / 5; // 20%
    let max_ram = proportional.clamp(12, 275);

    match tier {
        CapabilityTier::Full => FeatureBudget {
            tier,
            max_ram_mb: max_ram,
            ner_enabled: true,
            ner_model: "distilbert".to_string(),
            crf_enabled: true,
            ensemble_enabled: true,
            image_pipeline_enabled: true,
            ocr_tier: "full_recognition".to_string(),
            nsfw_enabled: true,
            screen_guard_enabled: true,
            face_model: "scrfd".to_string(),
            model_idle_timeout_secs: 300,
            voice_enabled: true,
            ri_enabled: true,
            gazetteer_enabled: true,
            keywords_enabled: true,
            ner_pool_size: 1,
        },
        CapabilityTier::Standard => FeatureBudget {
            tier,
            max_ram_mb: max_ram,
            ner_enabled: max_ram >= 80,
            ner_model: if max_ram >= 120 {
                "distilbert"
            } else {
                "tinybert"
            }
            .to_string(),
            crf_enabled: true,
            ensemble_enabled: false,
            image_pipeline_enabled: max_ram >= 100,
            ocr_tier: "detect_and_fill".to_string(),
            nsfw_enabled: max_ram >= 150,
            screen_guard_enabled: true,
            face_model: "scrfd".to_string(),
            model_idle_timeout_secs: 120,
            voice_enabled: max_ram >= 50,
            ri_enabled: max_ram >= 80,
            gazetteer_enabled: true,
            keywords_enabled: true,
            ner_pool_size: 1,
        },
        CapabilityTier::Lite => FeatureBudget {
            tier,
            max_ram_mb: max_ram,
            ner_enabled: max_ram >= 25,
            ner_model: "tinybert".to_string(),
            crf_enabled: max_ram >= 25,
            ensemble_enabled: false,
            image_pipeline_enabled: max_ram >= 40,
            ocr_tier: "detect_and_fill".to_string(),
            nsfw_enabled: false,
            screen_guard_enabled: false,
            face_model: "ultralight".to_string(),
            model_idle_timeout_secs: 60,
            voice_enabled: false,
            ri_enabled: false,
            gazetteer_enabled: true,
            keywords_enabled: true,
            ner_pool_size: 1,
        },
    }
}

// HW_MEMSIZE may not be exported by libc for all Darwin targets
#[cfg(any(target_os = "macos", target_os = "ios"))]
const HW_MEMSIZE: libc::c_int = 24;

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_total_ram_returns_some() {
        // Should work on macOS/Linux CI; may return None on exotic platforms
        let ram = total_ram_mb();
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert!(
                ram.is_some(),
                "total_ram_mb() should succeed on this platform"
            );
            assert!(ram.unwrap() > 0);
        }
    }

    #[test]
    fn test_available_ram_returns_some() {
        let ram = available_ram_mb();
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert!(
                ram.is_some(),
                "available_ram_mb() should succeed on this platform"
            );
            assert!(ram.unwrap() > 0);
        }
    }

    #[test]
    fn test_cpu_cores_at_least_one() {
        assert!(cpu_cores() >= 1);
    }

    #[test]
    fn test_detect_populates_fields() {
        let profile = detect(false);
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert!(profile.total_ram_mb > 0);
        }
        assert!(profile.cpu_cores >= 1);
        assert!(!profile.embedded);
    }

    #[test]
    fn test_detect_embedded_flag() {
        let gateway = detect(false);
        let mobile = detect(true);
        assert!(!gateway.embedded);
        assert!(mobile.embedded);
    }

    // ── Tier classification ──────────────────────────────────────────

    fn profile_with_ram(total_mb: u64, embedded: bool) -> DeviceProfile {
        DeviceProfile {
            total_ram_mb: total_mb,
            available_ram_mb: Some(total_mb / 2),
            cpu_cores: 4,
            embedded,
        }
    }

    #[test]
    fn test_tier_full_16gb() {
        let p = profile_with_ram(16384, false);
        assert_eq!(tier_for_profile(&p), CapabilityTier::Full);
    }

    #[test]
    fn test_tier_full_8gb() {
        let p = profile_with_ram(8192, false);
        assert_eq!(tier_for_profile(&p), CapabilityTier::Full);
    }

    #[test]
    fn test_tier_full_4gb_reported_as_3584() {
        // Physical 4 GB device reports ~3.5 GB after OS reservation
        let p = profile_with_ram(3584, false);
        assert_eq!(tier_for_profile(&p), CapabilityTier::Full);
    }

    #[test]
    fn test_tier_full_6gb() {
        let p = profile_with_ram(6144, false);
        assert_eq!(tier_for_profile(&p), CapabilityTier::Full);
    }

    #[test]
    fn test_tier_standard_3gb() {
        let p = profile_with_ram(3072, false);
        assert_eq!(tier_for_profile(&p), CapabilityTier::Standard);
    }

    #[test]
    fn test_tier_standard_2gb_reported_as_1536() {
        // Physical 2 GB device reports ~1.5 GB after OS reservation
        let p = profile_with_ram(1536, false);
        assert_eq!(tier_for_profile(&p), CapabilityTier::Standard);
    }

    #[test]
    fn test_tier_lite_1gb() {
        let p = profile_with_ram(1024, false);
        assert_eq!(tier_for_profile(&p), CapabilityTier::Lite);
    }

    #[test]
    fn test_tier_zero_ram() {
        let p = profile_with_ram(0, false);
        assert_eq!(tier_for_profile(&p), CapabilityTier::Lite);
    }

    // ── Gateway budgets ──────────────────────────────────────────────

    #[test]
    fn test_budget_gateway_full() {
        let p = profile_with_ram(16384, false);
        let tier = tier_for_profile(&p);
        let b = budget_for_tier(tier, &p);
        assert_eq!(b.max_ram_mb, 275);
        assert!(b.ner_enabled);
        assert_eq!(b.ner_model, "distilbert");
        assert!(b.crf_enabled);
        assert!(b.ensemble_enabled);
        assert!(b.image_pipeline_enabled);
        assert_eq!(b.ocr_tier, "full_recognition");
        assert!(b.nsfw_enabled);
        assert!(b.screen_guard_enabled);
        assert_eq!(b.face_model, "scrfd");
        assert_eq!(b.model_idle_timeout_secs, 300);
        assert!(b.voice_enabled);
        assert!(b.ri_enabled);
    }

    #[test]
    fn test_budget_gateway_standard() {
        let p = profile_with_ram(2048, false);
        let tier = tier_for_profile(&p);
        let b = budget_for_tier(tier, &p);
        assert_eq!(b.max_ram_mb, 200);
        assert!(b.ner_enabled);
        assert_eq!(b.ner_model, "tinybert");
        assert!(b.crf_enabled);
        assert!(!b.ensemble_enabled);
        assert!(b.image_pipeline_enabled);
        assert_eq!(b.ocr_tier, "full_recognition");
        assert!(b.nsfw_enabled);
        assert!(b.screen_guard_enabled);
        assert_eq!(b.face_model, "scrfd");
        assert_eq!(b.model_idle_timeout_secs, 120);
        assert!(b.voice_enabled);
        assert!(b.ri_enabled);
    }

    #[test]
    fn test_budget_gateway_lite() {
        let p = profile_with_ram(1024, false);
        let tier = tier_for_profile(&p);
        let b = budget_for_tier(tier, &p);
        assert_eq!(b.max_ram_mb, 80);
        assert!(b.ner_enabled);
        assert_eq!(b.ner_model, "tinybert");
        assert!(b.crf_enabled);
        assert!(!b.ensemble_enabled);
        assert_eq!(b.ocr_tier, "detect_and_fill");
        assert!(!b.nsfw_enabled);
        assert!(!b.screen_guard_enabled);
        assert_eq!(b.face_model, "ultralight");
        assert_eq!(b.model_idle_timeout_secs, 60);
        assert!(!b.voice_enabled);
        assert!(!b.ri_enabled);
    }

    // ── Embedded budgets ─────────────────────────────────────────────

    #[test]
    fn test_budget_embedded_full_16gb() {
        let p = profile_with_ram(16384, true);
        let tier = tier_for_profile(&p);
        let b = budget_for_tier(tier, &p);
        // 20% of 16384 = 3276, capped at 275
        assert_eq!(b.max_ram_mb, 275);
        assert!(b.ner_enabled);
        assert_eq!(b.ner_model, "distilbert");
        assert!(b.crf_enabled);
        assert!(b.ensemble_enabled);
        assert!(b.image_pipeline_enabled);
        assert_eq!(b.ocr_tier, "full_recognition");
        assert!(b.nsfw_enabled);
        assert!(b.screen_guard_enabled);
        assert_eq!(b.face_model, "scrfd");
        assert_eq!(b.model_idle_timeout_secs, 300);
        assert!(b.voice_enabled);
        assert!(b.ri_enabled);
    }

    #[test]
    fn test_budget_embedded_standard_2gb() {
        let p = profile_with_ram(2048, true);
        let tier = tier_for_profile(&p);
        let b = budget_for_tier(tier, &p);
        // 20% of 2048 = 409, capped at 275
        assert_eq!(b.max_ram_mb, 275);
        assert!(b.ner_enabled); // 275 >= 80
        assert_eq!(b.ner_model, "distilbert"); // 275 >= 120
        assert!(b.crf_enabled);
        assert!(!b.ensemble_enabled);
        assert!(b.image_pipeline_enabled); // 275 >= 100
        assert_eq!(b.ocr_tier, "detect_and_fill");
        assert!(b.nsfw_enabled); // 275 >= 150
        assert!(b.screen_guard_enabled);
        assert_eq!(b.face_model, "scrfd");
        assert_eq!(b.model_idle_timeout_secs, 120);
        assert!(b.voice_enabled); // 275 >= 50
        assert!(b.ri_enabled); // 275 >= 80
    }

    #[test]
    fn test_budget_embedded_lite_small_device() {
        let p = profile_with_ram(1024, true);
        let tier = tier_for_profile(&p);
        let b = budget_for_tier(tier, &p);
        // 20% of 1024 = 204, capped at 275 → 204
        assert_eq!(b.max_ram_mb, 204);
        assert!(b.ner_enabled); // 204 >= 25
        assert_eq!(b.ner_model, "tinybert");
        assert!(b.crf_enabled); // 102 >= 25
        assert!(!b.ensemble_enabled);
        assert!(b.image_pipeline_enabled); // 102 >= 40
        assert_eq!(b.ocr_tier, "detect_and_fill");
        assert!(!b.nsfw_enabled);
        assert!(!b.screen_guard_enabled);
        assert_eq!(b.face_model, "ultralight");
        assert_eq!(b.model_idle_timeout_secs, 60);
        assert!(!b.voice_enabled);
        assert!(!b.ri_enabled);
    }

    #[test]
    fn test_budget_embedded_lite_very_small() {
        let p = profile_with_ram(50, true);
        let tier = tier_for_profile(&p);
        let b = budget_for_tier(tier, &p);
        // 20% of 50 = 10, floor at 12
        assert_eq!(b.max_ram_mb, 12);
        assert!(!b.ner_enabled); // 12 < 25
        assert_eq!(b.ner_model, "tinybert");
        assert!(!b.crf_enabled); // 12 < 25
        assert!(!b.ensemble_enabled);
        assert!(!b.image_pipeline_enabled); // 12 < 40
        assert_eq!(b.ocr_tier, "detect_and_fill");
        assert!(!b.nsfw_enabled);
        assert!(!b.screen_guard_enabled);
        assert_eq!(b.face_model, "ultralight");
        assert_eq!(b.model_idle_timeout_secs, 60);
        assert!(!b.voice_enabled);
        assert!(!b.ri_enabled);
    }

    // ── Display ──────────────────────────────────────────────────────

    #[test]
    fn test_tier_display() {
        assert_eq!(format!("{}", CapabilityTier::Full), "full");
        assert_eq!(format!("{}", CapabilityTier::Standard), "standard");
        assert_eq!(format!("{}", CapabilityTier::Lite), "lite");
    }

    // ── Cross-Platform Validation (#6) ──────────────────────────────
    //
    // These tests validate that the platform-agnostic logic paths
    // (tier classification, budget derivation, display/serialize) work
    // identically regardless of OS. On Windows, total_ram_mb() and
    // available_ram_mb() use GlobalMemoryStatusEx via the `windows`
    // crate — the same logic then feeds into tier_for_profile() and
    // budget_for_tier() which are tested below with synthetic profiles
    // simulating Windows hardware configurations.

    #[test]
    fn test_cross_platform_detect_fallback_when_ram_unknown() {
        // Simulates a platform where hardware detection returns None
        // (e.g., exotic OS or sandboxed environment). detect() should
        // still produce a valid profile with 0 RAM.
        let profile = DeviceProfile {
            total_ram_mb: 0,
            available_ram_mb: None,
            cpu_cores: cpu_cores(),
            embedded: false,
        };
        let tier = tier_for_profile(&profile);
        assert_eq!(tier, CapabilityTier::Lite);
        let budget = budget_for_tier(tier, &profile);
        assert!(budget.ner_enabled);
        assert_eq!(budget.ner_model, "tinybert");
        assert_eq!(budget.max_ram_mb, 80);
    }

    #[test]
    fn test_cross_platform_windows_typical_16gb_desktop() {
        // Simulates a typical Windows 11 desktop with 16GB RAM
        let profile = DeviceProfile {
            total_ram_mb: 16384,
            available_ram_mb: Some(8000),
            cpu_cores: 8,
            embedded: false,
        };
        let tier = tier_for_profile(&profile);
        assert_eq!(tier, CapabilityTier::Full);
        let budget = budget_for_tier(tier, &profile);
        assert!(budget.ner_enabled);
        assert!(budget.ensemble_enabled);
        assert!(budget.image_pipeline_enabled);
        assert_eq!(budget.face_model, "scrfd");
        assert_eq!(budget.max_ram_mb, 275);
    }

    #[test]
    fn test_cross_platform_windows_8gb_laptop() {
        // Simulates a Windows laptop with 8GB RAM (Full tier boundary)
        let profile = DeviceProfile {
            total_ram_mb: 8192,
            available_ram_mb: Some(3000),
            cpu_cores: 4,
            embedded: false,
        };
        let tier = tier_for_profile(&profile);
        assert_eq!(tier, CapabilityTier::Full);
    }

    #[test]
    fn test_cross_platform_windows_4gb_low_end() {
        // Simulates a low-end Windows device with 4GB RAM — now Full tier
        let profile = DeviceProfile {
            total_ram_mb: 4096,
            available_ram_mb: Some(1500),
            cpu_cores: 2,
            embedded: false,
        };
        let tier = tier_for_profile(&profile);
        assert_eq!(tier, CapabilityTier::Full);
        let budget = budget_for_tier(tier, &profile);
        assert!(budget.ner_enabled);
        assert!(budget.ensemble_enabled);
        assert_eq!(budget.model_idle_timeout_secs, 300);
    }

    #[test]
    fn test_cross_platform_profile_serializes_to_json() {
        // Verify DeviceProfile + FeatureBudget serialize correctly
        // (important for Windows logging/diagnostics)
        let profile = DeviceProfile {
            total_ram_mb: 16384,
            available_ram_mb: Some(8000),
            cpu_cores: 8,
            embedded: false,
        };
        let json = serde_json::to_string(&profile).unwrap();
        assert!(json.contains("\"total_ram_mb\":16384"));
        assert!(json.contains("\"cpu_cores\":8"));

        let tier = tier_for_profile(&profile);
        let budget = budget_for_tier(tier, &profile);
        let budget_json = serde_json::to_string(&budget).unwrap();
        assert!(budget_json.contains("\"tier\":\"full\""));
        assert!(budget_json.contains("\"ner_enabled\":true"));
    }

    #[test]
    fn test_cross_platform_tier_serde_roundtrip() {
        // Verify CapabilityTier serializes as lowercase strings
        let tiers = [
            CapabilityTier::Full,
            CapabilityTier::Standard,
            CapabilityTier::Lite,
        ];
        let expected = ["\"full\"", "\"standard\"", "\"lite\""];
        for (tier, exp) in tiers.iter().zip(expected.iter()) {
            let json = serde_json::to_string(tier).unwrap();
            assert_eq!(&json, exp);
        }
    }

    #[test]
    fn test_cross_platform_available_ram_none_still_works() {
        // available_ram_mb can be None on any platform — budget
        // derivation must not depend on it
        let profile = DeviceProfile {
            total_ram_mb: 8192,
            available_ram_mb: None,
            cpu_cores: 4,
            embedded: false,
        };
        let tier = tier_for_profile(&profile);
        let budget = budget_for_tier(tier, &profile);
        assert_eq!(tier, CapabilityTier::Full);
        assert!(budget.ner_enabled);
    }

    // ── Feature Gate Registry ────────────────────────────────────────

    #[test]
    fn test_feature_gate_registry_parity() {
        // ═══════════════════════════════════════════════════════════════
        // FEATURE GATE REGISTRY — Every tier-gated feature MUST appear here.
        //
        // When adding a new feature:
        //   1. Add `<feature>_enabled: bool` to FeatureBudget struct
        //   2. Set it in all 6 budget arms (3 gateway + 3 embedded)
        //   3. Gate initialization in main.rs: `config.X.enabled && budget.X_enabled`
        //   4. Add the field name to this list
        //   5. Update FeatureBudgetSummary in health.rs
        //   6. Add assertions to test_budget_gateway_full/standard/lite
        // ═══════════════════════════════════════════════════════════════

        // Features that are OFF on Lite gateway tier (Full=true, Lite=false)
        const TIER_DIFFERENTIATED: &[&str] = &[
            "ensemble_enabled",
            "nsfw_enabled",
            "screen_guard_enabled",
            "voice_enabled",
            "ri_enabled",
        ];

        // Features that are ON for all gateway tiers but conditional on
        // embedded (RAM-proportional). Must exist in FeatureBudget.
        const ALWAYS_ON_GATEWAY: &[&str] =
            &["ner_enabled", "crf_enabled", "image_pipeline_enabled"];

        // String fields that differ between Full and Lite gateway tiers.
        // Full uses "distilbert" (higher accuracy, 63.7MB); Standard/Lite use "tinybert" (faster, 13.7MB).
        const TIER_DIFFERENTIATED_STRINGS: &[&str] = &["ner_model"];

        // --- Gateway: Full vs Lite must differ on TIER_DIFFERENTIATED ---
        let full_profile = profile_with_ram(16384, false);
        let lite_profile = profile_with_ram(1024, false);
        let full_budget = budget_for_tier(tier_for_profile(&full_profile), &full_profile);
        let lite_budget = budget_for_tier(tier_for_profile(&lite_profile), &lite_profile);
        let full_json = serde_json::to_value(&full_budget).unwrap();
        let lite_json = serde_json::to_value(&lite_budget).unwrap();

        for feature in TIER_DIFFERENTIATED {
            let full_val = full_json.get(feature).unwrap_or_else(|| {
                panic!(
                    "FeatureBudget missing field '{}'. See GATED_FEATURES registry in this test.",
                    feature
                )
            });
            let lite_val = lite_json.get(feature).unwrap_or_else(|| {
                panic!(
                    "FeatureBudget missing field '{}'. See GATED_FEATURES registry in this test.",
                    feature
                )
            });
            assert_ne!(
                full_val, lite_val,
                "Feature '{}' has same value on Full and Lite gateway — not tier-differentiated",
                feature
            );
        }

        // --- ALWAYS_ON_GATEWAY: must exist in budget (verified on Full) ---
        for feature in ALWAYS_ON_GATEWAY {
            assert!(
                full_json.get(feature).is_some(),
                "FeatureBudget missing field '{}'. See GATED_FEATURES registry in this test.",
                feature
            );
        }

        // --- Gateway: TIER_DIFFERENTIATED_STRINGS must differ between Full and Lite ---
        for feature in TIER_DIFFERENTIATED_STRINGS {
            let full_val = full_json.get(feature).unwrap_or_else(|| {
                panic!(
                    "FeatureBudget missing field '{}'. See GATED_FEATURES registry in this test.",
                    feature
                )
            });
            let lite_val = lite_json.get(feature).unwrap_or_else(|| {
                panic!(
                    "FeatureBudget missing field '{}'. See GATED_FEATURES registry in this test.",
                    feature
                )
            });
            assert_ne!(
                full_val, lite_val,
                "String field '{}' has same value on Full and Lite gateway — not tier-differentiated",
                feature
            );
        }

        // --- Embedded: ALWAYS_ON_GATEWAY features must be OFF on tiny devices ---
        let tiny_profile = profile_with_ram(50, true); // 20% of 50 = 10, floor 12MB
        let tiny_budget = budget_for_tier(tier_for_profile(&tiny_profile), &tiny_profile);
        let tiny_json = serde_json::to_value(&tiny_budget).unwrap();

        for feature in ALWAYS_ON_GATEWAY {
            let val = tiny_json
                .get(feature)
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            assert!(
                !val,
                "Feature '{}' should be OFF on tiny embedded device (12MB budget)",
                feature
            );
        }
    }
}
