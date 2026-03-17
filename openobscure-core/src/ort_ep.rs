//! Platform-specific ONNX Runtime execution provider configuration.
//!
//! Conditionally registers hardware-accelerated EPs:
//! - **Apple (iOS/macOS)**: CoreML → Neural Engine / GPU / CPU
//! - **Android**: NNAPI → NPU / GPU / CPU
//! - **Other**: CPU only (default)
//!
//! Falls back gracefully — if the EP feature isn't compiled in or the
//! platform doesn't support it, ORT silently falls back to CPU.

use std::path::Path;

use ort::ep::ExecutionProviderDispatch;
use ort::session::Session;

/// Returns platform-appropriate execution providers.
///
/// Empty on desktop Linux/Windows (CPU-only).
/// Non-empty on Apple (CoreML) or Android (NNAPI).
///
/// **iOS**: Uses CoreML with NeuralNetwork format (Core ML 3+) and CPUAndGPU compute
/// units. The default MLProgram format produces incorrect Conv padding for SCRFD,
/// PaddleOCR, and TinyBERT models on iOS. NeuralNetwork format avoids this.
/// ANE is skipped because some devices report `Unknown aneSubType`.
///
/// **macOS**: Uses CoreML with default settings (MLProgram + All compute units).
pub fn platform_eps() -> Vec<ExecutionProviderDispatch> {
    #[allow(unused_mut)]
    let mut eps = Vec::new();

    // CoreML on iOS: NeuralNetwork format + GPU only (no ANE)
    // MLProgram format mangles conv layers in SCRFD/PaddleOCR/TinyBERT models.
    // ANE has unknown subtype on some iOS devices → skip it.
    #[cfg(target_os = "ios")]
    {
        eps.push(
            ort::ep::CoreML::default()
                .with_model_format(ort::ep::coreml::ModelFormat::NeuralNetwork)
                .with_compute_units(ort::ep::coreml::ComputeUnits::CPUAndGPU)
                .build(),
        );
    }

    // CoreML on macOS: default settings (MLProgram works fine on macOS)
    #[cfg(all(target_vendor = "apple", not(target_os = "ios")))]
    {
        eps.push(ort::ep::CoreML::default().build());
    }

    // Android: we ship ORT as a runtime-loaded `.so` via the `alternative-backend`
    // mechanism, which does not support NNAPI. NNAPI also requires direct symbol
    // linking at compile time — incompatible with dynamic loading. CPU-only.

    eps
}

/// Build an ONNX Runtime session with platform EPs and single-threaded inference.
///
/// All model loaders should use this instead of calling `Session::builder()` directly.
/// This ensures consistent EP configuration across face detection, OCR, NER, and NSFW.
pub fn build_session(model_path: &Path) -> ort::Result<Session> {
    let mut builder = Session::builder()?.with_intra_threads(1)?;

    let eps = platform_eps();
    if !eps.is_empty() {
        builder = builder.with_execution_providers(eps)?;
    }

    builder.commit_from_file(model_path)
}

/// Build an ONNX Runtime session pinned to CPU-only execution.
///
/// Use for models where CoreML or NNAPI produce numerically incorrect results
/// (e.g., some quantised INT8 graphs that rely on CPU-specific op implementations).
/// Prefer `build_session()` for all other models.
pub fn build_session_cpu(model_path: &Path) -> ort::Result<Session> {
    Session::builder()?
        .with_intra_threads(1)?
        .commit_from_file(model_path)
}

/// Build an ONNX Runtime session using CoreML with MLProgram format.
///
/// MLProgram (Core ML 5+, iOS 15+) supports INT8 weight quantization natively,
/// making it the correct target for INT8-quantized ONNX models. NeuralNetwork
/// format has no INT8 op support and silently produces NaN for quantized graphs.
///
/// **iOS**: Uses CPUAndGPU compute units (no ANE) — same ANE exclusion as
/// `platform_eps()` to avoid `Unknown aneSubType` crashes on some devices.
/// Neural Engine INT8 execution requires A14 Bionic+; CPUAndGPU gives correct
/// results on all supported devices.
///
/// **macOS**: Uses default compute units (MLProgram is the macOS default and
/// handles INT8 correctly on all Apple Silicon Macs).
///
/// **Non-Apple**: Falls back to CPU (no CoreML available).
///
/// Use this specifically for INT8-quantized models where `build_session()`
/// (NeuralNetwork EP on iOS) produces NaN. Example: `nsfw_5class_int8.onnx`.
pub fn build_session_coreml_mlprogram(model_path: &Path) -> ort::Result<Session> {
    #[allow(unused_mut)]
    let mut eps: Vec<ort::ep::ExecutionProviderDispatch> = Vec::new();

    #[cfg(target_os = "ios")]
    {
        eps.push(
            ort::ep::CoreML::default()
                .with_model_format(ort::ep::coreml::ModelFormat::MLProgram)
                .with_compute_units(ort::ep::coreml::ComputeUnits::CPUAndGPU)
                .build(),
        );
    }

    #[cfg(all(target_vendor = "apple", not(target_os = "ios")))]
    {
        eps.push(
            ort::ep::CoreML::default()
                .with_model_format(ort::ep::coreml::ModelFormat::MLProgram)
                .build(),
        );
    }

    let mut builder = Session::builder()?.with_intra_threads(1)?;
    if !eps.is_empty() {
        builder = builder.with_execution_providers(eps)?;
    }
    builder.commit_from_file(model_path)
}

/// Name of the active execution provider backend (for logging).
pub fn ep_name() -> &'static str {
    if cfg!(target_vendor = "apple") {
        "CoreML"
    } else {
        "CPU"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_eps_returns_vec() {
        let eps = platform_eps();
        // Apple (macOS + iOS): CoreML EP; Android: CPU only (alternative-backend); other: CPU only
        #[cfg(target_vendor = "apple")]
        assert_eq!(eps.len(), 1);
        #[cfg(not(target_vendor = "apple"))]
        assert!(eps.is_empty());
    }

    #[test]
    fn test_ep_name() {
        let name = ep_name();
        #[cfg(target_vendor = "apple")]
        assert_eq!(name, "CoreML");
        #[cfg(not(target_vendor = "apple"))]
        assert_eq!(name, "CPU");
    }

    #[test]
    fn test_build_session_with_missing_model() {
        // Should fail gracefully with a file not found error
        let result = build_session(Path::new("/nonexistent/model.onnx"));
        assert!(result.is_err());
    }
}
