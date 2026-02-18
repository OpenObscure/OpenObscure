// Library target — exposes modules needed by criterion benchmarks and integration tests.
// The main binary crate is in main.rs and owns all modules independently.

#[macro_use]
pub mod oo_log;
pub mod pii_types;
pub mod scanner;
pub mod fpe_engine;
pub mod keyword_dict;
pub mod crf_scanner;
pub mod ner_scanner;
pub mod wordpiece;
pub mod hybrid_scanner;

// Image pipeline modules — exported for examples and integration tests
pub mod config;
pub mod image_pipeline;
pub mod image_blur;
pub mod face_detector;
pub mod ocr_engine;
pub mod screen_guard;
