use criterion::{black_box, criterion_group, criterion_main, Criterion};

use openobscure_core::fpe_engine::FpeEngine;
use openobscure_core::pii_types::PiiType;
use openobscure_core::scanner::PiiMatch;

fn test_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = i as u8;
    }
    key
}

fn fpe_benchmark(c: &mut Criterion) {
    let engine = FpeEngine::new(&test_key()).unwrap();
    let tweak = b"benchmark-tweak-value-1234567890";

    // ── Credit Card encrypt/decrypt ─────────────────────────────────

    c.bench_function("fpe_encrypt_credit_card", |b| {
        let pii = PiiMatch {
            pii_type: PiiType::CreditCard,
            start: 0,
            end: 19,
            raw_value: "4532-0151-1283-0366".to_string(),
            json_path: Some("content".to_string()),
            confidence: 1.0,
        };
        b.iter(|| engine.encrypt_match(black_box(&pii), black_box(tweak)))
    });

    c.bench_function("fpe_decrypt_credit_card", |b| {
        let pii = PiiMatch {
            pii_type: PiiType::CreditCard,
            start: 0,
            end: 19,
            raw_value: "4532-0151-1283-0366".to_string(),
            json_path: Some("content".to_string()),
            confidence: 1.0,
        };
        let encrypted = engine.encrypt_match(&pii, tweak).unwrap();
        b.iter(|| {
            engine.decrypt_value(
                black_box(&encrypted.encrypted),
                black_box(PiiType::CreditCard),
                black_box(tweak),
            )
        })
    });

    // ── SSN encrypt/decrypt ─────────────────────────────────────────

    c.bench_function("fpe_encrypt_ssn", |b| {
        let pii = PiiMatch {
            pii_type: PiiType::Ssn,
            start: 0,
            end: 11,
            raw_value: "123-45-6789".to_string(),
            json_path: None,
            confidence: 1.0,
        };
        b.iter(|| engine.encrypt_match(black_box(&pii), black_box(tweak)))
    });

    // ── Phone number encrypt/decrypt ────────────────────────────────

    c.bench_function("fpe_encrypt_phone", |b| {
        let pii = PiiMatch {
            pii_type: PiiType::PhoneNumber,
            start: 0,
            end: 14,
            raw_value: "(555) 123-4567".to_string(),
            json_path: None,
            confidence: 1.0,
        };
        b.iter(|| engine.encrypt_match(black_box(&pii), black_box(tweak)))
    });

    // ── Email encrypt/decrypt ───────────────────────────────────────

    c.bench_function("fpe_encrypt_email", |b| {
        let pii = PiiMatch {
            pii_type: PiiType::Email,
            start: 0,
            end: 19,
            raw_value: "johndoe@example.com".to_string(),
            json_path: None,
            confidence: 1.0,
        };
        b.iter(|| engine.encrypt_match(black_box(&pii), black_box(tweak)))
    });

    // ── Roundtrip (encrypt + decrypt) ───────────────────────────────

    c.bench_function("fpe_roundtrip_ssn", |b| {
        let pii = PiiMatch {
            pii_type: PiiType::Ssn,
            start: 0,
            end: 11,
            raw_value: "123-45-6789".to_string(),
            json_path: None,
            confidence: 1.0,
        };
        b.iter(|| {
            let res = engine
                .encrypt_match(black_box(&pii), black_box(tweak))
                .unwrap();
            engine
                .decrypt_value(
                    black_box(&res.encrypted),
                    black_box(PiiType::Ssn),
                    black_box(tweak),
                )
                .unwrap()
        })
    });

    // ── Tweak generation ────────────────────────────────────────────

    c.bench_function("tweak_generate", |b| {
        let uuid = uuid::Uuid::new_v4();
        b.iter(|| {
            openobscure_core::fpe_engine::TweakGenerator::generate(
                black_box(&uuid),
                black_box("messages[0].content"),
            )
        })
    });
}

criterion_group!(benches, fpe_benchmark);
criterion_main!(benches);
