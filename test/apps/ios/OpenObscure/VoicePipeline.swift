/// VoicePipeline — End-to-end voice PII detection for iOS.
///
/// Orchestrates: Audio decode → SFSpeechRecognizer transcription → PII scan → strip.
/// Uses the Rust `sanitizeAudioTranscript()` UniFFI binding for PII detection/encryption.
///
/// Phase 13D: Platform Speech API Integration (iOS)

import Foundation

/// Result of processing an audio block through the voice pipeline.
struct VoicePipelineResult {
    /// Whether PII was detected in the audio.
    let piiDetected: Bool
    /// Number of PII matches found.
    let piiCount: Int
    /// Categories of PII detected (e.g., "ssn", "credit_card").
    let categories: [String]
    /// Sanitized transcript (PII encrypted). Empty if no PII or transcription failed.
    let sanitizedTranscript: String
    /// Whether to strip the audio block from the request.
    let shouldStrip: Bool
    /// Total pipeline duration in milliseconds.
    let totalMs: Int
    /// Transcription duration in milliseconds.
    let transcribeMs: Int
}

/// Voice PII detection pipeline for iOS.
///
/// Flow: Audio data → VoiceScanner (SFSpeechRecognizer) → transcript →
/// OpenObscure Rust scanner (via UniFFI) → PII result → strip decision.
class VoicePipeline {

    private let voiceScanner: VoiceScanner
    private let handle: OpenObscureHandle

    /// Create a voice pipeline.
    /// - Parameter handle: OpenObscure handle for PII scanning.
    /// - Parameter locale: Locale for speech recognition.
    init(handle: OpenObscureHandle, locale: Locale = Locale(identifier: "en-US")) {
        self.voiceScanner = VoiceScanner(locale: locale)
        self.handle = handle
    }

    /// Whether voice scanning is available on this device.
    var isAvailable: Bool {
        voiceScanner.isAvailable
    }

    /// Process an audio block for PII.
    ///
    /// - Parameter audioData: Raw audio bytes (WAV format).
    /// - Parameter completion: Called with the pipeline result.
    func processAudioBlock(audioData: Data, completion: @escaping (VoicePipelineResult) -> Void) {
        let start = DispatchTime.now()

        guard isAvailable else {
            // Voice scanner not available — fail-open (pass audio through)
            completion(VoicePipelineResult(
                piiDetected: false, piiCount: 0, categories: [],
                sanitizedTranscript: "", shouldStrip: false,
                totalMs: 0, transcribeMs: 0
            ))
            return
        }

        voiceScanner.transcribe(audioData: audioData) { [self] transcription in
            let elapsed = DispatchTime.now().uptimeNanoseconds - start.uptimeNanoseconds
            let totalMs = Int(elapsed / 1_000_000)

            guard transcription.success, !transcription.transcript.isEmpty else {
                // Transcription failed — fail-open
                completion(VoicePipelineResult(
                    piiDetected: false, piiCount: 0, categories: [],
                    sanitizedTranscript: "", shouldStrip: false,
                    totalMs: totalMs, transcribeMs: transcription.durationMs
                ))
                return
            }

            // Check transcript for PII using the Rust scanner
            let piiCount = checkAudioPii(
                handle: self.handle, transcript: transcription.transcript
            )

            if piiCount > 0 {
                // PII found — sanitize the transcript
                do {
                    let result = try sanitizeAudioTranscript(
                        handle: self.handle,
                        transcript: transcription.transcript
                    )
                    completion(VoicePipelineResult(
                        piiDetected: true,
                        piiCount: Int(result.piiCount),
                        categories: result.categories,
                        sanitizedTranscript: result.sanitizedText,
                        shouldStrip: true,
                        totalMs: totalMs,
                        transcribeMs: transcription.durationMs
                    ))
                } catch {
                    // Sanitization failed — strip the audio block as precaution
                    completion(VoicePipelineResult(
                        piiDetected: true, piiCount: Int(piiCount),
                        categories: [], sanitizedTranscript: "",
                        shouldStrip: true, totalMs: totalMs,
                        transcribeMs: transcription.durationMs
                    ))
                }
            } else {
                // No PII — pass audio through
                completion(VoicePipelineResult(
                    piiDetected: false, piiCount: 0, categories: [],
                    sanitizedTranscript: "", shouldStrip: false,
                    totalMs: totalMs, transcribeMs: transcription.durationMs
                ))
            }
        }
    }
}
