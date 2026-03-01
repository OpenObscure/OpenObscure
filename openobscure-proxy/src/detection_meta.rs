//! Detection metadata structs for verification.
//!
//! Collected during inference (+10-20ms overhead) to enable blazing-fast
//! pure-logic validation in tests without re-running models.

/// Metadata for any bounding-box detection (face, text region, NSFW region).
#[derive(Debug, Clone)]
pub struct BboxMeta {
    pub x_min: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub y_max: f32,
    pub confidence: f32,
    pub img_width: u32,
    pub img_height: u32,
    pub label: String,
}

impl BboxMeta {
    /// Bounding box width in pixels.
    pub fn width(&self) -> f32 {
        self.x_max - self.x_min
    }

    /// Bounding box height in pixels.
    pub fn height(&self) -> f32 {
        self.y_max - self.y_min
    }

    /// Bounding box area in pixels².
    pub fn area(&self) -> f32 {
        self.width() * self.height()
    }

    /// Image area in pixels².
    pub fn img_area(&self) -> f32 {
        (self.img_width * self.img_height) as f32
    }

    /// Ratio of bbox area to image area.
    pub fn area_ratio(&self) -> f32 {
        let img = self.img_area();
        if img <= 0.0 {
            return 0.0;
        }
        self.area() / img
    }

    /// Aspect ratio (width / height).
    pub fn aspect_ratio(&self) -> f32 {
        let h = self.height();
        if h <= 0.0 {
            return 0.0;
        }
        self.width() / h
    }
}

/// NSFW detection metadata.
#[derive(Debug, Clone)]
pub struct NsfwMeta {
    pub is_nsfw: bool,
    pub confidence: f32,
    pub threshold: f32,
    pub category: Option<String>,
    /// Top exposed class scores for diagnostics.
    pub exposed_scores: Vec<(String, f32)>,
    /// Holistic classifier NSFW score (None if classifier not run or not available).
    pub classifier_score: Option<f32>,
}

/// Screenshot detection metadata.
#[derive(Debug, Clone, Default)]
pub struct ScreenshotMeta {
    pub is_screenshot: bool,
    pub resolution_match: bool,
    pub status_bar_variance: Option<f64>,
    pub exif_software: Option<String>,
    pub reason_count: usize,
}

/// Aggregate metadata from a full pipeline run.
#[derive(Debug, Clone, Default)]
pub struct PipelineMeta {
    pub image_size: (u32, u32),
    pub nsfw: Option<NsfwMeta>,
    pub faces: Vec<BboxMeta>,
    pub text_regions: Vec<BboxMeta>,
    pub screenshot: ScreenshotMeta,
}
