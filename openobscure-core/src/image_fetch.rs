//! Fetch external URL images for privacy pipeline processing.
//!
//! When LLM API requests contain images referenced by URL (Discord CDN, S3, etc.),
//! this module fetches the image so it can be processed through the image pipeline
//! (face redaction, OCR redaction, NSFW detection) before forwarding.
//!
//! Security: HTTPS-only (except localhost), SSRF prevention via private IP rejection,
//! size limits enforced during streaming, connect+read timeout.

use std::net::IpAddr;
use std::time::Instant;

use axum::body::Body;
use http_body_util::BodyExt;
use hyper_util::client::legacy::Client;

use crate::image_detect;

type HttpsConnector =
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>;

/// Configuration for URL image fetching.
#[derive(Debug, Clone)]
pub struct ImageFetchConfig {
    /// Enable URL image fetching (default: true when image pipeline is enabled).
    pub enabled: bool,
    /// Maximum bytes to download per image (default: 10MB).
    pub max_bytes: usize,
    /// Timeout for URL image fetch in seconds (default: 10).
    pub timeout_secs: u64,
    /// Allow HTTP (non-TLS) for localhost URLs (default: true, for testing).
    pub allow_localhost_http: bool,
}

impl Default for ImageFetchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_bytes: 10 * 1024 * 1024, // 10MB
            timeout_secs: 10,
            allow_localhost_http: true,
        }
    }
}

/// Errors from URL image fetching.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("URL fetch disabled")]
    Disabled,
    #[error("HTTP not allowed for non-localhost URL: {0}")]
    HttpNotAllowed(String),
    #[error("URL fetch timeout after {0}s")]
    Timeout(u64),
    #[error("response too large: {size} bytes exceeds limit {limit}")]
    TooLarge { size: usize, limit: usize },
    #[error("HTTP error {0}")]
    HttpStatus(u16),
    #[error("connection error: {0}")]
    Connection(String),
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("not an image: {0}")]
    NotImage(String),
    #[error("private IP rejected (SSRF prevention): {0}")]
    PrivateIp(String),
    #[error("unsupported scheme: {0}")]
    UnsupportedScheme(String),
}

/// Result of fetching an image from a URL.
pub struct FetchedImage {
    pub bytes: Vec<u8>,
    pub media_type: String,
    pub fetch_ms: u64,
}

/// Validate that a URL is safe to fetch.
///
/// Rejects:
/// - Non-HTTPS URLs (except localhost when `allow_localhost_http` is true)
/// - Private/internal IP addresses (SSRF prevention)
/// - Non-HTTP(S) schemes (file://, ftp://, etc.)
pub fn validate_url(url: &str, config: &ImageFetchConfig) -> Result<(), FetchError> {
    if !config.enabled {
        return Err(FetchError::Disabled);
    }

    // Parse scheme
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| FetchError::InvalidUrl("missing scheme".to_string()))?;

    match scheme {
        "https" => {} // always allowed
        "http" => {
            // HTTP only allowed for localhost
            let host = extract_host(rest);
            if !is_localhost(&host) || !config.allow_localhost_http {
                return Err(FetchError::HttpNotAllowed(url.to_string()));
            }
        }
        other => return Err(FetchError::UnsupportedScheme(other.to_string())),
    }

    // Check for private IPs (SSRF prevention)
    let host = extract_host(rest);
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(&ip) && !is_localhost_ip(&ip) {
            return Err(FetchError::PrivateIp(host.to_string()));
        }
    }

    Ok(())
}

/// Fetch an image from a URL with security controls.
pub async fn fetch_image_url(
    url: &str,
    client: &Client<HttpsConnector, Body>,
    config: &ImageFetchConfig,
) -> Result<FetchedImage, FetchError> {
    validate_url(url, config)?;

    let start = Instant::now();

    let uri: hyper::Uri = url
        .parse()
        .map_err(|e: hyper::http::uri::InvalidUri| FetchError::InvalidUrl(e.to_string()))?;

    let req = hyper::Request::builder()
        .method(hyper::Method::GET)
        .uri(&uri)
        .header("user-agent", "OpenObscure-Proxy/1.0")
        .body(Body::empty())
        .map_err(|e| FetchError::Connection(e.to_string()))?;

    // Send request with timeout
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(config.timeout_secs),
        client.request(req),
    )
    .await
    .map_err(|_| FetchError::Timeout(config.timeout_secs))?
    .map_err(|e| FetchError::Connection(e.to_string()))?;

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(FetchError::HttpStatus(status));
    }

    // Check Content-Type
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Detect media type from Content-Type header
    let media_type = if content_type.starts_with("image/") {
        content_type
            .split(';')
            .next()
            .unwrap_or("image/png")
            .to_string()
    } else if content_type.starts_with("application/octet-stream") || content_type.is_empty() {
        // Will be detected from magic bytes later
        String::new()
    } else {
        return Err(FetchError::NotImage(content_type));
    };

    // Check Content-Length hint (but don't trust it alone)
    if let Some(cl) = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
    {
        if cl > config.max_bytes {
            return Err(FetchError::TooLarge {
                size: cl,
                limit: config.max_bytes,
            });
        }
    }

    // Collect body with size limit enforcement
    let collected_bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| FetchError::Connection(e.to_string()))?
        .to_bytes();

    if collected_bytes.len() > config.max_bytes {
        return Err(FetchError::TooLarge {
            size: collected_bytes.len(),
            limit: config.max_bytes,
        });
    }

    let collected = collected_bytes.to_vec();

    // Validate magic bytes
    if !image_detect::has_image_magic_bytes(&collected) {
        return Err(FetchError::NotImage(
            "invalid magic bytes (not a recognized image format)".to_string(),
        ));
    }

    // Detect media type from magic bytes if not in Content-Type
    let media_type = if media_type.is_empty() {
        detect_media_type_from_bytes(&collected)
    } else {
        media_type
    };

    let fetch_ms = start.elapsed().as_millis() as u64;

    Ok(FetchedImage {
        bytes: collected,
        media_type,
        fetch_ms,
    })
}

/// Extract host from URL remainder (after scheme://).
fn extract_host(url_after_scheme: &str) -> String {
    // Remove path, query, fragment
    let host_port = url_after_scheme.split('/').next().unwrap_or("");
    // Remove port
    let host = if host_port.starts_with('[') {
        // IPv6: [::1]:8080
        host_port
            .split(']')
            .next()
            .unwrap_or("")
            .trim_start_matches('[')
            .to_string()
    } else {
        host_port.split(':').next().unwrap_or("").to_string()
    };
    host
}

/// Check if a host string is localhost.
fn is_localhost(host: &str) -> bool {
    host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
        || host == "[::1]"
        || host == "0.0.0.0"
}

/// Check if an IP address is localhost.
fn is_localhost_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// Check if an IP address is in a private/reserved range (SSRF prevention).
fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()          // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
                || v4.is_loopback()  // 127.0.0.0/8
                || v4.is_link_local() // 169.254.0.0/16
                || v4.is_unspecified() // 0.0.0.0
                || v4.octets()[0] == 100 && v4.octets()[1] >= 64 && v4.octets()[1] <= 127
            // 100.64.0.0/10 (CGNAT)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()         // ::1
                || v6.is_unspecified() // ::
                // fc00::/7 (unique local)
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // fe80::/10 (link-local)
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Detect media type from image magic bytes.
fn detect_media_type_from_bytes(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png".to_string()
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg".to_string()
    } else if bytes.starts_with(b"GIF8") {
        "image/gif".to_string()
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp".to_string()
    } else {
        "image/png".to_string() // fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- URL validation tests ---

    #[test]
    fn test_validate_url_https_allowed() {
        let config = ImageFetchConfig::default();
        assert!(validate_url("https://cdn.discordapp.com/image.png", &config).is_ok());
    }

    #[test]
    fn test_validate_url_http_rejected_non_localhost() {
        let config = ImageFetchConfig::default();
        let result = validate_url("http://example.com/image.png", &config);
        assert!(matches!(result, Err(FetchError::HttpNotAllowed(_))));
    }

    #[test]
    fn test_validate_url_http_localhost_allowed() {
        let config = ImageFetchConfig::default();
        assert!(validate_url("http://127.0.0.1:8080/image.png", &config).is_ok());
        assert!(validate_url("http://localhost:3000/image.png", &config).is_ok());
    }

    #[test]
    fn test_validate_url_http_localhost_rejected_when_disabled() {
        let config = ImageFetchConfig {
            allow_localhost_http: false,
            ..Default::default()
        };
        let result = validate_url("http://127.0.0.1/image.png", &config);
        assert!(matches!(result, Err(FetchError::HttpNotAllowed(_))));
    }

    #[test]
    fn test_validate_url_private_ip_rejected() {
        let config = ImageFetchConfig::default();

        // 10.0.0.0/8
        let result = validate_url("https://10.0.0.1/image.png", &config);
        assert!(matches!(result, Err(FetchError::PrivateIp(_))));

        // 172.16.0.0/12
        let result = validate_url("https://172.16.0.1/image.png", &config);
        assert!(matches!(result, Err(FetchError::PrivateIp(_))));

        // 192.168.0.0/16
        let result = validate_url("https://192.168.1.1/image.png", &config);
        assert!(matches!(result, Err(FetchError::PrivateIp(_))));

        // 169.254.0.0/16 (link-local)
        let result = validate_url("https://169.254.1.1/image.png", &config);
        assert!(matches!(result, Err(FetchError::PrivateIp(_))));
    }

    #[test]
    fn test_validate_url_file_scheme_rejected() {
        let config = ImageFetchConfig::default();
        let result = validate_url("file:///etc/passwd", &config);
        assert!(matches!(result, Err(FetchError::UnsupportedScheme(_))));
    }

    #[test]
    fn test_validate_url_ftp_scheme_rejected() {
        let config = ImageFetchConfig::default();
        let result = validate_url("ftp://example.com/image.png", &config);
        assert!(matches!(result, Err(FetchError::UnsupportedScheme(_))));
    }

    #[test]
    fn test_validate_url_missing_scheme() {
        let config = ImageFetchConfig::default();
        let result = validate_url("cdn.discordapp.com/image.png", &config);
        assert!(matches!(result, Err(FetchError::InvalidUrl(_))));
    }

    #[test]
    fn test_validate_url_disabled() {
        let config = ImageFetchConfig {
            enabled: false,
            ..Default::default()
        };
        let result = validate_url("https://example.com/image.png", &config);
        assert!(matches!(result, Err(FetchError::Disabled)));
    }

    #[test]
    fn test_validate_url_public_ip_allowed() {
        let config = ImageFetchConfig::default();
        // Public IP should be allowed
        assert!(validate_url("https://8.8.8.8/image.png", &config).is_ok());
        assert!(validate_url("https://1.1.1.1/image.png", &config).is_ok());
    }

    #[test]
    fn test_validate_url_ipv6_localhost_allowed_http() {
        let config = ImageFetchConfig::default();
        assert!(validate_url("http://[::1]:8080/image.png", &config).is_ok());
    }

    // --- Helper tests ---

    #[test]
    fn test_extract_host() {
        assert_eq!(extract_host("example.com/path"), "example.com");
        assert_eq!(extract_host("example.com:8080/path"), "example.com");
        assert_eq!(extract_host("127.0.0.1:3000/image.png"), "127.0.0.1");
        assert_eq!(extract_host("[::1]:8080/image.png"), "::1");
    }

    #[test]
    fn test_is_private_ip() {
        assert!(is_private_ip(&"10.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"172.16.0.1".parse().unwrap()));
        assert!(is_private_ip(&"192.168.1.1".parse().unwrap()));
        assert!(is_private_ip(&"169.254.1.1".parse().unwrap()));
        assert!(is_private_ip(&"127.0.0.1".parse().unwrap()));
        assert!(!is_private_ip(&"8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip(&"1.1.1.1".parse().unwrap()));
    }

    #[test]
    fn test_detect_media_type_png() {
        let bytes = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(detect_media_type_from_bytes(&bytes), "image/png");
    }

    #[test]
    fn test_detect_media_type_jpeg() {
        let bytes = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        assert_eq!(detect_media_type_from_bytes(&bytes), "image/jpeg");
    }

    #[test]
    fn test_detect_media_type_gif() {
        assert_eq!(detect_media_type_from_bytes(b"GIF89a"), "image/gif");
    }

    #[test]
    fn test_detect_media_type_webp() {
        let mut bytes = vec![0u8; 12];
        bytes[..4].copy_from_slice(b"RIFF");
        bytes[8..12].copy_from_slice(b"WEBP");
        assert_eq!(detect_media_type_from_bytes(&bytes), "image/webp");
    }
}
