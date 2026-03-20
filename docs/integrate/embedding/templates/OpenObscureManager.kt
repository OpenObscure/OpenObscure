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

    // In-memory cache: hash(originalText) → sanitizedText.
    // Avoids re-scanning prior user messages through NER on every turn.
    // Cleared on resetMappings() (conversation switch).
    private val sanitizeCache = mutableMapOf<Int, String>()

    // RI warning flags: hash(rawTokenText) → formatted warning label.
    // Set in ChatService.onSuccess when RI flags a response.
    // Read in Compose rememberRestoredText() to prepend warning to display.
    // Cleared on resetMappings() (conversation switch).
    private val riWarnings = mutableMapOf<Int, String>()

    // Version counter for RI warnings — incremented on each setRiWarning() call.
    // Used as a key in Compose `remember` to force recomposition when a warning
    // is stored after the initial render (the onSuccess callback fires after
    // Compose has already cached the restored text).
    val riVersion = androidx.compose.runtime.mutableIntStateOf(0)

    /** Call once from Application.onCreate(). */
    fun init(context: Context) {
        if (_handle != null) return
        val key = getOrCreateKey(context)
        val modelsDir = copyAssetsDir(context, "models")
        Log.d(TAG, "[INIT] models_dir=$modelsDir")
        _handle = createOpenobscure(
            configJson = """{"scanner_mode": "auto", "models_base_dir": "$modelsDir"}""",
            fpeKeyHex = key
        )

        // Drain Rust-side debug log — model loading, device profile, tier, etc.
        val debugLog = getDebugLog()
        if (debugLog.isNotEmpty()) {
            Log.d(TAG, "[INIT] $debugLog")
        }
        Log.d(TAG, "[INIT] handle=${_handle != null}, mappings=${accumulatedMappings.size}")
    }

    val handle: OpenObscureHandle
        get() = _handle ?: throw IllegalStateException("OpenObscureManager not initialized")

    /** Sanitize text — accumulates mappings across calls for the current conversation. */
    fun sanitize(text: String): SanitizeResultFfi {
        val startMs = System.currentTimeMillis()
        val result = sanitizeText(handle, text)
        val elapsed = System.currentTimeMillis() - startMs
        Log.d(TAG, "[SANITIZE] len=${text.length}, pii=${result.piiCount}, ms=$elapsed")
        if (result.piiCount > 0u) {
            mergeMappings(result.mappingJson)
            Log.d(TAG, "[SANITIZE] mappings_total=${accumulatedMappings.size}")
        }
        return result
    }

    /** Restore PII in LLM response text using accumulated mappings. */
    fun restore(text: String): String {
        val json = try {
            org.json.JSONArray(accumulatedMappings.map { org.json.JSONArray(it) }).toString()
        } catch (_: Exception) { "[]" }
        val startMs = System.currentTimeMillis()
        val restored = restoreText(handle, text, json)
        val elapsed = System.currentTimeMillis() - startMs
        Log.d(TAG, "[RESTORE] len=${text.length}, mappings=${accumulatedMappings.size}, ms=$elapsed")
        return restored
    }

    /**
     * Sanitize an image through the NSFW → face → OCR pipeline.
     * Returns sanitized image bytes (faces solid-filled, NSFW redacted, OCR text redacted).
     * Fail-open: returns original bytes if sanitization throws.
     */
    fun sanitizeImage(imageBytes: ByteArray): ByteArray {
        val startMs = System.currentTimeMillis()
        return try {
            val sanitized = uniffi.openobscure_core.sanitizeImage(handle, imageBytes)
            val elapsed = System.currentTimeMillis() - startMs
            // Drain pipeline debug log (face bbox, NSFW scores, OCR pre-filter)
            val debugLog = getDebugLog()
            if (debugLog.isNotEmpty()) {
                Log.d(TAG, "[IMAGE-PIPELINE] $debugLog")
            }
            Log.d(TAG, "[IMAGE] input=${imageBytes.size} bytes, output=${sanitized.size} bytes, ms=$elapsed")
            sanitized
        } catch (e: Exception) {
            val elapsed = System.currentTimeMillis() - startMs
            Log.d(TAG, "[IMAGE] sanitizeImage failed (${e.message}), passthrough, ms=$elapsed")
            imageBytes // fail-open: forward original
        }
    }

    /** Scan LLM response for persuasion/manipulation (cognitive firewall). */
    fun scanResponse(text: String): RiReportFfi? {
        val startMs = System.currentTimeMillis()
        val report = uniffi.openobscure_core.scanResponse(handle, text)
        val elapsed = System.currentTimeMillis() - startMs
        Log.d(TAG, "[RI] len=${text.length}, flagged=${report != null}, ms=$elapsed")
        return report
    }

    /**
     * Sanitize a full conversation history with caching.
     * Messages whose content matches a cached sanitized version skip NER entirely.
     * Only new/changed user messages go through the Rust sanitize() pipeline.
     * Assistant messages pass through unchanged (they have tokens from DB).
     */
    fun sanitizeMessages(messages: List<ChatMessageFfi>): SanitizeMessagesResultFfi {
        val startMs = System.currentTimeMillis()
        val resultMessages = mutableListOf<ChatMessageFfi>()
        var totalPii = 0u

        for ((idx, msg) in messages.withIndex()) {
            if (msg.role == "assistant" || msg.content.isEmpty()) {
                // Pass through unchanged
                resultMessages.add(msg)
                continue
            }

            // Check cache
            val cacheKey = msg.content.hashCode()
            val cached = sanitizeCache[cacheKey]
            if (cached != null) {
                Log.d(TAG, "[CACHE] msg[$idx] hit (${msg.content.length} chars)")
                resultMessages.add(ChatMessageFfi(role = msg.role, content = cached))
                continue
            }

            // New user message — run NER via single-message sanitize()
            val result = sanitize(msg.content)
            totalPii += result.piiCount
            val sanitizedContent = result.sanitizedText
            sanitizeCache[cacheKey] = sanitizedContent
            Log.d(TAG, "[CACHE] msg[$idx] miss, scanned (${msg.content.length} chars, pii=${result.piiCount})")
            resultMessages.add(ChatMessageFfi(role = msg.role, content = sanitizedContent))
        }

        val elapsed = System.currentTimeMillis() - startMs
        Log.d(TAG, "[SANITIZE] cached batch: ${messages.size} msgs, pii=$totalPii, ms=$elapsed")

        // Return in the same format as the Rust batch API
        val mappingJson = getMappingsJson()
        return SanitizeMessagesResultFfi(
            messages = resultMessages,
            piiCount = totalPii,
            mappingJson = mappingJson
        )
    }

    /**
     * Sanitize messages with isolated (disposable) mappings.
     * Used for auto-generated requests (title gen, suggestion gen) that should
     * not pollute the conversation's mapping pool. PII is still sanitized —
     * the LLM never sees real data — but the tokens are disposable and not
     * merged into accumulatedMappings.
     */
    fun sanitizeMessagesIsolated(messages: List<ChatMessageFfi>): SanitizeMessagesResultFfi {
        val startMs = System.currentTimeMillis()
        val resultMessages = mutableListOf<ChatMessageFfi>()
        var totalPii = 0u

        for ((idx, msg) in messages.withIndex()) {
            if (msg.role == "assistant" || msg.content.isEmpty()) {
                resultMessages.add(msg)
                continue
            }

            // Sanitize without accumulating — use the Rust single-message API directly
            val result = sanitizeText(handle, msg.content)
            totalPii += result.piiCount
            Log.d(TAG, "[SANITIZE-ISO] msg[$idx] len=${msg.content.length}, pii=${result.piiCount}")
            resultMessages.add(ChatMessageFfi(role = msg.role, content = result.sanitizedText))
            // NOTE: result.mappingJson is intentionally NOT merged into accumulatedMappings
        }

        val elapsed = System.currentTimeMillis() - startMs
        Log.d(TAG, "[SANITIZE-ISO] isolated batch: ${messages.size} msgs, pii=$totalPii, ms=$elapsed")

        return SanitizeMessagesResultFfi(
            messages = resultMessages,
            piiCount = totalPii,
            mappingJson = "[]"  // disposable — not persisted
        )
    }

    /** Reset mappings when starting a new conversation. */
    fun resetMappings() {
        Log.d(TAG, "[RESET] clearing ${accumulatedMappings.size} mappings, ${sanitizeCache.size} cached, ${riWarnings.size} ri flags")
        accumulatedMappings.clear()
        sanitizeCache.clear()
        riWarnings.clear()
        riVersion.intValue = 0
    }

    /**
     * Store an RI warning for a message (keyed by raw token text hash).
     * Called from ChatService.onSuccess when the cognitive firewall flags a response.
     */
    fun setRiWarning(rawTokenText: String, severity: String, categories: List<String>) {
        val label = formatRiWarningLabel(severity, categories)
        riWarnings[rawTokenText.hashCode()] = label
        riVersion.intValue++  // triggers Compose recomposition via remember key
        Log.d(TAG, "[RI-FLAG] stored warning for hash=${rawTokenText.hashCode()}, severity=$severity, version=${riVersion.intValue}")
    }

    /**
     * Get the RI warning label for a message, or null if not flagged.
     * Called from Compose rememberRestoredText() at render time.
     */
    fun getRiWarning(rawTokenText: String): String? {
        return riWarnings[rawTokenText.hashCode()]
    }

    private fun formatRiWarningLabel(severity: String, categories: List<String>): String {
        val tactics = if (categories.isEmpty()) severity else categories.joinToString(" • ")
        return when (severity) {
            "Caution" -> "--- OpenObscure WARNING ---\nDetected: $tactics\nRecommendation: Pause and verify with objective evidence before acting.\n---\n\n"
            "Warning" -> "--- OpenObscure WARNING ---\nDetected: $tactics\nThis response contains language patterns associated with influence tactics.\n---\n\n"
            else -> "--- OpenObscure WARNING ---\nDetected: $tactics\n---\n\n"
        }
    }

    /** Serialize current mappings for persistent storage (e.g., alongside conversation in DB). */
    fun getMappingsJson(): String {
        return try {
            org.json.JSONArray(accumulatedMappings.map { org.json.JSONArray(it) }).toString()
        } catch (_: Exception) { "[]" }
    }

    /** Load previously saved mappings (e.g., when switching to a conversation from DB). */
    fun loadMappings(json: String) {
        accumulatedMappings.clear()
        try {
            val arr = org.json.JSONArray(json)
            for (i in 0 until arr.length()) {
                val pair = arr.getJSONArray(i)
                val list = mutableListOf<String>()
                for (j in 0 until pair.length()) {
                    list.add(pair.getString(j))
                }
                accumulatedMappings.add(list)
            }
        } catch (_: Exception) { /* ignore parse errors */ }
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
