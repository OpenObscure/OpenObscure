/// VoiceScanner — On-device speech transcription for PII detection.
///
/// Uses `SFSpeechRecognizer` with `requiresOnDeviceRecognition = true` so audio
/// never leaves the device. `contextualStrings` biases recognition toward PII
/// trigger phrases for improved accuracy.
///
/// Phase 13D: Platform Speech API Integration (iOS)

import Foundation
import Speech

/// PII trigger phrases to bias speech recognition toward.
/// Matches the keyword set from `openobscure-proxy/models/kws/keywords.txt`.
let piiContextualStrings: [String] = [
    "social security", "social security number",
    "credit card", "credit card number", "card number",
    "date of birth", "phone number",
    "bank account", "account number",
    "driver's license", "passport number",
    "routing number", "medical record",
    "insurance number", "home address",
]

/// Result of a voice transcription attempt.
struct VoiceTranscriptionResult {
    /// Transcribed text (empty if transcription failed).
    let transcript: String
    /// Whether the transcription succeeded.
    let success: Bool
    /// Error message if transcription failed.
    let error: String?
    /// Transcription duration in milliseconds.
    let durationMs: Int
}

/// Wraps `SFSpeechRecognizer` for on-device PII-biased transcription.
class VoiceScanner {

    private let recognizer: SFSpeechRecognizer?
    private let onDeviceOnly: Bool

    /// Create a voice scanner.
    /// - Parameter locale: Locale for speech recognition (default: en-US).
    /// - Parameter onDeviceOnly: Require on-device recognition (default: true).
    init(locale: Locale = Locale(identifier: "en-US"), onDeviceOnly: Bool = true) {
        self.recognizer = SFSpeechRecognizer(locale: locale)
        self.onDeviceOnly = onDeviceOnly
    }

    /// Whether the speech recognizer is available and supports on-device recognition.
    var isAvailable: Bool {
        guard let rec = recognizer else { return false }
        if onDeviceOnly {
            return rec.isAvailable && rec.supportsOnDeviceRecognition
        }
        return rec.isAvailable
    }

    /// Transcribe audio data (PCM/WAV) to text.
    ///
    /// - Parameter audioData: Raw audio bytes (WAV format expected).
    /// - Parameter completion: Called with the transcription result.
    func transcribe(audioData: Data, completion: @escaping (VoiceTranscriptionResult) -> Void) {
        let start = DispatchTime.now()

        guard let rec = recognizer, isAvailable else {
            completion(VoiceTranscriptionResult(
                transcript: "", success: false,
                error: "Speech recognizer not available", durationMs: 0
            ))
            return
        }

        // Write audio to temp file for SFSpeechURLRecognitionRequest
        let tempURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString + ".wav")
        do {
            try audioData.write(to: tempURL)
        } catch {
            completion(VoiceTranscriptionResult(
                transcript: "", success: false,
                error: "Failed to write temp audio: \(error.localizedDescription)", durationMs: 0
            ))
            return
        }

        let request = SFSpeechURLRecognitionRequest(url: tempURL)
        request.requiresOnDeviceRecognition = onDeviceOnly
        request.contextualStrings = piiContextualStrings
        request.shouldReportPartialResults = false

        rec.recognitionTask(with: request) { result, error in
            // Clean up temp file
            try? FileManager.default.removeItem(at: tempURL)

            let elapsed = DispatchTime.now().uptimeNanoseconds - start.uptimeNanoseconds
            let durationMs = Int(elapsed / 1_000_000)

            if let error = error {
                completion(VoiceTranscriptionResult(
                    transcript: "", success: false,
                    error: error.localizedDescription, durationMs: durationMs
                ))
                return
            }

            let transcript = result?.bestTranscription.formattedString ?? ""
            completion(VoiceTranscriptionResult(
                transcript: transcript, success: true,
                error: nil, durationMs: durationMs
            ))
        }
    }

    /// Request speech recognition authorization.
    static func requestAuthorization(completion: @escaping (Bool) -> Void) {
        SFSpeechRecognizer.requestAuthorization { status in
            completion(status == .authorized)
        }
    }
}
