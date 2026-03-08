// OpenObscureManager.kt — Template for Android integration
// Copy this file into your app's source tree and adjust the package name.

package your.app.package

import android.content.Context
import uniffi.openobscure_proxy.*

object OpenObscureManager {
    private var _handle: OpenObscureHandle? = null
    var lastMappingJson: String = "{}"

    /** Call once from Application.onCreate(). */
    fun init(context: Context) {
        if (_handle != null) return
        val key = getOrCreateKey(context)
        // Bundle all models under assets/models/. Copy to internal storage at first launch.
        // The tier system auto-detects device RAM and loads only what fits:
        //   Full (≥8 GB) → DistilBERT NER, SCRFD, full OCR, NSFW, RI
        //   Standard (4–8 GB) → TinyBERT NER, SCRFD, detect-only OCR
        //   Lite (<4 GB) → TinyBERT NER, BlazeFace, minimal pipeline
        // EXIF metadata is always stripped from images regardless of tier.
        val modelsDir = copyAssetsDir(context, "models")
        _handle = createOpenobscure(
            configJson = """{"scanner_mode": "auto", "models_base_dir": "$modelsDir"}""",
            fpeKeyHex = key
        )
    }

    val handle: OpenObscureHandle
        get() = _handle ?: throw IllegalStateException("OpenObscureManager not initialized")

    /** Sanitize text — returns SanitizeResultFfi with sanitizedText, piiCount, mappingJson. */
    fun sanitize(text: String): SanitizeResultFfi {
        val result = sanitizeText(handle, text)
        if (result.piiCount > 0u) {
            lastMappingJson = result.mappingJson
        }
        return result
    }

    /** Restore PII in LLM response text using the last saved mapping. */
    fun restore(text: String): String {
        return restoreText(handle, text, lastMappingJson)
    }

    // --- Key storage via SharedPreferences ---

    private fun getOrCreateKey(context: Context): String {
        val prefs = context.getSharedPreferences("openobscure", Context.MODE_PRIVATE)
        prefs.getString("fpe_key", null)?.let { return it }

        val bytes = ByteArray(32)
        java.security.SecureRandom().nextBytes(bytes)
        val hex = bytes.joinToString("") { "%02x".format(it) }

        prefs.edit().putString("fpe_key", hex).apply()
        return hex
    }
}
