// OpenObscureManager.kt — Template for Android integration
// Copy this file into your app's source tree and adjust the package name.

package your.app.package

import android.content.Context
import android.util.Log
import uniffi.openobscure_core.*
import java.io.File

object OpenObscureManager {
    private var _handle: OpenObscureHandle? = null
    private val accumulatedMappings = mutableListOf<List<String>>()
    private const val TAG = "OpenObscure"

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

        // Drain Rust-side debug log — model loading diagnostics, verification results.
        val debugLog = getDebugLog()
        if (debugLog.isNotEmpty()) {
            Log.d(TAG, debugLog)
        }
    }

    val handle: OpenObscureHandle
        get() = _handle ?: throw IllegalStateException("OpenObscureManager not initialized")

    /** Sanitize text — accumulates mappings across calls for the current conversation. */
    fun sanitize(text: String): SanitizeResultFfi {
        val result = sanitizeText(handle, text)
        if (result.piiCount > 0u) {
            mergeMappings(result.mappingJson)
        }
        return result
    }

    /** Restore PII in LLM response text using accumulated mappings. */
    fun restore(text: String): String {
        val json = try {
            org.json.JSONArray(accumulatedMappings.map { org.json.JSONArray(it) }).toString()
        } catch (_: Exception) { "{}" }
        return restoreText(handle, text, json)
    }

    /** Scan LLM response for persuasion/manipulation (cognitive firewall). */
    fun scanResponse(text: String): RiReportFfi? {
        return uniffi.openobscure_core.scanResponse(handle, text)
    }

    /** Reset mappings when starting a new conversation. */
    fun resetMappings() {
        accumulatedMappings.clear()
    }

    private fun mergeMappings(json: String) {
        try {
            val arr = org.json.JSONArray(json)
            val tokenIndex = mutableMapOf<String, Int>()
            accumulatedMappings.forEachIndexed { i, pair ->
                if (pair.size >= 2) tokenIndex[pair[0]] = i
            }
            for (i in 0 until arr.length()) {
                val pair = arr.getJSONArray(i)
                if (pair.length() >= 2) {
                    val token = pair.getString(0)
                    val value = pair.getString(1)
                    val existing = tokenIndex[token]
                    if (existing != null) {
                        accumulatedMappings[existing] = listOf(token, value)
                    } else {
                        tokenIndex[token] = accumulatedMappings.size
                        accumulatedMappings.add(listOf(token, value))
                    }
                }
            }
        } catch (_: Exception) { }
    }

    // --- Asset copying ---

    /** Copy an assets directory to internal storage (ONNX Runtime needs file paths). */
    private fun copyAssetsDir(context: Context, assetDir: String): String {
        val outDir = File(context.filesDir, assetDir)
        if (!outDir.exists()) {
            outDir.mkdirs()
            copyAssetsDirRecursive(context, assetDir, outDir)
        }
        return outDir.absolutePath
    }

    private fun copyAssetsDirRecursive(context: Context, assetPath: String, outDir: File) {
        val entries = context.assets.list(assetPath) ?: return
        for (entry in entries) {
            val subAssetPath = "$assetPath/$entry"
            val subEntries = context.assets.list(subAssetPath)
            if (subEntries != null && subEntries.isNotEmpty()) {
                // It's a subdirectory — recurse
                val subDir = File(outDir, entry)
                subDir.mkdirs()
                copyAssetsDirRecursive(context, subAssetPath, subDir)
            } else {
                // It's a file — copy
                context.assets.open(subAssetPath).use { input ->
                    File(outDir, entry).outputStream().use { output ->
                        input.copyTo(output)
                    }
                }
            }
        }
    }

    // --- Key storage via SharedPreferences ---

    private fun getOrCreateKey(context: Context): String {
        val prefs = context.getSharedPreferences("openobscure", Context.MODE_PRIVATE)
        prefs.getString("fpe_key", null)?.let { return it }

        val bytes = ByteArray(32)
        java.security.SecureRandom().nextBytes(bytes)
        val hex = bytes.joinToString("") { "%02x".format(it) }

        // In production: use Android Keystore instead of SharedPreferences
        prefs.edit().putString("fpe_key", hex).apply()
        return hex
    }
}
