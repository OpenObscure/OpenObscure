// Library target — exposes modules needed by criterion benchmarks and integration tests.
// The main binary crate is in main.rs and owns all modules independently.

#[macro_use]
pub mod cg_log;
pub mod pii_types;
pub mod scanner;
pub mod fpe_engine;
pub mod keyword_dict;
pub mod crf_scanner;
pub mod ner_scanner;
pub mod wordpiece;
pub mod hybrid_scanner;
