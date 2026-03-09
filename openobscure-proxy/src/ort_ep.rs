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

    // NNAPI: Qualcomm Hexagon / Mali GPU on Android
    #[cfg(target_os = "android")]
    {
        eps.push(ort::ep::NNAPI::default().build());
    }

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

/// Build an ONNX Runtime session with CPU-only execution (no hardware accelerators).
///
/// Use this for models where CoreML/NNAPI produce incorrect results.
pub fn build_session_cpu(model_path: &Path) -> ort::Result<Session> {
    Session::builder()?
        .with_intra_threads(1)?
        .commit_from_file(model_path)
}

/// Name of the active execution provider backend (for logging).
pub fn ep_name() -> &'static str {
    if cfg!(target_vendor = "apple") {
        "CoreML"
    } else if cfg!(target_os = "android") {
        "NNAPI"
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
        // Apple (macOS + iOS): CoreML EP; Android: NNAPI EP; other: CPU only
        #[cfg(target_vendor = "apple")]
        assert_eq!(eps.len(), 1);
        #[cfg(target_os = "android")]
        assert_eq!(eps.len(), 1);
        #[cfg(not(any(target_vendor = "apple", target_os = "android")))]
        assert!(eps.is_empty());
    }

    #[test]
    fn test_ep_name() {
        let name = ep_name();
        #[cfg(target_vendor = "apple")]
        assert_eq!(name, "CoreML");
        #[cfg(target_os = "android")]
        assert_eq!(name, "NNAPI");
        #[cfg(not(any(target_vendor = "apple", target_os = "android")))]
        assert_eq!(name, "CPU");
    }

    #[test]
    fn test_build_session_with_missing_model() {
        // Should fail gracefully with a file not found error
        let result = build_session(Path::new("/nonexistent/model.onnx"));
        assert!(result.is_err());
    }
}
