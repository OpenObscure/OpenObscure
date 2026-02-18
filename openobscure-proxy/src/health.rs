use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Json;
use serde::Serialize;

// ── Latency Histogram ────────────────────────────────────────────────────

/// Fixed 16-bucket geometric latency histogram (zero-alloc per recording).
///
/// Bucket boundaries (upper bound, microseconds):
///   [100, 250, 500, 1000, 2500, 5000, 10000, 25000,
///    50000, 100000, 250000, 500000, 1000000, 2500000, 5000000, +inf]
///
/// Percentile calculation is O(16).
#[derive(Clone)]
pub struct LatencyHistogram {
    buckets: Arc<[AtomicU64; 16]>,
    total_count: Arc<AtomicU64>,
    total_us: Arc<AtomicU64>,
}

/// Upper bound of each bucket in microseconds.
const BUCKET_BOUNDS_US: [u64; 16] = [
    100,      // 0.1ms
    250,      // 0.25ms
    500,      // 0.5ms
    1_000,    // 1ms
    2_500,    // 2.5ms
    5_000,    // 5ms
    10_000,   // 10ms
    25_000,   // 25ms
    50_000,   // 50ms
    100_000,  // 100ms
    250_000,  // 250ms
    500_000,  // 500ms
    1_000_000,  // 1s
    2_500_000,  // 2.5s
    5_000_000,  // 5s
    u64::MAX,   // +inf
];

impl LatencyHistogram {
    pub fn new() -> Self {
        Self {
            buckets: Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
            total_count: Arc::new(AtomicU64::new(0)),
            total_us: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Record a latency measurement.
    pub fn record(&self, duration: std::time::Duration) {
        let us = duration.as_micros() as u64;
        let idx = BUCKET_BOUNDS_US.iter().position(|&b| us <= b).unwrap_or(15);
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
        self.total_count.fetch_add(1, Ordering::Relaxed);
        self.total_us.fetch_add(us, Ordering::Relaxed);
    }

    /// Calculate a percentile (0.0 to 100.0). Returns bucket upper bound in microseconds.
    pub fn percentile(&self, p: f64) -> u64 {
        let total = self.total_count.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        let target = ((p / 100.0) * total as f64).ceil() as u64;
        let mut cumulative = 0u64;
        for (i, bucket) in self.buckets.iter().enumerate() {
            cumulative += bucket.load(Ordering::Relaxed);
            if cumulative >= target {
                return BUCKET_BOUNDS_US[i];
            }
        }
        BUCKET_BOUNDS_US[15]
    }

    pub fn count(&self) -> u64 {
        self.total_count.load(Ordering::Relaxed)
    }

    pub fn mean_us(&self) -> u64 {
        let count = self.total_count.load(Ordering::Relaxed);
        if count == 0 {
            return 0;
        }
        self.total_us.load(Ordering::Relaxed) / count
    }
}

/// Global health statistics tracked across all requests.
#[derive(Clone)]
pub struct HealthStats {
    start_time: Instant,
    pii_matches_total: Arc<AtomicU64>,
    requests_total: Arc<AtomicU64>,
    images_processed_total: Arc<AtomicU64>,
    faces_blurred_total: Arc<AtomicU64>,
    text_regions_total: Arc<AtomicU64>,
    cross_border_flags_total: Arc<AtomicU64>,
    pub scan_latency: LatencyHistogram,
    pub request_latency: LatencyHistogram,
}

impl HealthStats {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            pii_matches_total: Arc::new(AtomicU64::new(0)),
            requests_total: Arc::new(AtomicU64::new(0)),
            images_processed_total: Arc::new(AtomicU64::new(0)),
            faces_blurred_total: Arc::new(AtomicU64::new(0)),
            text_regions_total: Arc::new(AtomicU64::new(0)),
            cross_border_flags_total: Arc::new(AtomicU64::new(0)),
            scan_latency: LatencyHistogram::new(),
            request_latency: LatencyHistogram::new(),
        }
    }

    /// Record PII matches detected in a request.
    pub fn record_pii_matches(&self, count: u64) {
        self.pii_matches_total.fetch_add(count, Ordering::Relaxed);
    }

    /// Record a proxied request.
    pub fn record_request(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record images processed.
    pub fn record_images_processed(&self, count: u64) {
        self.images_processed_total.fetch_add(count, Ordering::Relaxed);
    }

    /// Record faces blurred.
    pub fn record_faces_blurred(&self, count: u64) {
        self.faces_blurred_total.fetch_add(count, Ordering::Relaxed);
    }

    /// Record text regions detected.
    pub fn record_text_regions(&self, count: u64) {
        self.text_regions_total.fetch_add(count, Ordering::Relaxed);
    }

    /// Record cross-border jurisdiction flags raised.
    pub fn record_cross_border_flags(&self, count: u64) {
        self.cross_border_flags_total.fetch_add(count, Ordering::Relaxed);
    }

    pub fn uptime_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    pub fn pii_matches_total(&self) -> u64 {
        self.pii_matches_total.load(Ordering::Relaxed)
    }

    pub fn requests_total(&self) -> u64 {
        self.requests_total.load(Ordering::Relaxed)
    }

    pub fn images_processed_total(&self) -> u64 {
        self.images_processed_total.load(Ordering::Relaxed)
    }

    pub fn faces_blurred_total(&self) -> u64 {
        self.faces_blurred_total.load(Ordering::Relaxed)
    }

    pub fn text_regions_total(&self) -> u64 {
        self.text_regions_total.load(Ordering::Relaxed)
    }

    pub fn cross_border_flags_total(&self) -> u64 {
        self.cross_border_flags_total.load(Ordering::Relaxed)
    }
}

/// Combined health state: stats + optional auth token for the health endpoint.
#[derive(Clone)]
pub struct HealthState {
    pub stats: HealthStats,
    pub auth_token: Option<String>,
    pub key_version: Arc<std::sync::atomic::AtomicU32>,
}

/// Health endpoint response body.
#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub uptime_secs: u64,
    pub pii_matches_total: u64,
    pub requests_total: u64,
    pub images_processed_total: u64,
    pub faces_blurred_total: u64,
    pub text_regions_total: u64,
    pub cross_border_flags_total: u64,
    pub fpe_key_version: u32,
    pub scan_latency_p50_us: u64,
    pub scan_latency_p95_us: u64,
    pub scan_latency_p99_us: u64,
    pub request_latency_p50_us: u64,
    pub request_latency_p95_us: u64,
    pub request_latency_p99_us: u64,
}

/// GET /_openobscure/health — returns proxy health status.
///
/// If an auth token is configured, the `X-OpenObscure-Token` header must match.
pub async fn health_handler(
    State(health_state): State<HealthState>,
    headers: HeaderMap,
) -> Result<Json<HealthResponse>, StatusCode> {
    // Validate auth token if configured
    if let Some(ref expected) = health_state.auth_token {
        let provided = headers
            .get("x-openobscure-token")
            .and_then(|v| v.to_str().ok());
        match provided {
            Some(token) if token == expected => {}
            _ => return Err(StatusCode::UNAUTHORIZED),
        }
    }

    let stats = &health_state.stats;
    Ok(Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        uptime_secs: stats.uptime_secs(),
        pii_matches_total: stats.pii_matches_total(),
        requests_total: stats.requests_total(),
        images_processed_total: stats.images_processed_total(),
        faces_blurred_total: stats.faces_blurred_total(),
        text_regions_total: stats.text_regions_total(),
        cross_border_flags_total: stats.cross_border_flags_total(),
        fpe_key_version: health_state.key_version.load(std::sync::atomic::Ordering::Relaxed),
        scan_latency_p50_us: stats.scan_latency.percentile(50.0),
        scan_latency_p95_us: stats.scan_latency.percentile(95.0),
        scan_latency_p99_us: stats.scan_latency.percentile(99.0),
        request_latency_p50_us: stats.request_latency.percentile(50.0),
        request_latency_p95_us: stats.request_latency.percentile(95.0),
        request_latency_p99_us: stats.request_latency.percentile(99.0),
    }))
}

/// Install a panic hook that writes a crash marker file before aborting.
/// On next startup, `check_crash_marker()` detects and reports recovery.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Write crash marker
        if let Some(crash_dir) = crash_marker_dir() {
            let _ = std::fs::create_dir_all(&crash_dir);
            let marker_path = crash_dir.join(".crashed");
            let timestamp = chrono_lite_now();
            let message = info.to_string();
            let content = format!("timestamp={}\nmessage={}\n", timestamp, message);
            let _ = std::fs::write(&marker_path, content);
            eprintln!("[OpenObscure] Crash marker written to {}", marker_path.display());
        }
        // Call default hook (prints backtrace etc.)
        default_hook(info);
    }));
}

/// Check for crash marker from a previous run. If found, log recovery and delete.
pub fn check_crash_marker() {
    if let Some(crash_dir) = crash_marker_dir() {
        let marker_path = crash_dir.join(".crashed");
        if marker_path.exists() {
            match std::fs::read_to_string(&marker_path) {
                Ok(content) => {
                    cg_warn!(crate::cg_log::modules::HEALTH, "Recovered from previous crash",
                        crash_info = %content.trim());
                }
                Err(_) => {
                    cg_warn!(crate::cg_log::modules::HEALTH, "Recovered from previous crash (marker unreadable)");
                }
            }
            let _ = std::fs::remove_file(&marker_path);
        }
    }
}

/// Get the crash marker directory (~/.openobscure/).
fn crash_marker_dir() -> Option<std::path::PathBuf> {
    dirs_path().map(|home| home.join(".openobscure"))
}

/// Get the user's home directory.
fn dirs_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

/// Simple ISO 8601-ish timestamp without a chrono dependency.
fn chrono_lite_now() -> String {
    use std::time::SystemTime;
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => format!("{}s_since_epoch", d.as_secs()),
        Err(_) => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_stats_initial() {
        let stats = HealthStats::new();
        assert_eq!(stats.pii_matches_total(), 0);
        assert_eq!(stats.requests_total(), 0);
        // uptime should be very small but ≥ 0
        assert!(stats.uptime_secs() < 2);
    }

    #[test]
    fn test_health_stats_record() {
        let stats = HealthStats::new();
        stats.record_pii_matches(5);
        stats.record_pii_matches(3);
        stats.record_request();
        stats.record_request();
        stats.record_request();

        assert_eq!(stats.pii_matches_total(), 8);
        assert_eq!(stats.requests_total(), 3);
    }

    #[test]
    fn test_health_stats_clone_shares_counters() {
        let stats = HealthStats::new();
        let stats2 = stats.clone();

        stats.record_pii_matches(10);
        stats2.record_request();

        assert_eq!(stats.pii_matches_total(), 10);
        assert_eq!(stats2.pii_matches_total(), 10);
        assert_eq!(stats.requests_total(), 1);
    }

    #[test]
    fn test_crash_marker_dir_exists() {
        // Just verify it doesn't panic and returns Some on platforms with HOME
        let dir = crash_marker_dir();
        if std::env::var_os("HOME").is_some() {
            assert!(dir.is_some());
        }
    }

    #[test]
    fn test_crash_marker_write_and_read() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join(".crashed");

        // Simulate crash write
        let content = format!("timestamp=12345\nmessage=test panic\n");
        std::fs::write(&marker, &content).unwrap();

        // Verify it exists and contains expected data
        assert!(marker.exists());
        let read = std::fs::read_to_string(&marker).unwrap();
        assert!(read.contains("test panic"));

        // Clean up (simulates check_crash_marker deletion)
        std::fs::remove_file(&marker).unwrap();
        assert!(!marker.exists());
    }

    // ── Latency Histogram Tests ────────────────────────────────────────

    #[test]
    fn test_histogram_empty() {
        let h = LatencyHistogram::new();
        assert_eq!(h.count(), 0);
        assert_eq!(h.mean_us(), 0);
        assert_eq!(h.percentile(50.0), 0);
        assert_eq!(h.percentile(99.0), 0);
    }

    #[test]
    fn test_histogram_single_record() {
        let h = LatencyHistogram::new();
        h.record(std::time::Duration::from_micros(500)); // falls in bucket 2 (<=500µs)
        assert_eq!(h.count(), 1);
        assert_eq!(h.mean_us(), 500);
        assert_eq!(h.percentile(50.0), 500);
        assert_eq!(h.percentile(99.0), 500);
    }

    #[test]
    fn test_histogram_multiple_buckets() {
        let h = LatencyHistogram::new();
        // 10 records at ~50µs → bucket 0 (<=100µs)
        for _ in 0..10 {
            h.record(std::time::Duration::from_micros(50));
        }
        // 10 records at ~5ms → bucket 5 (<=5000µs)
        for _ in 0..10 {
            h.record(std::time::Duration::from_micros(5000));
        }
        assert_eq!(h.count(), 20);
        // p50 should be bucket 0 (100µs) since 50% of 20 = 10 which is exactly the first bucket
        assert_eq!(h.percentile(50.0), 100);
        // p99 should be bucket 5 (5000µs)
        assert_eq!(h.percentile(99.0), 5_000);
    }

    #[test]
    fn test_histogram_clone_shares_state() {
        let h = LatencyHistogram::new();
        let h2 = h.clone();
        h.record(std::time::Duration::from_millis(1));
        assert_eq!(h2.count(), 1);
    }

    #[test]
    fn test_histogram_very_large_latency() {
        let h = LatencyHistogram::new();
        h.record(std::time::Duration::from_secs(10)); // 10s → bucket 15 (+inf)
        assert_eq!(h.count(), 1);
        assert_eq!(h.percentile(50.0), u64::MAX);
    }

    // ── Auth Token Tests ──────────────────────────────────────────────

    use axum::{body::Body, http::Request, Router};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn health_app(auth_token: Option<String>) -> Router {
        let state = HealthState {
            stats: HealthStats::new(),
            auth_token,
            key_version: Arc::new(std::sync::atomic::AtomicU32::new(1)),
        };
        Router::new().route(
            "/_openobscure/health",
            axum::routing::get(health_handler).with_state(state),
        )
    }

    #[tokio::test]
    async fn test_health_valid_token() {
        let app = health_app(Some("test-token-123".to_string()));

        let req = Request::get("/_openobscure/health")
            .header("x-openobscure-token", "test-token-123")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn test_health_missing_token_rejected() {
        let app = health_app(Some("test-token-123".to_string()));

        let req = Request::get("/_openobscure/health")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_health_wrong_token_rejected() {
        let app = health_app(Some("test-token-123".to_string()));

        let req = Request::get("/_openobscure/health")
            .header("x-openobscure-token", "wrong-token")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_health_no_auth_configured_allows_all() {
        let app = health_app(None);

        let req = Request::get("/_openobscure/health")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
