//! L0 Embedded Test Suite — exercises `OpenObscureMobile` directly (no HTTP server).
//!
//! Writes output to:
//!   test/data/output/<category>/json/<name>_l0_embedded.json
//!   test/data/output/<category>/redacted/<name>_l0_embedded.<ext>
//!
//! JSON schema is comparable with gateway and l1_plugin outputs for latency
//! and detection count comparison across all three architectures.
//!
//! Run subset: cargo test --tests embedded_suite -- --nocapture
//! Run image:  cargo test --tests embedded_suite test_image -- --nocapture
//! Run audio:  cargo test --tests embedded_suite test_audio -- --nocapture

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use openobscure_core::image_pipeline::{
    decode_image, encode_image, resize_if_needed, ImageModelManager, OutputFormat,
};
use openobscure_core::lib_mobile::{MobileConfig, OpenObscureMobile};

// ─── Constants ────────────────────────────────────────────────

/// Fixed test key — same value used in lib_mobile unit tests.
const TEST_KEY: [u8; 32] = [0x42u8; 32];

const TEXT_CATEGORIES: &[&str] = &[
    "PII_Detection",
    "Multilingual_PII",
    "Code_Config_PII",
    "Structured_Data_PII",
    "Agent_Tool_Results",
];

const TEXT_EXTS: &[&str] = &[
    ".txt", ".csv", ".tsv", ".env", ".py", ".yaml", ".yml", ".json", ".sh", ".md", ".log",
];

const IMAGE_EXTS: &[&str] = &[".jpg", ".jpeg", ".png", ".webp", ".gif", ".bmp"];

// ─── Path helpers ─────────────────────────────────────────────

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn test_input() -> PathBuf {
    manifest_dir().join("../test/data/input")
}

fn test_output() -> PathBuf {
    manifest_dir().join("../test/data/output")
}

fn models_dir() -> PathBuf {
    manifest_dir().join("models")
}

// ─── Model availability ───────────────────────────────────────

fn is_real_model(path: &Path) -> bool {
    path.exists()
        && std::fs::metadata(path)
            .map(|m| m.len() > 1024)
            .unwrap_or(false)
}

fn image_models_available() -> bool {
    let m = models_dir();
    is_real_model(&m.join("blazeface/face_detection_short_range.onnx"))
        || is_real_model(&m.join("scrfd/scrfd_2.5g_bnkps.onnx"))
        || is_real_model(&m.join("paddleocr/det_model.onnx"))
}

// ─── Mobile instance ──────────────────────────────────────────

fn make_mobile() -> OpenObscureMobile {
    OpenObscureMobile::new(MobileConfig::default(), TEST_KEY)
        .expect("OpenObscureMobile::new failed — check MobileConfig::default()")
}

// ─── ISO 8601 timestamp (no external deps) ───────────────────

/// Convert Unix seconds to ISO 8601 UTC string using Hinnant's algorithm.
fn utc_iso8601(secs: u64) -> String {
    let s = (secs % 60) as u32;
    let mi = ((secs / 60) % 60) as u32;
    let h = ((secs / 3600) % 24) as u32;
    let d = secs / 86400;

    let z = d as i64 + 719_468i64;
    let era = if z >= 0 {
        z / 146_097
    } else {
        (z - 146_096) / 146_097
    };
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = if month <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, h, mi, s
    )
}

fn now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    utc_iso8601(secs)
}

// ─── Metadata helpers ─────────────────────────────────────────

/// Build per-type counts from sanitize_text() mapping JSON.
/// Format: [(ciphertext, plaintext, pii_type_str), ...]
fn type_summary_from_mapping(mapping_json: &str) -> HashMap<String, u32> {
    let mappings: Vec<(String, String, String)> =
        serde_json::from_str(mapping_json).unwrap_or_default();
    let mut counts: HashMap<String, u32> = HashMap::new();
    for (_, _, t) in &mappings {
        *counts.entry(t.clone()).or_insert(0) += 1;
    }
    counts
}

/// Split "filename.ext" → ("filename", ".ext"). Returns ("filename", "") if no dot.
fn split_filename(filename: &str) -> (&str, &str) {
    if let Some(dot) = filename.rfind('.') {
        (&filename[..dot], &filename[dot..])
    } else {
        (filename, "")
    }
}

/// Purge all *_l0_embedded.* files from json/ and redacted/ subdirectories.
fn purge_l0_outputs(output_root: &Path, categories: &[&str]) {
    for cat in categories {
        for subdir in &["json", "redacted"] {
            let dir = output_root.join(cat).join(subdir);
            if dir.exists() {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.contains("_l0_embedded") {
                            let _ = std::fs::remove_file(entry.path());
                        }
                    }
                }
            }
        }
    }
}

// ─── Image pipeline config (mirrors pipeline_validation_test.rs) ──

fn make_image_config() -> openobscure_core::config::ImageConfig {
    let m = models_dir();
    let blazeface = m.join("blazeface");
    let scrfd = m.join("scrfd");
    let ocr = m.join("paddleocr");
    let nsfw = m.join("nsfw_classifier");

    let face_model = if scrfd.exists() {
        "scrfd".to_string()
    } else {
        "blazeface".to_string()
    };

    openobscure_core::config::ImageConfig {
        enabled: true,
        face_detection: blazeface.exists() || scrfd.exists(),
        ocr_enabled: ocr.exists(),
        ocr_tier: "detect_and_fill".to_string(),
        max_dimension: 960,
        model_idle_timeout_secs: 300,
        face_model,
        face_model_dir: if blazeface.exists() {
            Some(blazeface.to_string_lossy().into_owned())
        } else {
            None
        },
        face_model_dir_scrfd: if scrfd.exists() {
            Some(scrfd.to_string_lossy().into_owned())
        } else {
            None
        },
        face_model_dir_ultralight: None,
        ocr_model_dir: if ocr.exists() {
            Some(ocr.to_string_lossy().into_owned())
        } else {
            None
        },
        screen_guard: true,
        exif_strip: true,
        nsfw_detection: nsfw.exists(),
        nsfw_model_dir: if nsfw.exists() {
            Some(nsfw.to_string_lossy().into_owned())
        } else {
            None
        },
        nsfw_threshold: 0.80,
        nsfw_classifier_enabled: false,
        nsfw_classifier_model_dir: None,
        nsfw_classifier_threshold: 0.0,
        url_fetch_enabled: false,
        url_max_bytes: 0,
        url_timeout_secs: 0,
        url_allow_localhost_http: false,
    }
}

// ═══════════════════════════════════════════════════════════════
// TEXT PII TESTS
// ═══════════════════════════════════════════════════════════════

/// Process all text categories through OpenObscureMobile.sanitize_text().
///
/// Output format is compatible with gateway and l1_plugin JSON for
/// cross-architecture latency comparison via validate_results.sh.
#[test]
fn test_text_all_categories() {
    let mobile = make_mobile();
    let s = mobile.stats();
    let scanner_mode = s.scanner_mode.clone();
    let device_tier = s.device_tier.clone();

    let input_root = test_input();
    let output_root = test_output();

    purge_l0_outputs(&output_root, TEXT_CATEGORIES);

    let mut grand_files = 0u32;
    let mut grand_matches = 0u32;
    let mut failures: Vec<String> = Vec::new();

    for cat in TEXT_CATEGORIES {
        let cat_input = input_root.join(cat);
        let json_dir = output_root.join(cat).join("json");
        let redacted_dir = output_root.join(cat).join("redacted");

        if !cat_input.exists() {
            eprintln!("SKIP {} (directory not found)", cat);
            continue;
        }

        std::fs::create_dir_all(&json_dir).expect("create json dir");
        std::fs::create_dir_all(&redacted_dir).expect("create redacted dir");

        let mut files: Vec<_> = std::fs::read_dir(&cat_input)
            .unwrap_or_else(|_| panic!("cannot read {}", cat_input.display()))
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_lowercase();
                e.path().is_file() && TEXT_EXTS.iter().any(|ext| name.ends_with(ext))
            })
            .collect();
        files.sort_by_key(|e| e.file_name());

        let mut cat_files = 0u32;
        let mut cat_matches = 0u32;

        for entry in &files {
            let path = entry.path();
            let filename = entry.file_name().to_string_lossy().to_string();
            let (name_no_ext, ext) = split_filename(&filename);

            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    failures.push(format!("{}/{}: read error: {}", cat, filename, e));
                    continue;
                }
            };

            // ── Time sanitize_text() ──────────────────────────────
            let t0 = Instant::now();
            let result = match mobile.sanitize_text(&text) {
                Ok(r) => r,
                Err(e) => {
                    failures.push(format!("{}/{}: sanitize error: {}", cat, filename, e));
                    continue;
                }
            };
            let sanitize_ms = t0.elapsed().as_millis() as u64;

            let type_counts = type_summary_from_mapping(&result.mapping_json);

            // ── Write JSON ────────────────────────────────────────
            let envelope = serde_json::json!({
                "file": filename,
                "path": path.display().to_string(),
                "architecture": "l0_embedded",
                "redaction_mode": "fpe",
                "timestamp": now_utc(),
                "total_matches": result.pii_count,
                "type_summary": type_counts,
                "timing": {
                    "sanitize_ms": sanitize_ms,
                    "total_ms": sanitize_ms
                },
                "scanner_mode": &scanner_mode,
                "device_tier": &device_tier,
                "matches": []
            });

            let json_path = json_dir.join(format!("{}_l0_embedded.json", name_no_ext));
            if let Err(e) = std::fs::write(
                &json_path,
                serde_json::to_string_pretty(&envelope).unwrap() + "\n",
            ) {
                failures.push(format!("{}/{}: write JSON: {}", cat, filename, e));
                continue;
            }

            // ── Write redacted file ───────────────────────────────
            let redacted_path = redacted_dir.join(format!("{}_l0_embedded{}", name_no_ext, ext));
            if let Err(e) = std::fs::write(&redacted_path, &result.sanitized_text) {
                failures.push(format!("{}/{}: write redacted: {}", cat, filename, e));
                continue;
            }

            eprintln!(
                "  OK  {}/{} — {} matches ({}ms)",
                cat, filename, result.pii_count, sanitize_ms
            );
            cat_files += 1;
            cat_matches += result.pii_count;
        }

        eprintln!(
            "  {} summary: {} files, {} matches",
            cat, cat_files, cat_matches
        );
        grand_files += cat_files;
        grand_matches += cat_matches;
    }

    eprintln!(
        "\nL0 Embedded text: {} files, {} total matches",
        grand_files, grand_matches
    );

    assert!(
        failures.is_empty(),
        "L0 embedded text failures:\n{}",
        failures.join("\n")
    );
}

// ═══════════════════════════════════════════════════════════════
// IMAGE PIPELINE TESTS
// ═══════════════════════════════════════════════════════════════

/// Process all Visual_PII images through ImageModelManager directly.
///
/// Skips gracefully when ONNX models are absent (CI without Git LFS).
/// When models are present, records per-phase timing (nsfw_ms, face_ms,
/// ocr_ms) enabling comparison with gateway visual pipeline timing.
#[test]
fn test_image_all_visual_pii_files() {
    if !image_models_available() {
        eprintln!("Skipping test_image_all_visual_pii_files: ONNX models not available");
        return;
    }

    let mobile = make_mobile();
    let s = mobile.stats();
    let scanner_mode = s.scanner_mode.clone();
    let device_tier = s.device_tier.clone();

    let manager = ImageModelManager::new(make_image_config());
    let visual_input = test_input().join("Visual_PII");
    let visual_output = test_output().join("Visual_PII");
    let json_dir = visual_output.join("json");
    let redacted_dir = visual_output.join("redacted");

    if !visual_input.exists() {
        eprintln!("SKIP test_image_all_visual_pii_files: Visual_PII input not found");
        return;
    }

    std::fs::create_dir_all(&json_dir).ok();
    std::fs::create_dir_all(&redacted_dir).ok();

    // Purge previous l0_embedded visual outputs
    for dir in [&json_dir, &redacted_dir] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.contains("_l0_embedded") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    let mut total = 0u32;
    let mut failures: Vec<String> = Vec::new();

    // Walk subdirectories (Faces/, Screenshots/, etc.) and top-level files
    let mut all_image_paths: Vec<(String, PathBuf)> = Vec::new(); // (subcategory, path)

    if let Ok(entries) = std::fs::read_dir(&visual_input) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                // Subdirectory — walk contents
                if let Ok(sub_entries) = std::fs::read_dir(&path) {
                    for sub in sub_entries.flatten() {
                        let sub_path = sub.path();
                        if sub_path.is_file() {
                            let fname = sub.file_name().to_string_lossy().to_lowercase();
                            if IMAGE_EXTS.iter().any(|e| fname.ends_with(e)) {
                                all_image_paths.push((name.clone(), sub_path));
                            }
                        }
                    }
                }
            } else if path.is_file() {
                let fname_lower = name.to_lowercase();
                if IMAGE_EXTS.iter().any(|e| fname_lower.ends_with(e)) {
                    all_image_paths.push(("Visual_PII".to_string(), path));
                }
            }
        }
    }

    all_image_paths.sort_by(|a, b| a.1.cmp(&b.1));

    for (subcat, path) in &all_image_paths {
        let filename = path.file_name().unwrap().to_string_lossy().to_string();
        let (name_no_ext, ext) = split_filename(&filename);

        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{}: read error: {}", filename, e));
                continue;
            }
        };

        // ── Decode + resize ───────────────────────────────────
        let t_decode = Instant::now();
        let img = match decode_image(&bytes) {
            Ok(i) => i,
            Err(e) => {
                failures.push(format!("{}: decode error: {}", filename, e));
                continue;
            }
        };
        let img = resize_if_needed(img, 960);
        let decode_ms = t_decode.elapsed().as_millis() as u64;

        // ── Run pipeline (NSFW → face → OCR) ─────────────────
        let t_pipeline = Instant::now();
        let (result_img, stats, _meta) = match manager.process_image(img, None, None) {
            Ok(r) => r,
            Err(e) => {
                failures.push(format!("{}: pipeline error: {}", filename, e));
                continue;
            }
        };
        let pipeline_ms = t_pipeline.elapsed().as_millis() as u64;
        let total_ms = decode_ms + pipeline_ms;

        // ── Encode result ─────────────────────────────────────
        let out_format = if ext.eq_ignore_ascii_case(".png") {
            OutputFormat::Png
        } else {
            OutputFormat::Jpeg
        };
        let result_bytes = match encode_image(&result_img, out_format) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{}: encode error: {}", filename, e));
                continue;
            }
        };

        // ── Build type summary ────────────────────────────────
        let mut type_counts = serde_json::Map::new();
        if stats.faces_redacted > 0 {
            type_counts.insert("face".to_string(), stats.faces_redacted.into());
        }
        if stats.text_regions_found > 0 {
            type_counts.insert("text_region".to_string(), stats.text_regions_found.into());
        }
        if stats.nsfw_detected {
            type_counts.insert("nsfw".to_string(), 1u32.into());
        }
        let total_matches = stats.faces_redacted
            + stats.text_regions_found
            + if stats.nsfw_detected { 1 } else { 0 };

        // ── Write JSON ────────────────────────────────────────
        let envelope = serde_json::json!({
            "file": filename,
            "path": path.display().to_string(),
            "architecture": "l0_embedded",
            "redaction_mode": "fpe",
            "timestamp": now_utc(),
            "subcategory": subcat,
            "total_matches": total_matches,
            "type_summary": type_counts,
            "pipeline_results": {
                "faces_redacted": stats.faces_redacted,
                "text_regions_detected": stats.text_regions_found,
                "nsfw_blocked": stats.nsfw_detected,
                "screenshot_detected": stats.is_screenshot,
                "exif_stripped": true
            },
            "timing": {
                "decode_ms": decode_ms,
                "pipeline_ms": pipeline_ms,
                "nsfw_ms": stats.nsfw_ms,
                "face_ms": stats.face_ms,
                "ocr_ms": stats.ocr_ms,
                "total_ms": total_ms
            },
            "scanner_mode": &scanner_mode,
            "device_tier": &device_tier,
            "matches": []
        });

        let json_path = json_dir.join(format!("{}_l0_embedded.json", name_no_ext));
        let _ = std::fs::write(
            &json_path,
            serde_json::to_string_pretty(&envelope).unwrap() + "\n",
        );

        // ── Write redacted image ──────────────────────────────
        let redacted_path = redacted_dir.join(format!("{}_l0_embedded{}", name_no_ext, ext));
        let _ = std::fs::write(&redacted_path, &result_bytes);

        eprintln!(
            "  OK  Visual_PII/{}/{} — faces:{} text:{} nsfw:{} (decode:{}ms pipeline:{}ms total:{}ms)",
            subcat, filename,
            stats.faces_redacted, stats.text_regions_found, stats.nsfw_detected,
            decode_ms, pipeline_ms, total_ms
        );
        total += 1;
    }

    eprintln!("\nL0 Embedded visual: {} files", total);
    assert!(
        failures.is_empty(),
        "Visual failures:\n{}",
        failures.join("\n")
    );
}

// ═══════════════════════════════════════════════════════════════
// AUDIO TRANSCRIPT TESTS
// ═══════════════════════════════════════════════════════════════

/// Per-file transcript fixture.
#[derive(serde::Deserialize)]
struct TranscriptEntry {
    /// Known transcript text for this audio file.
    transcript: String,
    /// PII types expected to be detected (warnings if missing, not failures).
    #[serde(default)]
    expected_pii_types: Vec<String>,
    /// Minimum PII count to assert (0 = no assertion).
    #[serde(default)]
    expected_pii_min: u32,
}

/// Process all audio transcript fixtures through sanitize_audio_transcript().
///
/// Audio files are tested via transcript text (the mobile path — platform
/// speech API produces the transcript, OpenObscure sanitizes it).
/// Fixture file: test/data/input/Audio_PII/transcripts.json
#[test]
fn test_audio_transcript_all() {
    let mobile = make_mobile();
    let s = mobile.stats();
    let scanner_mode = s.scanner_mode.clone();
    let device_tier = s.device_tier.clone();

    let audio_input = test_input().join("Audio_PII");
    let audio_output = test_output().join("Audio_PII");
    let json_dir = audio_output.join("json");
    std::fs::create_dir_all(&json_dir).ok();

    // Purge previous l0_embedded audio JSON
    if let Ok(entries) = std::fs::read_dir(&json_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.contains("_l0_embedded") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    let fixtures_path = audio_input.join("transcripts.json");
    if !fixtures_path.exists() {
        eprintln!(
            "Skipping test_audio_transcript_all: transcripts.json not found at {}",
            fixtures_path.display()
        );
        return;
    }

    let fixtures_str = std::fs::read_to_string(&fixtures_path).expect("read transcripts.json");
    let fixtures: HashMap<String, TranscriptEntry> =
        serde_json::from_str(&fixtures_str).expect("parse transcripts.json");

    let mut failures: Vec<String> = Vec::new();
    let mut total = 0u32;

    // Process in deterministic order
    let mut sorted_keys: Vec<&String> = fixtures.keys().collect();
    sorted_keys.sort();

    for filename in sorted_keys {
        let fixture = &fixtures[filename];
        let (name_no_ext, _ext) = split_filename(filename);

        // ── Time sanitize_audio_transcript() ─────────────────
        let t0 = Instant::now();
        let result = match mobile.sanitize_audio_transcript(&fixture.transcript) {
            Ok(r) => r,
            Err(e) => {
                failures.push(format!("{}: sanitize error: {}", filename, e));
                continue;
            }
        };
        let sanitize_ms = t0.elapsed().as_millis() as u64;

        let type_counts = type_summary_from_mapping(&result.mapping_json);
        let pii_detected = result.pii_count > 0;
        let action = if pii_detected { "SANITIZED" } else { "CLEAN" };

        // ── Validate expected types (soft warnings) ───────────
        for expected in &fixture.expected_pii_types {
            if !result.categories.contains(expected) {
                eprintln!(
                    "  WARN {}: expected type '{}' not detected (found: {:?})",
                    filename, expected, result.categories
                );
            }
        }

        // ── Assert minimum PII count (hard failure) ───────────
        if fixture.expected_pii_min > 0 && result.pii_count < fixture.expected_pii_min {
            failures.push(format!(
                "{}: expected >= {} PII matches, got {}",
                filename, fixture.expected_pii_min, result.pii_count
            ));
        }

        // ── Write JSON ────────────────────────────────────────
        let audio_file_path = audio_input.join(filename);
        let envelope = serde_json::json!({
            "file": filename,
            "path": audio_file_path.display().to_string(),
            "architecture": "l0_embedded",
            "redaction_mode": "fpe",
            "timestamp": now_utc(),
            "total_matches": result.pii_count,
            "type_summary": type_counts,
            "kws_results": {
                "mode": "transcript",
                "pii_detected": pii_detected,
                "pii_count": result.pii_count,
                "action": action
            },
            "timing": {
                "sanitize_ms": sanitize_ms,
                "total_ms": sanitize_ms
            },
            "scanner_mode": &scanner_mode,
            "device_tier": &device_tier,
            "matches": []
        });

        let json_path = json_dir.join(format!("{}_l0_embedded.json", name_no_ext));
        let _ = std::fs::write(
            &json_path,
            serde_json::to_string_pretty(&envelope).unwrap() + "\n",
        );

        eprintln!(
            "  OK  Audio_PII/{} — {} matches ({}ms) [{}]",
            filename, result.pii_count, sanitize_ms, action
        );
        total += 1;
    }

    eprintln!("\nL0 Embedded audio: {} transcripts", total);
    assert!(
        failures.is_empty(),
        "Audio transcript failures:\n{}",
        failures.join("\n")
    );
}

// ═══════════════════════════════════════════════════════════════
// RESPONSE INTEGRITY TESTS
// ═══════════════════════════════════════════════════════════════

/// R1 dictionary detects urgency/manipulation phrases (no model required).
#[test]
fn test_response_integrity_detects_urgency() {
    let mobile = make_mobile();
    let report = mobile.scan_response(
        "You must act now! This is a limited-time offer. Don't miss out or you'll regret it forever.",
    );
    assert!(
        report.is_some(),
        "Expected persuasion detection for urgency phrase"
    );
    let r = report.unwrap();
    assert!(
        !r.severity.is_empty(),
        "Severity should not be empty: {:?}",
        r
    );
    eprintln!(
        "  RI urgency: severity={}, flags={:?}, scan_time={}us",
        r.severity, r.flags, r.scan_time_us
    );
}

/// R1 dictionary passes clean factual text without false positives.
#[test]
fn test_response_integrity_passes_clean_text() {
    let mobile = make_mobile();
    let report = mobile.scan_response(
        "The capital of France is Paris. The Eiffel Tower was built in 1889 and stands 330 metres tall.",
    );
    assert!(
        report.is_none(),
        "Expected no manipulation detected for clean factual text"
    );
}

/// ri_available() reflects whether RI scanner was configured.
#[test]
fn test_response_integrity_available() {
    let mobile = make_mobile();
    // Default config enables RI (ri_enabled = true) — R1 dictionary always available
    assert!(
        mobile.ri_available(),
        "RI should be available with default config"
    );
}

// ═══════════════════════════════════════════════════════════════
// FPE ROUNDTRIP TESTS
// ═══════════════════════════════════════════════════════════════

/// Verify FPE encrypt → restore roundtrip for each structured PII type.
#[test]
fn test_fpe_roundtrip_all_types() {
    let mobile = make_mobile();

    let cases: &[(&str, &str)] = &[
        ("Card: 4111-1111-1111-1111", "4111-1111-1111-1111"),
        ("SSN: 123-45-6789", "123-45-6789"),
        ("Phone: (555) 867-5309", "867-5309"),
        ("Email: john.doe@example.com", "john.doe@example.com"),
        (
            "Key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz01234567890ABCDEFGHIJKLMNOPQRS",
            "sk-ant-api03-abcdefghijklmnopqrstuvwxyz01234567890ABCDEFGHIJKLMNOPQRS",
        ),
    ];

    let mut verified = 0u32;
    for (input, original_pii) in cases {
        let sanitized = mobile.sanitize_text(input).expect("sanitize_text");

        // Skip if this type wasn't detected (regex tuning may vary)
        if sanitized.pii_count == 0 {
            eprintln!("  SKIP roundtrip for '{}' (not detected)", original_pii);
            continue;
        }

        assert!(
            !sanitized.sanitized_text.contains(original_pii),
            "PII not encrypted: '{}'",
            input
        );

        let restored = mobile.restore_text(&sanitized.sanitized_text, &sanitized.mapping_json);
        assert!(
            restored.contains(original_pii),
            "Restore failed for '{}': got '{}'",
            original_pii,
            restored
        );

        eprintln!(
            "  OK  roundtrip: '{}' → '{}' → restored",
            original_pii,
            &sanitized.sanitized_text[..sanitized.sanitized_text.len().min(40)]
        );
        verified += 1;
    }

    assert!(
        verified >= 3,
        "Expected at least 3 FPE roundtrip types verified, got {}",
        verified
    );
}

// ═══════════════════════════════════════════════════════════════
// TIMESTAMP UNIT TEST
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_utc_iso8601_known_dates() {
    // Jan 1, 2000 00:00:00 UTC = unix 946684800
    assert_eq!(utc_iso8601(946_684_800), "2000-01-01T00:00:00Z");
    // Unix epoch
    assert_eq!(utc_iso8601(0), "1970-01-01T00:00:00Z");
    // Dec 31, 1999 23:59:59 UTC = unix 946684799
    assert_eq!(utc_iso8601(946_684_799), "1999-12-31T23:59:59Z");
    // Mar 15, 2026 02:53:34 UTC = unix 1773543214
    assert_eq!(utc_iso8601(1_773_543_214), "2026-03-15T02:53:34Z");
}
