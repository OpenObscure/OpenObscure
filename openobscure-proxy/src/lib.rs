// Library target — exposes modules needed by criterion benchmarks and integration tests.
// The main binary crate is in main.rs and owns all modules independently.

// UniFFI scaffolding — must precede any module using UniFFI derive macros
#[cfg(feature = "mobile")]
uniffi::setup_scaffolding!();

#[macro_use]
pub mod oo_log;
pub mod crf_scanner;
pub mod device_profile;
pub mod fpe_engine;
pub mod hybrid_scanner;
pub mod keyword_dict;
pub mod ner_scanner;
pub mod pii_types;
pub mod scanner;
pub mod wordpiece;

// Multilingual PII detection
pub mod lang_detect;
pub mod multilingual;

// Image pipeline modules — exported for examples and integration tests
pub mod config;
pub mod face_detector;
pub mod image_detect;
pub mod image_pipeline;
pub mod image_redact;
pub mod nsfw_detector;
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

// Request/response mapping + SSE frame accumulation
pub mod mapping;
pub mod sse_accumulator;

// Mobile library API
pub mod lib_mobile;
#[cfg(feature = "mobile")]
pub mod uniffi_bindings;
