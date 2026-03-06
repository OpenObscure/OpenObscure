/**
 * VoicePipeline — End-to-end voice PII detection for Android.
 *
 * Orchestrates: Audio decode -> SpeechRecognizer transcription -> PII scan -> strip.
 * Uses the Rust `sanitizeAudioTranscript()` UniFFI binding for PII detection/encryption.
 *
 * Phase 13D: Platform Speech API Integration (Android)
 */
package ai.openobscure.test

import android.content.Context
import uniffi.openobscure_proxy.OpenObscureHandle
import uniffi.openobscure_proxy.checkAudioPii
import uniffi.openobscure_proxy.sanitizeAudioTranscript

/**
 * Result of processing an audio block through the voice pipeline.
 */
data class VoicePipelineResult(
    val piiDetected: Boolean,
    val piiCount: Int,
    val categories: List<String>,
    val sanitizedTranscript: String,
    val shouldStrip: Boolean,
    val totalMs: Long,
    val transcribeMs: Long,
)

/**
 * Voice PII detection pipeline for Android.
 *
 * Flow: Audio data -> VoiceScanner (SpeechRecognizer) -> transcript ->
 * OpenObscure Rust scanner (via UniFFI) -> PII result -> strip decision.
 */
class VoicePipeline(
    private val context: Context,
    private val handle: OpenObscureHandle,
) {
    private val voiceScanner = VoiceScanner(context)

    /**
     * Whether voice scanning is available on this device.
     */
    val isAvailable: Boolean
        get() = voiceScanner.isAvailable

    /**
     * Initialize the voice pipeline. Must be called from the main thread.
     */
    fun initialize() {
        voiceScanner.initialize()
    }

    /**
     * Process an audio block for PII.
     *
     * @param audioData Raw audio bytes (WAV format).
     * @param callback Called with the pipeline result.
     */
    fun processAudioBlock(audioData: ByteArray, callback: (VoicePipelineResult) -> Unit) {
        val startMs = System.currentTimeMillis()

        if (!isAvailable) {
            // Voice scanner not available — fail-open
            callback(VoicePipelineResult(
                piiDetected = false, piiCount = 0, categories = emptyList(),
                sanitizedTranscript = "", shouldStrip = false,
                totalMs = 0, transcribeMs = 0
            ))
            return
        }

        voiceScanner.transcribe(audioData) { transcription ->
            val totalMs = System.currentTimeMillis() - startMs

            if (!transcription.success || transcription.transcript.isEmpty()) {
                // Transcription failed — fail-open
                callback(VoicePipelineResult(
                    piiDetected = false, piiCount = 0, categories = emptyList(),
                    sanitizedTranscript = "", shouldStrip = false,
                    totalMs = totalMs, transcribeMs = transcription.durationMs
                ))
                return@transcribe
            }

            // Check transcript for PII using the Rust scanner
            val piiCount = checkAudioPii(handle, transcription.transcript)

            if (piiCount > 0u) {
                // PII found — sanitize the transcript
                try {
                    val result = sanitizeAudioTranscript(handle, transcription.transcript)
                    callback(VoicePipelineResult(
                        piiDetected = true,
                        piiCount = result.piiCount.toInt(),
                        categories = result.categories,
                        sanitizedTranscript = result.sanitizedText,
                        shouldStrip = true,
                        totalMs = totalMs,
                        transcribeMs = transcription.durationMs
                    ))
                } catch (e: Exception) {
                    // Sanitization failed — strip as precaution
                    callback(VoicePipelineResult(
                        piiDetected = true, piiCount = piiCount.toInt(),
                        categories = emptyList(), sanitizedTranscript = "",
                        shouldStrip = true, totalMs = totalMs,
                        transcribeMs = transcription.durationMs
                    ))
                }
            } else {
                // No PII — pass audio through
                callback(VoicePipelineResult(
                    piiDetected = false, piiCount = 0, categories = emptyList(),
                    sanitizedTranscript = "", shouldStrip = false,
                    totalMs = totalMs, transcribeMs = transcription.durationMs
                ))
            }
        }
    }

    /**
     * Release resources.
     */
    fun release() {
        voiceScanner.release()
    }
}
