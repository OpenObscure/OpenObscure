// Library target — exposes modules needed by criterion benchmarks and integration tests.
// The main binary crate is in main.rs and owns all modules independently.

#[macro_use]
pub mod oo_log;
pub mod crf_scanner;
pub mod fpe_engine;
pub mod hybrid_scanner;
pub mod keyword_dict;
pub mod ner_scanner;
pub mod pii_types;
pub mod scanner;
pub mod wordpiece;

// Image pipeline modules — exported for examples and integration tests
pub mod config;
pub mod face_detector;
pub mod image_blur;
pub mod image_pipeline;
pub mod nsfw_detector;
pub mod ocr_engine;
pub mod screen_guard;

// Detection verification framework
pub mod detection_meta;
pub mod detection_validators;

// Compliance and breach detection (also used by governance for mobile breach assessment)
pub mod breach_detect;
pub mod compliance;

// Privacy governance (consent, file guard, retention — SQLite-backed)
#[cfg(feature = "governance")]
pub mod governance;

// Mobile library API
pub mod lib_mobile;
#[cfg(feature = "mobile")]
pub mod uniffi_bindings;
