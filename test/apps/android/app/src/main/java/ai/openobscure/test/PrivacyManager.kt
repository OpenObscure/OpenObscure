package ai.openobscure.test

import uniffi.openobscure_core.OpenObscureMobile
import uniffi.openobscure_core.SanitizeResultFfi
import uniffi.openobscure_core.MobileStatsFfi
import uniffi.openobscure_core.MobileBindingException
import uniffi.openobscure_core.createOpenobscure
import uniffi.openobscure_core.sanitizeText
import uniffi.openobscure_core.restoreText
import uniffi.openobscure_core.sanitizeImage
import uniffi.openobscure_core.getStats

/**
 * High-level wrapper around UniFFI-generated OpenObscure bindings.
 *
 * Provides a Kotlin-idiomatic API for the Android test app and
 * instrumented tests. Mirrors the iOS PrivacyManager.swift.
 */
class PrivacyManager(configJson: String = "{}", fpeKeyHex: String) {

    private val handle: OpenObscureMobile = createOpenobscure(
        configJson = configJson,
        fpeKeyHex = fpeKeyHex,
    )

    /** Scan text for PII, returning sanitized text + mapping. */
    fun sanitize(text: String): SanitizeResultFfi {
        return sanitizeText(handle = handle, text = text)
    }

    /** Restore original PII from sanitized text + mapping JSON. */
    fun restore(text: String, mappingJson: String): String {
        return restoreText(handle = handle, text = text, mappingJson = mappingJson)
    }

    /** Process image bytes for visual PII (face blur, OCR, EXIF strip). */
    fun sanitizeImage(imageBytes: ByteArray): ByteArray {
        return sanitizeImage(handle = handle, imageBytes = imageBytes)
    }

    /** Get current statistics. */
    fun stats(): MobileStatsFfi {
        return getStats(handle = handle)
    }
}
