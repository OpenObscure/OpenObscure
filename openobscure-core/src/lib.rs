//! Library target for OpenObscure Core.
//!
//! Re-exports the modules needed by criterion benchmarks, integration tests,
//! and the mobile UniFFI API. The binary entry point (`main.rs`) owns all
//! module declarations independently and is never compiled as part of this
//! library target.

// UniFFI scaffolding — must be the first UniFFI call in the crate.
// Generates the FFI glue (`uniffi_bindgen` reads the resulting UDL at build
// time). All modules that use `#[derive(uniffi::Object)]` or similar must
// appear AFTER this macro expansion.
#[cfg(feature = "mobile")]
uniffi::setup_scaffolding!();

#[macro_use]
pub mod oo_log;
pub mod crf_scanner;
pub mod device_profile;
pub mod fpe_engine;
pub mod hybrid_scanner;
pub mod keyword_dict;
pub mod name_gazetteer;
pub mod ner_scanner;
pub mod pii_types;
pub mod scanner;
pub mod wordpiece;

// Multilingual PII detection — whatlang language identification feeds
// language-specific scanners (es, fr, de, pt, ja, zh, ko, ar) with
// check-digit validation for national IDs, IBANs, and phone numbers.
pub mod lang_detect;
pub mod multilingual;

// Image pipeline modules — exported for examples and integration tests
pub mod config;
pub mod face_detector;
pub mod image_detect;
#[cfg(feature = "server")]
pub mod image_fetch;
pub mod image_pipeline;
pub mod image_redact;
pub mod nsfw_classifier;
pub mod ocr_engine;
pub mod screen_guard;

// Voice anonymization pipeline
pub mod audio_decode;
pub mod kws_engine;
pub mod voice_detect;
pub mod voice_pipeline;

// ONNX Runtime execution provider configuration
pub mod ort_ep;

// Response integrity (cognitive firewall)
pub mod persuasion_dict;
pub mod response_integrity;
pub mod ri_model;

// Detection verification framework
pub mod detection_meta;
pub mod detection_validators;

// Hash-based redaction tokens for non-FPE PII
pub mod hash_token;

// Inspect mode (--inspect CLI flag)
pub mod inspect;

// Request/response mapping + SSE frame accumulation
pub mod mapping;
pub mod response_format;
pub mod sse_accumulator;

// Mobile library API
pub mod lib_mobile;
#[cfg(feature = "mobile")]
pub mod uniffi_bindings;
