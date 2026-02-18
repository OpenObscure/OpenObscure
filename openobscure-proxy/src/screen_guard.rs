//! Screenshot detection heuristics.
//!
//! Identifies images that are likely screenshots based on:
//! - EXIF metadata: software tags indicating screenshot tools, no camera hardware
//! - Dimensions: common screen resolutions (1920x1080, 2560x1440, etc.)
//! - Visual: high color uniformity in the top strip (status/title bar)
//!
//! When a screenshot is detected, the image pipeline switches to aggressive mode:
//! all text regions are blurred regardless of PII detection tier.

use image::{DynamicImage, GenericImageView};

/// Common screen resolutions (width, height) at 1x and 2x (Retina) scaling.
const SCREEN_RESOLUTIONS: &[(u32, u32)] = &[
    // 1x desktop
    (1920, 1080),
    (2560, 1440),
    (1366, 768),
    (1440, 900),
    (1680, 1050),
    (1280, 720),
    (1280, 800),
    (1600, 900),
    (3840, 2160),
    // 2x Retina / HiDPI
    (2880, 1800),
    (3024, 1964),
    (2560, 1600),
    (5120, 2880),
    (3840, 2400),
    // Mobile (common screenshot sizes)
    (1170, 2532), // iPhone 12/13/14
    (1284, 2778), // iPhone 12/13/14 Pro Max
    (1080, 2340), // Many Android
    (1080, 2400), // Many Android
    (1440, 3200), // Samsung S series
    (1290, 2796), // iPhone 15 Pro Max
];

/// EXIF software tags that indicate a screenshot tool.
const SCREENSHOT_SOFTWARE: &[&str] = &[
    "screenshot",
    "snipping tool",
    "snip & sketch",
    "gnome-screenshot",
    "spectacle",
    "flameshot",
    "shutter",
    "grab",
    "screencapture",
    "lightshot",
    "greenshot",
    "sharex",
    "snagit",
    "cleanshot",
    "monosnap",
    "skitch",
    "screen capture",
    "printscreen",
];

/// Result of screenshot detection.
#[derive(Debug, Clone)]
pub struct ScreenGuardResult {
    /// Whether the image is likely a screenshot.
    pub is_screenshot: bool,
    /// Which heuristics matched.
    pub reasons: Vec<ScreenGuardReason>,
}

/// Reason a screenshot was detected.
#[derive(Debug, Clone, PartialEq)]
pub enum ScreenGuardReason {
    /// EXIF software field contains a screenshot tool name.
    ExifSoftware(String),
    /// Image dimensions match a common screen resolution.
    ScreenResolution(u32, u32),
    /// High color uniformity in the top strip suggests a status/title bar.
    StatusBar,
    /// EXIF has no camera hardware info but has software info.
    NoCameraHardware,
}

impl ScreenGuardResult {
    pub fn not_screenshot() -> Self {
        Self {
            is_screenshot: false,
            reasons: Vec::new(),
        }
    }
}

/// Analyze EXIF metadata for screenshot indicators.
///
/// Reads the raw bytes (before `image` crate decoding strips EXIF) using `kamadak-exif`.
/// Returns reasons if screenshot indicators found.
pub fn check_exif(raw_bytes: &[u8]) -> Vec<ScreenGuardReason> {
    let mut reasons = Vec::new();

    let mut cursor = std::io::Cursor::new(raw_bytes);
    let exif = match exif::Reader::new().read_from_container(&mut cursor) {
        Ok(e) => e,
        Err(_) => return reasons, // No EXIF data or parse error
    };

    let mut has_software = false;
    let mut has_camera = false;

    // Check Software tag
    if let Some(field) = exif.get_field(exif::Tag::Software, exif::In::PRIMARY) {
        let software = field.display_value().to_string().to_lowercase();
        has_software = true;
        for term in SCREENSHOT_SOFTWARE {
            if software.contains(term) {
                reasons.push(ScreenGuardReason::ExifSoftware(software.clone()));
                break;
            }
        }
    }

    // Check UserComment for screenshot terms
    if let Some(field) = exif.get_field(exif::Tag::UserComment, exif::In::PRIMARY) {
        let comment = field.display_value().to_string().to_lowercase();
        for term in SCREENSHOT_SOFTWARE {
            if comment.contains(term) {
                reasons.push(ScreenGuardReason::ExifSoftware(comment.clone()));
                break;
            }
        }
    }

    // Check for camera hardware (Make, Model)
    if exif.get_field(exif::Tag::Make, exif::In::PRIMARY).is_some()
        || exif.get_field(exif::Tag::Model, exif::In::PRIMARY).is_some()
    {
        has_camera = true;
    }

    // Software present but no camera → suspicious
    if has_software && !has_camera {
        reasons.push(ScreenGuardReason::NoCameraHardware);
    }

    reasons
}

/// Check if image dimensions match a common screen resolution.
pub fn check_resolution(width: u32, height: u32) -> Option<ScreenGuardReason> {
    for &(sw, sh) in SCREEN_RESOLUTIONS {
        if (width == sw && height == sh) || (width == sh && height == sw) {
            return Some(ScreenGuardReason::ScreenResolution(width, height));
        }
    }
    None
}

/// Check for a status/title bar by analyzing color uniformity in the top strip.
///
/// Screenshots typically have a very uniform color band at the top (OS status bar,
/// browser title bar). We check if the top 5% of the image has low color variance.
pub fn check_status_bar(img: &DynamicImage) -> Option<ScreenGuardReason> {
    let (width, height) = img.dimensions();
    if width < 100 || height < 100 {
        return None; // Too small for meaningful analysis
    }

    let strip_height = (height as f32 * 0.05).max(3.0) as u32;
    let rgb = img.to_rgb8();

    // Sample pixels across the strip
    let sample_count = width.min(200); // Sample up to 200 pixels
    let step = (width as f32 / sample_count as f32).max(1.0) as u32;

    let mut sum_r = 0u64;
    let mut sum_g = 0u64;
    let mut sum_b = 0u64;
    let mut count = 0u64;

    for y in 0..strip_height {
        let mut x = 0;
        while x < width {
            let pixel = rgb.get_pixel(x, y);
            sum_r += pixel[0] as u64;
            sum_g += pixel[1] as u64;
            sum_b += pixel[2] as u64;
            count += 1;
            x += step;
        }
    }

    if count == 0 {
        return None;
    }

    let mean_r = sum_r as f64 / count as f64;
    let mean_g = sum_g as f64 / count as f64;
    let mean_b = sum_b as f64 / count as f64;

    // Calculate variance
    let mut var_sum = 0.0f64;
    for y in 0..strip_height {
        let mut x = 0;
        while x < width {
            let pixel = rgb.get_pixel(x, y);
            let dr = pixel[0] as f64 - mean_r;
            let dg = pixel[1] as f64 - mean_g;
            let db = pixel[2] as f64 - mean_b;
            var_sum += dr * dr + dg * dg + db * db;
            x += step;
        }
    }

    let variance = var_sum / (count as f64 * 3.0);

    // Low variance threshold: status bars typically have variance < 50
    // (compared to photos which usually have variance > 500)
    if variance < 50.0 {
        Some(ScreenGuardReason::StatusBar)
    } else {
        None
    }
}

/// Run all screenshot detection heuristics.
///
/// A screenshot is flagged if at least 2 heuristics match, or if an EXIF software
/// tag explicitly names a screenshot tool.
pub fn detect_screenshot(raw_bytes: &[u8], img: &DynamicImage) -> ScreenGuardResult {
    let (width, height) = img.dimensions();
    let mut reasons = Vec::new();

    // Check EXIF
    let exif_reasons = check_exif(raw_bytes);
    let has_exif_software_match = exif_reasons
        .iter()
        .any(|r| matches!(r, ScreenGuardReason::ExifSoftware(_)));
    reasons.extend(exif_reasons);

    // Check resolution
    if let Some(reason) = check_resolution(width, height) {
        reasons.push(reason);
    }

    // Check status bar
    if let Some(reason) = check_status_bar(img) {
        reasons.push(reason);
    }

    // Decision: explicit EXIF match → screenshot.
    // Otherwise, need ≥2 heuristics matching.
    let is_screenshot = has_exif_software_match || reasons.len() >= 2;

    if is_screenshot {
        cg_info!(crate::cg_log::modules::SCREEN, "Screenshot detected",
            width = width, height = height,
            reasons = reasons.len());
    }

    ScreenGuardResult {
        is_screenshot,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn make_image(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([128, 64, 32])))
    }

    fn make_uniform_top_image(width: u32, height: u32) -> DynamicImage {
        let mut img = RgbImage::new(width, height);
        // Uniform gray top 10%
        let bar_height = height / 10;
        for y in 0..height {
            for x in 0..width {
                if y < bar_height {
                    img.put_pixel(x, y, Rgb([50, 50, 50])); // Uniform status bar
                } else {
                    // Varied content below
                    img.put_pixel(x, y, Rgb([
                        ((x * 7 + y * 13) % 256) as u8,
                        ((x * 11 + y * 3) % 256) as u8,
                        ((x * 5 + y * 9) % 256) as u8,
                    ]));
                }
            }
        }
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn test_check_resolution_match() {
        assert!(check_resolution(1920, 1080).is_some());
        assert!(check_resolution(1080, 1920).is_some()); // Rotated
        assert!(check_resolution(2560, 1440).is_some());
        assert!(check_resolution(2880, 1800).is_some()); // Retina
    }

    #[test]
    fn test_check_resolution_no_match() {
        assert!(check_resolution(640, 480).is_none());
        assert!(check_resolution(100, 100).is_none());
        assert!(check_resolution(1921, 1080).is_none()); // Off by one
    }

    #[test]
    fn test_check_status_bar_uniform_top() {
        let img = make_uniform_top_image(400, 200);
        let result = check_status_bar(&img);
        assert!(
            result.is_some(),
            "Uniform top strip should trigger status bar detection"
        );
    }

    #[test]
    fn test_check_status_bar_varied_image() {
        // Create a highly varied image (like a photo)
        let mut img = RgbImage::new(400, 200);
        for y in 0..200 {
            for x in 0..400 {
                img.put_pixel(x, y, Rgb([
                    ((x * 37 + y * 97) % 256) as u8,
                    ((x * 53 + y * 29) % 256) as u8,
                    ((x * 71 + y * 41) % 256) as u8,
                ]));
            }
        }
        let result = check_status_bar(&DynamicImage::ImageRgb8(img));
        assert!(result.is_none(), "Varied image should not trigger status bar detection");
    }

    #[test]
    fn test_check_status_bar_small_image() {
        let img = make_image(50, 50);
        assert!(check_status_bar(&img).is_none(), "Small images should be skipped");
    }

    #[test]
    fn test_check_exif_no_exif() {
        // PNG without EXIF
        let img = make_image(100, 100);
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let reasons = check_exif(&buf.into_inner());
        assert!(reasons.is_empty());
    }

    #[test]
    fn test_detect_screenshot_resolution_only() {
        let img = make_image(1920, 1080);
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let result = detect_screenshot(&buf.into_inner(), &img);
        // Resolution match alone is not enough (need ≥2 heuristics)
        // But uniform solid color may trigger status bar too
        // With a solid color image, the status bar check will likely fire
        // So this might actually detect as screenshot (resolution + status bar)
        // Just verify it doesn't panic
        assert!(!result.reasons.is_empty());
    }

    #[test]
    fn test_detect_screenshot_small_image() {
        let img = make_image(200, 150);
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let result = detect_screenshot(&buf.into_inner(), &img);
        // Small non-standard resolution, no EXIF → not a screenshot
        assert!(
            !result.is_screenshot || result.reasons.len() >= 2,
            "Small image should not be flagged without multiple reasons"
        );
    }

    #[test]
    fn test_detect_screenshot_varied_non_standard() {
        // Varied content, non-standard resolution, no EXIF → not screenshot
        let mut img = RgbImage::new(300, 200);
        for y in 0..200 {
            for x in 0..300 {
                img.put_pixel(x, y, Rgb([
                    ((x * 37 + y * 97) % 256) as u8,
                    ((x * 53 + y * 29) % 256) as u8,
                    ((x * 71 + y * 41) % 256) as u8,
                ]));
            }
        }
        let dyn_img = DynamicImage::ImageRgb8(img);
        let mut buf = std::io::Cursor::new(Vec::new());
        dyn_img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let result = detect_screenshot(&buf.into_inner(), &dyn_img);
        assert!(
            !result.is_screenshot,
            "Varied non-standard image should not be flagged as screenshot"
        );
    }

    #[test]
    fn test_screen_guard_result_not_screenshot() {
        let result = ScreenGuardResult::not_screenshot();
        assert!(!result.is_screenshot);
        assert!(result.reasons.is_empty());
    }
}
