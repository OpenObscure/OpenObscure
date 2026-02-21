package ai.openobscure.test

import uniffi.openobscure_proxy.OpenObscureMobile
import uniffi.openobscure_proxy.SanitizeResultFfi
import uniffi.openobscure_proxy.MobileStatsFfi
import uniffi.openobscure_proxy.MobileBindingException
import uniffi.openobscure_proxy.createOpenobscure
import uniffi.openobscure_proxy.sanitizeText
import uniffi.openobscure_proxy.restoreText
import uniffi.openobscure_proxy.sanitizeImage
import uniffi.openobscure_proxy.getStats

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
