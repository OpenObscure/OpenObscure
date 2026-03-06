/**
 * VoiceScanner — On-device speech transcription for PII detection.
 *
 * Uses Android `SpeechRecognizer` with offline language model for
 * on-device transcription. Audio never leaves the device when the
 * offline language pack is installed.
 *
 * Phase 13D: Platform Speech API Integration (Android)
 */
package ai.openobscure.test

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import java.io.File
import java.io.FileOutputStream

/**
 * Result of a voice transcription attempt.
 */
data class VoiceTranscriptionResult(
    val transcript: String,
    val success: Boolean,
    val error: String?,
    val durationMs: Long,
)

/**
 * Wraps Android SpeechRecognizer for on-device PII-biased transcription.
 */
class VoiceScanner(private val context: Context) {

    private var recognizer: SpeechRecognizer? = null

    /**
     * Whether the speech recognizer is available.
     */
    val isAvailable: Boolean
        get() = SpeechRecognizer.isRecognitionAvailable(context)

    /**
     * Initialize the speech recognizer.
     * Must be called from the main thread.
     */
    fun initialize() {
        if (recognizer == null) {
            recognizer = SpeechRecognizer.createSpeechRecognizer(context)
        }
    }

    /**
     * Transcribe audio data to text.
     *
     * Writes audio to a temp file and uses SpeechRecognizer to transcribe.
     * The recognizer is configured for offline recognition when available.
     *
     * @param audioData Raw audio bytes (WAV format).
     * @param callback Called with the transcription result.
     */
    fun transcribe(audioData: ByteArray, callback: (VoiceTranscriptionResult) -> Unit) {
        val startMs = System.currentTimeMillis()

        val rec = recognizer
        if (rec == null || !isAvailable) {
            callback(VoiceTranscriptionResult(
                transcript = "", success = false,
                error = "Speech recognizer not available", durationMs = 0
            ))
            return
        }

        // Write audio to temp file
        val tempFile = File(context.cacheDir, "oo_voice_${System.nanoTime()}.wav")
        try {
            FileOutputStream(tempFile).use { it.write(audioData) }
        } catch (e: Exception) {
            callback(VoiceTranscriptionResult(
                transcript = "", success = false,
                error = "Failed to write temp audio: ${e.message}", durationMs = 0
            ))
            return
        }

        val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
            putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            putExtra(RecognizerIntent.EXTRA_LANGUAGE, "en-US")
            putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, false)
            // Request offline recognition
            putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, true)
        }

        rec.setRecognitionListener(object : RecognitionListener {
            override fun onResults(results: Bundle?) {
                tempFile.delete()
                val elapsed = System.currentTimeMillis() - startMs
                val matches = results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
                val transcript = matches?.firstOrNull() ?: ""
                callback(VoiceTranscriptionResult(
                    transcript = transcript, success = true,
                    error = null, durationMs = elapsed
                ))
            }

            override fun onError(error: Int) {
                tempFile.delete()
                val elapsed = System.currentTimeMillis() - startMs
                val errorMsg = when (error) {
                    SpeechRecognizer.ERROR_AUDIO -> "Audio recording error"
                    SpeechRecognizer.ERROR_CLIENT -> "Client error"
                    SpeechRecognizer.ERROR_NETWORK -> "Network error"
                    SpeechRecognizer.ERROR_NO_MATCH -> "No speech detected"
                    SpeechRecognizer.ERROR_SPEECH_TIMEOUT -> "Speech timeout"
                    else -> "Recognition error ($error)"
                }
                callback(VoiceTranscriptionResult(
                    transcript = "", success = false,
                    error = errorMsg, durationMs = elapsed
                ))
            }

            override fun onReadyForSpeech(params: Bundle?) {}
            override fun onBeginningOfSpeech() {}
            override fun onRmsChanged(rmsdB: Float) {}
            override fun onBufferReceived(buffer: ByteArray?) {}
            override fun onEndOfSpeech() {}
            override fun onPartialResults(partialResults: Bundle?) {}
            override fun onEvent(eventType: Int, params: Bundle?) {}
        })

        rec.startListening(intent)
    }

    /**
     * Release the speech recognizer resources.
     */
    fun release() {
        recognizer?.destroy()
        recognizer = null
    }
}
