//! OpenObscure Image Pipeline Demo
//!
//! Processes an image through the full visual PII pipeline:
//! face detection + blur, OCR text detection + blur, EXIF strip.
//!
//! Usage:
//!   # First download models
//!   ./scripts/download_models.sh
//!
//!   # Then run the demo
//!   cargo run --example demo_image_pipeline -- \
//!     --input photo.jpg --output photo-blurred.jpg
//!
//!   # Custom model directory
//!   cargo run --example demo_image_pipeline -- \
//!     --input photo.jpg --output photo-blurred.jpg \
//!     --models-dir /path/to/models

use std::path::PathBuf;
use std::time::Instant;

use clap::Parser;
use image::GenericImageView;

use openobscure_proxy::config::ImageConfig;
use openobscure_proxy::image_pipeline::{
    decode_image, encode_image, resize_if_needed, ImageModelManager, OutputFormat,
};

#[derive(Parser)]
#[command(name = "demo_image_pipeline", about = "OpenObscure image pipeline demo")]
struct Args {
    /// Input image path (JPEG, PNG, WebP)
    #[arg(long)]
    input: PathBuf,

    /// Output image path
    #[arg(long)]
    output: PathBuf,

    /// Base directory containing blazeface/ and paddleocr/ model subdirectories
    #[arg(long, default_value = "models")]
    models_dir: PathBuf,
}

fn main() {
    // Initialize minimal tracing so oo_info!/oo_warn! macros produce output
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .init();

    let args = Args::parse();

    // Read input image
    let input_bytes = std::fs::read(&args.input).unwrap_or_else(|e| {
        eprintln!("Failed to read {}: {}", args.input.display(), e);
        std::process::exit(1);
    });

    println!("Input:  {} ({} bytes)", args.input.display(), input_bytes.len());

    // Decode
    let img = decode_image(&input_bytes).unwrap_or_else(|e| {
        eprintln!("Failed to decode image: {}", e);
        std::process::exit(1);
    });

    let (w, h) = img.dimensions();
    println!("Size:   {}x{}", w, h);

    // Resize if needed (max 960px)
    let img = resize_if_needed(img, 960);
    let (w2, h2) = img.dimensions();
    if (w2, h2) != (w, h) {
        println!("Resize: {}x{} → {}x{}", w, h, w2, h2);
    }

    // Configure pipeline
    let face_dir = args.models_dir.join("blazeface");
    let ocr_dir = args.models_dir.join("paddleocr");

    let config = ImageConfig {
        enabled: true,
        face_detection: face_dir.exists(),
        ocr_enabled: ocr_dir.exists(),
        ocr_tier: "detect_and_blur".to_string(),
        max_dimension: 960,
        face_blur_sigma: 25.0,
        text_blur_sigma: 15.0,
        model_idle_timeout_secs: 300,
        face_model_dir: if face_dir.exists() {
            Some(face_dir.to_string_lossy().into_owned())
        } else {
            println!("Note:   BlazeFace models not found at {}", face_dir.display());
            None
        },
        ocr_model_dir: if ocr_dir.exists() {
            Some(ocr_dir.to_string_lossy().into_owned())
        } else {
            println!("Note:   PaddleOCR models not found at {}", ocr_dir.display());
            None
        },
        screen_guard: true,
        exif_strip: true,
    };

    // Process
    let manager = ImageModelManager::new(config);
    let start = Instant::now();

    let (result_img, stats) = manager.process_image(img).unwrap_or_else(|e| {
        eprintln!("Pipeline error: {}", e);
        std::process::exit(1);
    });

    let elapsed = start.elapsed();

    // Determine output format from extension
    let format = match args.output.extension().and_then(|e| e.to_str()) {
        Some("jpg") | Some("jpeg") => OutputFormat::Jpeg,
        Some("webp") => OutputFormat::WebP,
        Some("gif") => OutputFormat::Gif,
        _ => OutputFormat::Png,
    };

    // Encode and write
    let output_bytes = encode_image(&result_img, format).unwrap_or_else(|e| {
        eprintln!("Failed to encode output: {}", e);
        std::process::exit(1);
    });

    std::fs::write(&args.output, &output_bytes).unwrap_or_else(|e| {
        eprintln!("Failed to write {}: {}", args.output.display(), e);
        std::process::exit(1);
    });

    println!("");
    println!("=== Results ===");
    println!("Faces blurred:      {}", stats.faces_blurred);
    println!("Text regions found: {}", stats.text_regions_found);
    println!("Screenshot:         {}", stats.is_screenshot);
    println!("Processing time:    {:.0}ms", elapsed.as_millis());
    println!("Output: {} ({} bytes)", args.output.display(), output_bytes.len());
}
