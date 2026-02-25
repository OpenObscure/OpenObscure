//! Region redaction operations for face and text regions in images.
//!
//! Uses solid-color fill (irreversible) for elliptical, rectangular, or
//! quadrilateral regions. Used for face anonymization (elliptical),
//! text region obfuscation (rectangular/quad), and full-image NSFW redaction.

use image::{Rgb, RgbImage};

/// Light gray color used for solid face redaction: rgb(200, 200, 200).
pub const SOLID_FILL_COLOR: Rgb<u8> = Rgb([200, 200, 200]);

/// Fill a rectangular region with a solid color.
///
/// Replaces all pixels in the region with the given color.
/// Original pixel data is completely destroyed — not recoverable by AI.
pub fn solid_fill_region(
    img: &mut RgbImage,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: Rgb<u8>,
) {
    if width == 0 || height == 0 {
        return;
    }
    let img_w = img.width();
    let img_h = img.height();
    let x = x.min(img_w.saturating_sub(1));
    let y = y.min(img_h.saturating_sub(1));
    let width = width.min(img_w - x);
    let height = height.min(img_h - y);
    if width == 0 || height == 0 {
        return;
    }

    for py in y..y + height {
        for px in x..x + width {
            img.put_pixel(px, py, color);
        }
    }
}

/// Fill an elliptical region with a solid color.
///
/// Fills pixels inside an ellipse inscribed within the bounding rectangle.
/// Pixels outside the ellipse keep the original. Original pixel data inside
/// the ellipse is completely destroyed — not recoverable by AI.
pub fn solid_fill_region_elliptical(
    img: &mut RgbImage,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: Rgb<u8>,
) {
    if width == 0 || height == 0 {
        return;
    }
    let img_w = img.width();
    let img_h = img.height();
    let x = x.min(img_w.saturating_sub(1));
    let y = y.min(img_h.saturating_sub(1));
    let width = width.min(img_w - x);
    let height = height.min(img_h - y);
    if width == 0 || height == 0 {
        return;
    }

    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let rx = cx;
    let ry = cy;

    for py in 0..height {
        for px in 0..width {
            let dx = (px as f32 - cx) / rx;
            let dy = (py as f32 - cy) / ry;
            let dist_sq = dx * dx + dy * dy;

            if dist_sq <= 1.0 {
                img.put_pixel(x + px, y + py, color);
            }
        }
    }
}

/// Expand a bounding box by a margin percentage, clamped to image bounds.
///
/// A 15% margin ensures face redaction covers hair/ears that may extend past the detection box.
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

/// Fill a quadrilateral text region with a solid color.
///
/// Approximates the quad with its axis-aligned bounding box for simplicity.
/// Sufficient for text region redaction where precision is less critical than coverage.
/// Expands vertically by 50% of text height for thicker coverage.
pub fn solid_fill_quad_region(img: &mut RgbImage, points: &[(f32, f32); 4], color: Rgb<u8>) {
    let x_min = points.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
    let y_min = points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let x_max = points.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
    let y_max = points.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);

    // Expand vertically by 50% of text height for thicker fill coverage
    let text_h = y_max - y_min;
    let pad = text_h * 0.5;
    let y_min = (y_min - pad).max(0.0);
    let y_max = (y_max + pad).min(img.height() as f32);

    let x = x_min.max(0.0) as u32;
    let y = y_min as u32;
    let w = (x_max - x_min).max(0.0) as u32;
    let h = (y_max - y_min).max(0.0) as u32;

    solid_fill_region(img, x, y, w, h, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn solid_image(width: u32, height: u32, color: Rgb<u8>) -> RgbImage {
        RgbImage::from_pixel(width, height, color)
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

    // --- Solid fill tests ---

    #[test]
    fn test_solid_fill_region_fills_with_color() {
        let mut img = solid_image(100, 100, Rgb([0, 0, 0]));
        let fill = Rgb([200, 200, 200]);
        solid_fill_region(&mut img, 10, 10, 30, 30, fill);
        // Center of filled region should be the fill color
        assert_eq!(*img.get_pixel(25, 25), fill);
        // Just inside the fill boundary
        assert_eq!(*img.get_pixel(10, 10), fill);
        assert_eq!(*img.get_pixel(39, 39), fill);
    }

    #[test]
    fn test_solid_fill_region_preserves_outside() {
        let original = Rgb([50, 100, 150]);
        let mut img = solid_image(100, 100, original);
        solid_fill_region(&mut img, 20, 20, 20, 20, Rgb([200, 200, 200]));
        // Pixels outside the region are untouched
        assert_eq!(*img.get_pixel(0, 0), original);
        assert_eq!(*img.get_pixel(99, 99), original);
        assert_eq!(*img.get_pixel(19, 19), original);
    }

    #[test]
    fn test_solid_fill_region_zero_size_noop() {
        let mut img = solid_image(50, 50, Rgb([100, 100, 100]));
        let orig = img.clone();
        solid_fill_region(&mut img, 10, 10, 0, 0, Rgb([200, 200, 200]));
        assert_eq!(img, orig);
    }

    #[test]
    fn test_solid_fill_region_clamps_to_bounds() {
        let mut img = solid_image(50, 50, Rgb([0, 0, 0]));
        // Should not panic even if region exceeds image
        solid_fill_region(&mut img, 40, 40, 100, 100, Rgb([200, 200, 200]));
        assert_eq!(img.width(), 50);
        // Corner pixel within the fill region should be filled
        assert_eq!(*img.get_pixel(49, 49), Rgb([200, 200, 200]));
    }

    #[test]
    fn test_solid_fill_elliptical_fills_center() {
        let mut img = solid_image(100, 100, Rgb([0, 0, 0]));
        let fill = Rgb([200, 200, 200]);
        solid_fill_region_elliptical(&mut img, 10, 10, 80, 80, fill);
        // Center of ellipse should be fill color
        assert_eq!(*img.get_pixel(50, 50), fill);
    }

    #[test]
    fn test_solid_fill_elliptical_preserves_corners() {
        let original = Rgb([0, 0, 0]);
        let mut img = solid_image(100, 100, original);
        solid_fill_region_elliptical(&mut img, 0, 0, 100, 100, Rgb([200, 200, 200]));
        // Corners of bounding box are outside the inscribed ellipse
        assert_eq!(*img.get_pixel(0, 0), original);
        assert_eq!(*img.get_pixel(99, 0), original);
        assert_eq!(*img.get_pixel(0, 99), original);
        assert_eq!(*img.get_pixel(99, 99), original);
    }

    #[test]
    fn test_solid_fill_elliptical_zero_size_noop() {
        let mut img = solid_image(50, 50, Rgb([100, 100, 100]));
        let orig = img.clone();
        solid_fill_region_elliptical(&mut img, 10, 10, 0, 0, Rgb([200, 200, 200]));
        assert_eq!(img, orig);
    }

    #[test]
    fn test_solid_fill_destroys_original_data() {
        // Verify that original pixel data is completely replaced (not blended)
        let mut img = solid_image(100, 100, Rgb([255, 0, 0])); // Red image
        let fill = Rgb([200, 200, 200]);
        solid_fill_region(&mut img, 0, 0, 100, 100, fill);
        // Every pixel should be exactly the fill color — no trace of red
        for y in 0..100 {
            for x in 0..100 {
                assert_eq!(*img.get_pixel(x, y), fill);
            }
        }
    }

    // --- Solid fill quad region tests ---

    #[test]
    fn test_solid_fill_quad_region_fills_bbox() {
        let mut img = solid_image(100, 100, Rgb([0, 0, 0]));
        let fill = Rgb([200, 200, 200]);
        let points = [(20.0, 40.0), (80.0, 40.0), (80.0, 60.0), (20.0, 60.0)];
        solid_fill_quad_region(&mut img, &points, fill);
        // Center of the text region should be filled
        assert_eq!(*img.get_pixel(50, 50), fill);
        // Outside the region horizontally should be original
        assert_eq!(*img.get_pixel(5, 50), Rgb([0, 0, 0]));
    }

    #[test]
    fn test_solid_fill_quad_region_vertical_padding() {
        let mut img = solid_image(100, 100, Rgb([0, 0, 0]));
        let fill = Rgb([200, 200, 200]);
        // Text region from y=40 to y=60 (height=20), so 50% padding = 10px each side
        let points = [(10.0, 40.0), (90.0, 40.0), (90.0, 60.0), (10.0, 60.0)];
        solid_fill_quad_region(&mut img, &points, fill);
        // y=35 should be within the padded region (40 - 10 = 30)
        assert_eq!(*img.get_pixel(50, 35), fill);
        // y=65 should be within the padded region (60 + 10 = 70)
        assert_eq!(*img.get_pixel(50, 65), fill);
    }

    #[test]
    fn test_solid_fill_quad_region_clamps_to_image() {
        let mut img = solid_image(50, 50, Rgb([0, 0, 0]));
        let fill = Rgb([200, 200, 200]);
        // Region near bottom edge — vertical padding should clamp
        let points = [(0.0, 40.0), (50.0, 40.0), (50.0, 50.0), (0.0, 50.0)];
        solid_fill_quad_region(&mut img, &points, fill);
        // Should not panic, image dimensions preserved
        assert_eq!(img.width(), 50);
        assert_eq!(img.height(), 50);
    }
}
