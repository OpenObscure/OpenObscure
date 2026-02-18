//! Face detection diagnostic — prints bbox details for an image.
//!
//! Usage:
//!   cargo run --example face_diag -- --input photo.jpg --models-dir models

use std::path::PathBuf;

use clap::Parser;
use image::GenericImageView;

use openobscure_proxy::face_detector::FaceDetector;
use openobscure_proxy::image_pipeline::decode_image;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    input: PathBuf,
    #[arg(long, default_value = "models")]
    models_dir: PathBuf,
}

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_target(false)
        .init();

    let args = Args::parse();
    let bytes = std::fs::read(&args.input).unwrap();
    let img = decode_image(&bytes).unwrap();
    let (w, h) = img.dimensions();
    let img_area = (w * h) as f32;
    println!("Image: {}x{} ({:.0} px²)", w, h, img_area);

    let face_dir = args.models_dir.join("blazeface");
    let mut detector = FaceDetector::load(&face_dir, 0.75).unwrap();
    let faces = detector.detect(&img).unwrap();

    println!("Faces detected: {}", faces.len());
    for (i, f) in faces.iter().enumerate() {
        let fw = f.x_max - f.x_min;
        let fh = f.y_max - f.y_min;
        let face_area = fw * fh;
        let ratio = face_area / img_area;
        println!(
            "  Face {}: bbox=({:.0},{:.0})→({:.0},{:.0})  size={:.0}x{:.0}  area={:.0}  ratio={:.1}%  conf={:.3}  {}",
            i,
            f.x_min, f.y_min, f.x_max, f.y_max,
            fw, fh,
            face_area,
            ratio * 100.0,
            f.confidence,
            if ratio > 0.8 { "→ FULL BLUR" } else { "→ SELECTIVE BLUR" }
        );
    }
}
