//! Gaussian blur operations for face and text regions in images.
//!
//! Applies targeted blur to specific rectangular or quadrilateral regions
//! without affecting the rest of the image. Used for face anonymization
//! and text region obfuscation.

use image::{imageops, DynamicImage, RgbImage};

/// Apply Gaussian blur to a rectangular region within an image.
///
/// Extracts the sub-image, blurs it, and pastes it back. The original
/// image outside the region is unaffected.
pub fn blur_region(img: &mut RgbImage, x: u32, y: u32, width: u32, height: u32, sigma: f32) {
    if width == 0 || height == 0 {
        return;
    }
    let img_w = img.width();
    let img_h = img.height();
    // Clamp to image bounds
    let x = x.min(img_w.saturating_sub(1));
    let y = y.min(img_h.saturating_sub(1));
    let width = width.min(img_w - x);
    let height = height.min(img_h - y);
    if width == 0 || height == 0 {
        return;
    }

    // Extract the sub-image
    let sub = image::imageops::crop_imm(img, x, y, width, height).to_image();
    // Blur it (imageops::blur returns RgbaImage, convert back to Rgb)
    let blurred_rgba = imageops::blur(&DynamicImage::ImageRgb8(sub), sigma);
    let blurred_rgb = DynamicImage::ImageRgba8(blurred_rgba).to_rgb8();
    // Paste back
    imageops::overlay(img, &blurred_rgb, x as i64, y as i64);
}

/// Expand a bounding box by a margin percentage, clamped to image bounds.
///
/// A 15% margin ensures face blur covers hair/ears that may extend past the detection box.
pub fn expand_bbox(
    x_min: f32,
    y_min: f32,
    x_max: f32,
    y_max: f32,
    margin: f32,
    img_width: u32,
    img_height: u32,
) -> (u32, u32, u32, u32) {
    let w = x_max - x_min;
    let h = y_max - y_min;
    let dx = w * margin;
    let dy = h * margin;

    let x = (x_min - dx).max(0.0) as u32;
    let y = (y_min - dy).max(0.0) as u32;
    let x2 = ((x_max + dx) as u32).min(img_width);
    let y2 = ((y_max + dy) as u32).min(img_height);

    (x, y, x2.saturating_sub(x), y2.saturating_sub(y))
}

/// Apply Gaussian blur to a quadrilateral text region.
///
/// Approximates the quad with its axis-aligned bounding box for simplicity.
/// Sufficient for text region blurring where precision is less critical than coverage.
pub fn blur_quad_region(img: &mut RgbImage, points: &[(f32, f32); 4], sigma: f32) {
    let x_min = points.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
    let y_min = points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let x_max = points.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
    let y_max = points.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);

    // Expand vertically by 50% of text height for thicker blur coverage
    let text_h = y_max - y_min;
    let pad = text_h * 0.5;
    let y_min = (y_min - pad).max(0.0);
    let y_max = (y_max + pad).min(img.height() as f32);

    let x = x_min.max(0.0) as u32;
    let y = y_min as u32;
    let w = (x_max - x_min).max(0.0) as u32;
    let h = (y_max - y_min).max(0.0) as u32;

    blur_region(img, x, y, w, h, sigma);
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn solid_image(width: u32, height: u32, color: Rgb<u8>) -> RgbImage {
        RgbImage::from_pixel(width, height, color)
    }

    #[test]
    fn test_blur_region_modifies_pixels() {
        let mut img = solid_image(100, 100, Rgb([255, 0, 0]));
        // Place a white rectangle in the middle
        for x in 40..60 {
            for y in 40..60 {
                img.put_pixel(x, y, Rgb([255, 255, 255]));
            }
        }
        let original_pixel = *img.get_pixel(50, 50);
        blur_region(&mut img, 35, 35, 30, 30, 10.0);
        let blurred_pixel = *img.get_pixel(50, 50);
        // The pixel should have changed due to blur mixing with surrounding red
        assert_ne!(original_pixel, blurred_pixel);
    }

    #[test]
    fn test_blur_region_preserves_outside() {
        let mut img = solid_image(100, 100, Rgb([128, 128, 128]));
        blur_region(&mut img, 10, 10, 20, 20, 5.0);
        // Corner pixel should be unchanged
        assert_eq!(*img.get_pixel(0, 0), Rgb([128, 128, 128]));
        assert_eq!(*img.get_pixel(99, 99), Rgb([128, 128, 128]));
    }

    #[test]
    fn test_blur_region_preserves_dimensions() {
        let mut img = solid_image(200, 150, Rgb([0, 0, 0]));
        blur_region(&mut img, 0, 0, 200, 150, 5.0);
        assert_eq!(img.width(), 200);
        assert_eq!(img.height(), 150);
    }

    #[test]
    fn test_blur_region_zero_size_noop() {
        let mut img = solid_image(50, 50, Rgb([100, 100, 100]));
        let orig = img.clone();
        blur_region(&mut img, 10, 10, 0, 0, 5.0);
        assert_eq!(img, orig);
    }

    #[test]
    fn test_blur_region_clamps_to_bounds() {
        let mut img = solid_image(50, 50, Rgb([200, 200, 200]));
        // Region extends past image bounds — should not panic
        blur_region(&mut img, 40, 40, 100, 100, 5.0);
        assert_eq!(img.width(), 50);
    }

    #[test]
    fn test_expand_bbox_within_bounds() {
        let (x, y, w, h) = expand_bbox(100.0, 100.0, 200.0, 200.0, 0.15, 400, 400);
        assert!(x < 100);
        assert!(y < 100);
        assert!(w > 100);
        assert!(h > 100);
    }

    #[test]
    fn test_expand_bbox_clamps_to_image() {
        let (x, y, w, h) = expand_bbox(0.0, 0.0, 50.0, 50.0, 0.5, 60, 60);
        assert_eq!(x, 0);
        assert_eq!(y, 0);
        // Should not exceed image dimensions
        assert!(x + w <= 60);
        assert!(y + h <= 60);
    }

    #[test]
    fn test_blur_quad_region() {
        let mut img = solid_image(100, 100, Rgb([0, 0, 0]));
        // White rectangle
        for x in 20..80 {
            for y in 30..70 {
                img.put_pixel(x, y, Rgb([255, 255, 255]));
            }
        }
        let points = [(20.0, 30.0), (80.0, 30.0), (80.0, 70.0), (20.0, 70.0)];
        blur_quad_region(&mut img, &points, 10.0);
        // Center pixel should be affected by blur
        let p = *img.get_pixel(50, 50);
        // It should still be mostly white but possibly slightly changed at edges
        assert!(p[0] > 200); // Still bright
    }
}
