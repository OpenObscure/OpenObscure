use criterion::{black_box, criterion_group, criterion_main, Criterion};

// We benchmark the regex scanner directly since HybridScanner requires
// ONNX models or CRF weights that may not be available in CI.
// This isolates the regex+Luhn+SSN validation path.

fn scanner_benchmark(c: &mut Criterion) {
    // Use inline scanner construction — the PiiScanner is the deterministic
    // regex layer that dominates scan latency for structured PII.
    let scanner = openobscure_core::scanner::PiiScanner::new();

    // ── Single PII type ─────────────────────────────────────────────

    c.bench_function("scan_ssn_short", |b| {
        let text = "My SSN is 123-45-6789";
        b.iter(|| scanner.scan_text(black_box(text)))
    });

    c.bench_function("scan_credit_card", |b| {
        let text = "Card: 4111-1111-1111-1111";
        b.iter(|| scanner.scan_text(black_box(text)))
    });

    c.bench_function("scan_email", |b| {
        let text = "Contact johndoe@example.com for details";
        b.iter(|| scanner.scan_text(black_box(text)))
    });

    c.bench_function("scan_phone_us", |b| {
        let text = "Call (555) 123-4567 for support";
        b.iter(|| scanner.scan_text(black_box(text)))
    });

    c.bench_function("scan_api_key", |b| {
        let text = "Key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz1234567890";
        b.iter(|| scanner.scan_text(black_box(text)))
    });

    // ── Mixed PII ───────────────────────────────────────────────────

    c.bench_function("scan_mixed_3_types", |b| {
        let text = "User 123-45-6789 email johndoe@example.com phone (555) 123-4567";
        b.iter(|| scanner.scan_text(black_box(text)))
    });

    // ── No PII (negative case) ──────────────────────────────────────

    c.bench_function("scan_no_pii_short", |b| {
        let text = "Hello, how are you today? The weather is nice.";
        b.iter(|| scanner.scan_text(black_box(text)))
    });

    c.bench_function("scan_no_pii_long", |b| {
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(50);
        b.iter(|| scanner.scan_text(black_box(&text)))
    });

    // ── JSON scanning ───────────────────────────────────────────────

    c.bench_function("scan_json_messages", |b| {
        let json: serde_json::Value = serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "messages": [
                {"role": "user", "content": "My SSN is 123-45-6789 and my card is 4111-1111-1111-1111"},
                {"role": "assistant", "content": "I understand you shared sensitive information."},
                {"role": "user", "content": "Also email me at johndoe@example.com"}
            ]
        });
        let skip = vec!["model".to_string()];
        b.iter(|| scanner.scan_json(black_box(&json), black_box(&skip)))
    });

    // ── Luhn rejection (numbers that look like CCs but fail) ────────

    c.bench_function("scan_luhn_rejection", |b| {
        let text = "Not a card: 4111-1111-1111-1112 or 5500-0000-0000-0003";
        b.iter(|| scanner.scan_text(black_box(text)))
    });
}

criterion_group!(benches, scanner_benchmark);
criterion_main!(benches);
