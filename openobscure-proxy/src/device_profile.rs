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
    /// ≥8 GB — full feature parity with gateway.
    Full,
    /// 4–8 GB — NER + image pipeline, shorter idle timeouts.
    Standard,
    /// <4 GB — CRF/regex only, conservative resource usage.
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
#[derive(Debug, Clone, Serialize)]
pub struct FeatureBudget {
    /// Capability tier that produced this budget.
    pub tier: CapabilityTier,
    /// Maximum RAM (MB) OpenObscure should consume.
    pub max_ram_mb: u64,
    /// Enable TinyBERT INT8 NER scanner.
    pub ner_enabled: bool,
    /// Enable CRF fallback scanner.
    pub crf_enabled: bool,
    /// Enable ensemble confidence voting (agreement bonus).
    pub ensemble_enabled: bool,
    /// Enable image pipeline (face blur, OCR blur, NSFW).
    pub image_pipeline_enabled: bool,
    /// Model idle timeout before eviction (seconds).
    pub model_idle_timeout_secs: u64,
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
pub fn tier_for_profile(profile: &DeviceProfile) -> CapabilityTier {
    match profile.total_ram_mb {
        ram if ram >= 8192 => CapabilityTier::Full,
        ram if ram >= 4096 => CapabilityTier::Standard,
        _ => CapabilityTier::Lite,
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
            crf_enabled: true,
            ensemble_enabled: true,
            image_pipeline_enabled: true,
            model_idle_timeout_secs: 300,
        },
        CapabilityTier::Standard => FeatureBudget {
            tier,
            max_ram_mb: 200,
            ner_enabled: true,
            crf_enabled: true,
            ensemble_enabled: false,
            image_pipeline_enabled: true,
            model_idle_timeout_secs: 120,
        },
        CapabilityTier::Lite => FeatureBudget {
            tier,
            max_ram_mb: 80,
            ner_enabled: false,
            crf_enabled: true,
            ensemble_enabled: false,
            image_pipeline_enabled: true,
            model_idle_timeout_secs: 60,
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
            crf_enabled: true,
            ensemble_enabled: true,
            image_pipeline_enabled: true,
            model_idle_timeout_secs: 300,
        },
        CapabilityTier::Standard => FeatureBudget {
            tier,
            max_ram_mb: max_ram,
            ner_enabled: max_ram >= 80,
            crf_enabled: true,
            ensemble_enabled: false,
            image_pipeline_enabled: max_ram >= 100,
            model_idle_timeout_secs: 120,
        },
        CapabilityTier::Lite => FeatureBudget {
            tier,
            max_ram_mb: max_ram,
            ner_enabled: false,
            crf_enabled: max_ram >= 25,
            ensemble_enabled: false,
            image_pipeline_enabled: max_ram >= 40,
            model_idle_timeout_secs: 60,
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
    fn test_tier_full_boundary_8gb() {
        let p = profile_with_ram(8192, false);
        assert_eq!(tier_for_profile(&p), CapabilityTier::Full);
    }

    #[test]
    fn test_tier_standard_6gb() {
        let p = profile_with_ram(6144, false);
        assert_eq!(tier_for_profile(&p), CapabilityTier::Standard);
    }

    #[test]
    fn test_tier_standard_boundary_4gb() {
        let p = profile_with_ram(4096, false);
        assert_eq!(tier_for_profile(&p), CapabilityTier::Standard);
    }

    #[test]
    fn test_tier_lite_3gb() {
        let p = profile_with_ram(3072, false);
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
        assert!(b.crf_enabled);
        assert!(b.ensemble_enabled);
        assert!(b.image_pipeline_enabled);
        assert_eq!(b.model_idle_timeout_secs, 300);
    }

    #[test]
    fn test_budget_gateway_standard() {
        let p = profile_with_ram(6144, false);
        let tier = tier_for_profile(&p);
        let b = budget_for_tier(tier, &p);
        assert_eq!(b.max_ram_mb, 200);
        assert!(b.ner_enabled);
        assert!(!b.ensemble_enabled);
        assert!(b.image_pipeline_enabled);
        assert_eq!(b.model_idle_timeout_secs, 120);
    }

    #[test]
    fn test_budget_gateway_lite() {
        let p = profile_with_ram(2048, false);
        let tier = tier_for_profile(&p);
        let b = budget_for_tier(tier, &p);
        assert_eq!(b.max_ram_mb, 80);
        assert!(!b.ner_enabled);
        assert!(b.crf_enabled);
        assert!(!b.ensemble_enabled);
        assert_eq!(b.model_idle_timeout_secs, 60);
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
        assert!(b.ensemble_enabled);
        assert!(b.image_pipeline_enabled);
    }

    #[test]
    fn test_budget_embedded_standard_6gb() {
        let p = profile_with_ram(6144, true);
        let tier = tier_for_profile(&p);
        let b = budget_for_tier(tier, &p);
        // 20% of 6144 = 1228, capped at 275
        assert_eq!(b.max_ram_mb, 275);
        assert!(b.ner_enabled); // 275 >= 80
        assert!(b.image_pipeline_enabled); // 275 >= 100
    }

    #[test]
    fn test_budget_embedded_lite_small_device() {
        let p = profile_with_ram(512, true);
        let tier = tier_for_profile(&p);
        let b = budget_for_tier(tier, &p);
        // 20% of 512 = 102, capped at 275 → 102
        assert_eq!(b.max_ram_mb, 102);
        assert!(!b.ner_enabled);
        assert!(b.crf_enabled); // 102 >= 25
        assert!(b.image_pipeline_enabled); // 102 >= 40
    }

    #[test]
    fn test_budget_embedded_lite_very_small() {
        let p = profile_with_ram(50, true);
        let tier = tier_for_profile(&p);
        let b = budget_for_tier(tier, &p);
        // 20% of 50 = 10, floor at 12
        assert_eq!(b.max_ram_mb, 12);
        assert!(!b.ner_enabled);
        assert!(!b.crf_enabled); // 12 < 25
        assert!(!b.image_pipeline_enabled); // 12 < 40
    }

    // ── Display ──────────────────────────────────────────────────────

    #[test]
    fn test_tier_display() {
        assert_eq!(format!("{}", CapabilityTier::Full), "full");
        assert_eq!(format!("{}", CapabilityTier::Standard), "standard");
        assert_eq!(format!("{}", CapabilityTier::Lite), "lite");
    }
}
