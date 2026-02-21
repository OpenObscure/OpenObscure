package ai.openobscure.test

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.openobscure_proxy.MobileBindingException
import uniffi.openobscure_proxy.createOpenobscure
import uniffi.openobscure_proxy.getStats
import uniffi.openobscure_proxy.sanitizeText
import uniffi.openobscure_proxy.restoreText
import uniffi.openobscure_proxy.sanitizeImage
import java.util.concurrent.CountDownLatch
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.concurrent.thread

/**
 * Instrumented tests for OpenObscure Android bindings.
 *
 * Run with:
 *   cd test-apps/android && ./gradlew connectedAndroidTest
 *
 * Mirrors the iOS test suite at test-apps/ios/OpenObscureTests/.
 */
@RunWith(AndroidJUnit4::class)
class OpenObscureInstrumentedTest {

    // 32-byte test key as hex (64 hex chars)
    private val testKeyHex = "42".repeat(32)

    // ── JNI Library Loading ──

    @Test
    fun jniLibraryLoads() {
        // UniFFI's generated code loads the library via JNA when first called.
        // If this throws UnsatisfiedLinkError, the .so is missing from jniLibs/.
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        assertNotNull(handle)
    }

    // ── Initialization ──

    @Test
    fun createWithDefaults() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        assertNotNull(handle)
    }

    @Test
    fun createWithExplicitConfig() {
        val config = """{"keywords_enabled": true, "scanner_mode": "regex", "auto_detect": false}"""
        val handle = createOpenobscure(configJson = config, fpeKeyHex = testKeyHex)
        val stats = getStats(handle = handle)
        assertEquals("regex", stats.scannerMode)
        assertEquals("manual", stats.deviceTier)
    }

    @Test(expected = MobileBindingException.InvalidKey::class)
    fun createWithInvalidKeyFails() {
        createOpenobscure(configJson = "{}", fpeKeyHex = "short")
    }

    @Test(expected = MobileBindingException.Config::class)
    fun createWithInvalidJsonFails() {
        createOpenobscure(configJson = "not json", fpeKeyHex = testKeyHex)
    }

    // ── Text Sanitization ──

    @Test
    fun sanitizeNoPii() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        val result = sanitizeText(handle = handle, text = "Hello world, no PII here")
        assertEquals(0u, result.piiCount)
        assertEquals("Hello world, no PII here", result.sanitizedText)
        assertTrue(result.categories.isEmpty())
    }

    @Test
    fun sanitizeCreditCard() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        val result = sanitizeText(handle = handle, text = "My card is 4111-1111-1111-1111")
        assertTrue("piiCount should be >= 1, got ${result.piiCount}", result.piiCount >= 1u)
        assertFalse(
            "Original CC still present: ${result.sanitizedText}",
            result.sanitizedText.contains("4111-1111-1111-1111"),
        )
        assertTrue(
            "Missing credit_card category: ${result.categories}",
            result.categories.contains("credit_card"),
        )
    }

    @Test
    fun sanitizeEmail() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        val result = sanitizeText(handle = handle, text = "Email: john.doe@example.com")
        assertTrue(result.piiCount >= 1u)
        assertFalse(result.sanitizedText.contains("john.doe@example.com"))
    }

    @Test
    fun sanitizeSsn() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        val result = sanitizeText(handle = handle, text = "SSN: 123-45-6789")
        assertTrue(result.piiCount >= 1u)
        assertFalse(result.sanitizedText.contains("123-45-6789"))
    }

    @Test
    fun sanitizeMultiplePii() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        val result = sanitizeText(
            handle = handle,
            text = "Card 4111-1111-1111-1111 and SSN 123-45-6789",
        )
        assertTrue("piiCount should be >= 2, got ${result.piiCount}", result.piiCount >= 2u)
        assertFalse(result.sanitizedText.contains("4111"))
        assertFalse(result.sanitizedText.contains("123-45-6789"))
    }

    @Test
    fun sanitizeEmptyText() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        val result = sanitizeText(handle = handle, text = "")
        assertEquals(0u, result.piiCount)
        assertEquals("", result.sanitizedText)
    }

    // ── Text Restoration ──

    @Test
    fun restoreRoundTrip() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        val sanitized = sanitizeText(handle = handle, text = "My card is 4111-1111-1111-1111")
        assertTrue(sanitized.piiCount >= 1u)

        val restored = restoreText(
            handle = handle,
            text = sanitized.sanitizedText,
            mappingJson = sanitized.mappingJson,
        )
        assertTrue(
            "Restored text missing original CC: $restored",
            restored.contains("4111-1111-1111-1111"),
        )
    }

    @Test
    fun restoreWithEmptyMapping() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        val result = restoreText(handle = handle, text = "Hello world", mappingJson = "{}")
        assertEquals("Hello world", result)
    }

    @Test
    fun restoreWithInvalidJson() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        val result = restoreText(handle = handle, text = "Hello world", mappingJson = "not json")
        assertEquals("Hello world", result)
    }

    // ── Statistics ──

    @Test
    fun statsInitialState() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        val stats = getStats(handle = handle)
        assertEquals(0uL, stats.totalPiiFound)
        assertEquals(0uL, stats.totalImagesProcessed)
        assertFalse(stats.imagePipelineAvailable)
        assertTrue(stats.deviceTier.isNotEmpty())
    }

    @Test
    fun statsAfterSanitize() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        sanitizeText(handle = handle, text = "Card: 4111-1111-1111-1111")
        val stats = getStats(handle = handle)
        assertTrue("totalPiiFound should be >= 1", stats.totalPiiFound >= 1uL)
    }

    @Test
    fun statsAccumulate() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        sanitizeText(handle = handle, text = "Card: 4111-1111-1111-1111")
        sanitizeText(handle = handle, text = "SSN: 123-45-6789")
        val stats = getStats(handle = handle)
        assertTrue("totalPiiFound should be >= 2, got ${stats.totalPiiFound}", stats.totalPiiFound >= 2uL)
    }

    @Test
    fun deviceTierIsValid() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        val stats = getStats(handle = handle)
        val validTiers = setOf("full", "standard", "lite", "manual")
        assertTrue(
            "Unexpected device tier: ${stats.deviceTier}",
            stats.deviceTier in validTiers,
        )
    }

    // ── Image Sanitization ──

    @Test
    fun imageNotEnabledReturnsError() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        try {
            sanitizeImage(handle = handle, imageBytes = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0xFF.toByte()))
            fail("Expected Processing error for disabled image pipeline")
        } catch (e: MobileBindingException.Processing) {
            assertTrue("Wrong error message: ${e.v1}", e.v1.contains("not enabled"))
        }
    }

    // ── FPE Format Preservation ──

    @Test
    fun fpePreservesCardFormat() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        val result = sanitizeText(handle = handle, text = "Card: 4111-1111-1111-1111")
        assertFalse(result.sanitizedText.contains("4111-1111-1111-1111"))

        // FPE should preserve the digit-dash format: XXXX-XXXX-XXXX-XXXX
        val pattern = Regex("""Card: \d{4}-\d{4}-\d{4}-\d{4}""")
        assertTrue(
            "FPE should preserve card format, got: ${result.sanitizedText}",
            pattern.containsMatchIn(result.sanitizedText),
        )
    }

    // ── Thread Safety ──

    @Test
    fun concurrentSanitize() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        val errors = CopyOnWriteArrayList<String>()
        val latch = CountDownLatch(10)

        for (i in 0 until 10) {
            thread {
                try {
                    val result = sanitizeText(
                        handle = handle,
                        text = "Card $i: 4111-1111-1111-1111",
                    )
                    if (result.piiCount < 1u) {
                        errors.add("Thread $i: piiCount = ${result.piiCount}")
                    }
                } catch (e: Exception) {
                    errors.add("Thread $i: ${e.message}")
                } finally {
                    latch.countDown()
                }
            }
        }

        latch.await()
        assertTrue("Concurrent errors: $errors", errors.isEmpty())

        val stats = getStats(handle = handle)
        assertTrue("totalPiiFound should be >= 10, got ${stats.totalPiiFound}", stats.totalPiiFound >= 10uL)
    }

    // ── Memory Footprint ──

    @Test
    fun memoryFootprintUnderLimit() {
        // Verify that creating a handle + sanitizing text stays under 60MB.
        // This is a rough check — the Android Profiler gives more precise numbers.
        val runtime = Runtime.getRuntime()
        runtime.gc()
        val before = runtime.totalMemory() - runtime.freeMemory()

        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        for (i in 0 until 100) {
            sanitizeText(handle = handle, text = "Card $i: 4111-1111-1111-1111 and SSN 123-45-6789")
        }

        runtime.gc()
        val after = runtime.totalMemory() - runtime.freeMemory()
        val usedMb = (after - before) / (1024 * 1024)

        // Lite tier target: < 60MB. We check managed heap only (native heap
        // is harder to measure without Debug.getNativeHeapAllocatedSize()).
        assertTrue("Managed heap growth ${usedMb}MB exceeds 60MB", usedMb < 60)
    }

    // ── Image Pipeline (#11) ──

    @Test
    fun imageDisabledRejectsValidJpeg() {
        // With image_enabled=false (default), even valid image bytes are rejected
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        try {
            sanitizeImage(handle = handle, imageBytes = byteArrayOf(
                0xFF.toByte(), 0xD8.toByte(), 0xFF.toByte(), 0xE0.toByte()
            ))
            fail("Expected error for disabled image pipeline")
        } catch (e: MobileBindingException.Processing) {
            assertTrue("Wrong error: ${e.v1}", e.v1.contains("not enabled"))
        }
    }

    @Test
    fun imageEnabledRejectsTruncatedJpeg() {
        // With image_enabled=true, truncated JPEG should fail with decode error
        val config = """{"image_enabled": true}"""
        val handle = createOpenobscure(configJson = config, fpeKeyHex = testKeyHex)
        try {
            sanitizeImage(handle = handle, imageBytes = byteArrayOf(
                0xFF.toByte(), 0xD8.toByte(), 0xFF.toByte(), 0xE0.toByte(),
                0x00, 0x10
            ))
            fail("Expected error for truncated JPEG")
        } catch (e: MobileBindingException) {
            // Expected — truncated image can't be decoded
        }
    }

    @Test
    fun imageEnabledRejectsTruncatedPng() {
        // With image_enabled=true, truncated PNG should fail with decode error
        val config = """{"image_enabled": true}"""
        val handle = createOpenobscure(configJson = config, fpeKeyHex = testKeyHex)
        try {
            sanitizeImage(handle = handle, imageBytes = byteArrayOf(
                0x89.toByte(), 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A // PNG magic only
            ))
            fail("Expected error for truncated PNG")
        } catch (e: MobileBindingException) {
            // Expected — truncated image can't be decoded
        }
    }

    @Test
    fun imageEnabledRejectsEmptyData() {
        val config = """{"image_enabled": true}"""
        val handle = createOpenobscure(configJson = config, fpeKeyHex = testKeyHex)
        try {
            sanitizeImage(handle = handle, imageBytes = byteArrayOf())
            fail("Expected error for empty image data")
        } catch (e: MobileBindingException) {
            // Expected — empty data can't be an image
        }
    }

    @Test
    fun imageEnabledRejectsRandomBytes() {
        val config = """{"image_enabled": true}"""
        val handle = createOpenobscure(configJson = config, fpeKeyHex = testKeyHex)
        try {
            sanitizeImage(handle = handle, imageBytes = ByteArray(256) { it.toByte() })
            fail("Expected error for random bytes")
        } catch (e: MobileBindingException) {
            // Expected — random bytes aren't a valid image
        }
    }

    @Test
    fun imageStatsZeroWhenDisabled() {
        val handle = createOpenobscure(configJson = "{}", fpeKeyHex = testKeyHex)
        val stats = getStats(handle = handle)
        assertEquals(0uL, stats.totalImagesProcessed)
        assertFalse(stats.imagePipelineAvailable)
    }

    @Test
    fun imageStatsShowPipelineAvailableWhenEnabled() {
        val config = """{"image_enabled": true}"""
        val handle = createOpenobscure(configJson = config, fpeKeyHex = testKeyHex)
        val stats = getStats(handle = handle)
        assertTrue("imagePipelineAvailable should be true", stats.imagePipelineAvailable)
    }
}
