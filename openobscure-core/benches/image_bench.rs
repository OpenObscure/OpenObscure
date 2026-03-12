use criterion::{black_box, criterion_group, criterion_main, Criterion};
use image::{DynamicImage, Rgb, RgbImage};

fn create_test_image(width: u32, height: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([128, 64, 32])))
}

fn image_pipeline_benchmark(c: &mut Criterion) {
    // ── Image decode/encode ──────────────────────────────────────────

    c.bench_function("image_decode_png_256x256", |b| {
        let img = create_test_image(256, 256);
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let png_bytes = buf.into_inner();
        b.iter(|| {
            image::load_from_memory(black_box(&png_bytes)).unwrap();
        })
    });

    c.bench_function("image_decode_jpeg_640x480", |b| {
        let img = create_test_image(640, 480);
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Jpeg).unwrap();
        let jpeg_bytes = buf.into_inner();
        b.iter(|| {
            image::load_from_memory(black_box(&jpeg_bytes)).unwrap();
        })
    });

    c.bench_function("image_encode_jpeg_640x480", |b| {
        let img = create_test_image(640, 480);
        b.iter(|| {
            let mut buf = std::io::Cursor::new(Vec::new());
            black_box(&img)
                .write_to(&mut buf, image::ImageFormat::Jpeg)
                .unwrap();
        })
    });

    // ── Image resize ─────────────────────────────────────────────────

    c.bench_function("image_resize_1024_to_640", |b| {
        let img = create_test_image(1024, 768);
        b.iter(|| openobscure_core::image_pipeline::resize_if_needed(black_box(img.clone()), 640))
    });

    c.bench_function("image_resize_noop_small", |b| {
        let img = create_test_image(320, 240);
        b.iter(|| openobscure_core::image_pipeline::resize_if_needed(black_box(img.clone()), 640))
    });

    // ── Base64 detection ─────────────────────────────────────────────

    c.bench_function("base64_detect_anthropic_block", |b| {
        let img = create_test_image(10, 10);
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let b64 =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, buf.into_inner());
        let json = serde_json::json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": b64
            }
        });
        let map = json.as_object().unwrap();
        b.iter(|| {
            openobscure_core::image_detect::is_image_content_block(black_box(map));
        })
    });

    c.bench_function("base64_detect_non_image", |b| {
        let json = serde_json::json!({
            "type": "text",
            "text": "Hello, this is not an image block at all."
        });
        let map = json.as_object().unwrap();
        b.iter(|| {
            openobscure_core::image_detect::is_image_content_block(black_box(map));
        })
    });

    // ── Gaussian blur ────────────────────────────────────────────────

    c.bench_function("gaussian_blur_100x100", |b| {
        let img = create_test_image(100, 100);
        b.iter(|| image::imageops::blur(black_box(&img), 10.0))
    });

    c.bench_function("gaussian_blur_640x480", |b| {
        let img = create_test_image(640, 480);
        b.iter(|| image::imageops::blur(black_box(&img), 15.0))
    });
}

criterion_group!(benches, image_pipeline_benchmark);
criterion_main!(benches);
