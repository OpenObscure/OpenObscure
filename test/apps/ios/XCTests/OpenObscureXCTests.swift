import XCTest
import OpenObscure

/// XCTest wrapper for OpenObscure iOS integration tests.
///
/// Run with: xcodebuild test -scheme OpenObscureTests -destination 'platform=macOS'
/// Or from Xcode: Cmd+U on the test target.
///
/// These tests mirror the custom runner in OpenObscureTests/OpenObscureTests.swift
/// but use the XCTest framework for Xcode CI integration.
final class OpenObscureXCTests: XCTestCase {

    // 32-byte test key as hex (64 hex chars)
    private let testKeyHex = String(repeating: "42", count: 32)

    // MARK: - Initialization

    func testCreateWithDefaults() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        XCTAssertNotNil(handle)
    }

    func testCreateWithExplicitConfig() throws {
        let config = #"{"keywords_enabled": true, "scanner_mode": "regex", "auto_detect": false}"#
        let handle = try createOpenobscure(configJson: config, fpeKeyHex: testKeyHex)
        let stats = getStats(handle: handle)
        XCTAssertEqual(stats.scannerMode, "regex")
        XCTAssertEqual(stats.deviceTier, "manual")
    }

    func testCreateWithInvalidKeyFails() {
        XCTAssertThrowsError(try createOpenobscure(configJson: "{}", fpeKeyHex: "short")) { error in
            XCTAssertTrue(error is MobileBindingError)
        }
    }

    func testCreateWithInvalidJsonFails() {
        XCTAssertThrowsError(try createOpenobscure(configJson: "not json", fpeKeyHex: testKeyHex)) { error in
            XCTAssertTrue(error is MobileBindingError)
        }
    }

    // MARK: - Text Sanitization

    func testSanitizeNoPii() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        let result = try sanitizeText(handle: handle, text: "Hello world, no PII here")
        XCTAssertEqual(result.piiCount, 0)
        XCTAssertEqual(result.sanitizedText, "Hello world, no PII here")
        XCTAssertTrue(result.categories.isEmpty)
    }

    func testSanitizeCreditCard() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        let result = try sanitizeText(handle: handle, text: "My card is 4111-1111-1111-1111")
        XCTAssertGreaterThanOrEqual(result.piiCount, 1)
        XCTAssertFalse(result.sanitizedText.contains("4111-1111-1111-1111"),
            "Original CC still present: \(result.sanitizedText)")
        XCTAssertTrue(result.categories.contains("credit_card"),
            "Missing credit_card category: \(result.categories)")
    }

    func testSanitizeEmail() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        let result = try sanitizeText(handle: handle, text: "Email: john.doe@example.com")
        XCTAssertGreaterThanOrEqual(result.piiCount, 1)
        XCTAssertFalse(result.sanitizedText.contains("john.doe@example.com"))
    }

    func testSanitizeSsn() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        let result = try sanitizeText(handle: handle, text: "SSN: 123-45-6789")
        XCTAssertGreaterThanOrEqual(result.piiCount, 1)
        XCTAssertFalse(result.sanitizedText.contains("123-45-6789"))
    }

    func testSanitizeMultiplePii() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        let result = try sanitizeText(
            handle: handle,
            text: "Card 4111-1111-1111-1111 and SSN 123-45-6789"
        )
        XCTAssertGreaterThanOrEqual(result.piiCount, 2)
        XCTAssertFalse(result.sanitizedText.contains("4111"))
        XCTAssertFalse(result.sanitizedText.contains("123-45-6789"))
    }

    func testSanitizeEmptyText() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        let result = try sanitizeText(handle: handle, text: "")
        XCTAssertEqual(result.piiCount, 0)
        XCTAssertEqual(result.sanitizedText, "")
    }

    // MARK: - Text Restoration

    func testRestoreRoundTrip() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        let sanitized = try sanitizeText(handle: handle, text: "My card is 4111-1111-1111-1111")
        XCTAssertGreaterThanOrEqual(sanitized.piiCount, 1)

        let restored = restoreText(
            handle: handle,
            text: sanitized.sanitizedText,
            mappingJson: sanitized.mappingJson
        )
        XCTAssertTrue(restored.contains("4111-1111-1111-1111"),
            "Restored text missing original CC: \(restored)")
    }

    func testRestoreWithEmptyMapping() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        let result = restoreText(handle: handle, text: "Hello world", mappingJson: "{}")
        XCTAssertEqual(result, "Hello world")
    }

    func testRestoreWithInvalidJson() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        let result = restoreText(handle: handle, text: "Hello world", mappingJson: "not json")
        XCTAssertEqual(result, "Hello world")
    }

    // MARK: - Statistics

    func testStatsInitialState() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        let stats = getStats(handle: handle)
        XCTAssertEqual(stats.totalPiiFound, 0)
        XCTAssertEqual(stats.totalImagesProcessed, 0)
        XCTAssertFalse(stats.imagePipelineAvailable)
        XCTAssertFalse(stats.deviceTier.isEmpty)
    }

    func testStatsAfterSanitize() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        _ = try sanitizeText(handle: handle, text: "Card: 4111-1111-1111-1111")
        let stats = getStats(handle: handle)
        XCTAssertGreaterThanOrEqual(stats.totalPiiFound, 1)
    }

    func testStatsAccumulate() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        _ = try sanitizeText(handle: handle, text: "Card: 4111-1111-1111-1111")
        _ = try sanitizeText(handle: handle, text: "SSN: 123-45-6789")
        let stats = getStats(handle: handle)
        XCTAssertGreaterThanOrEqual(stats.totalPiiFound, 2)
    }

    func testDeviceTierIsValid() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        let stats = getStats(handle: handle)
        let validTiers = ["full", "standard", "lite", "manual"]
        XCTAssertTrue(validTiers.contains(stats.deviceTier),
            "Unexpected device tier: \(stats.deviceTier)")
    }

    // MARK: - Image Sanitization

    func testImageNotEnabledReturnsError() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        XCTAssertThrowsError(
            try sanitizeImage(handle: handle, imageBytes: Data([0xFF, 0xD8, 0xFF]))
        ) { error in
            if case MobileBindingError.Processing(let msg) = error {
                XCTAssertTrue(msg.contains("not enabled"), "Wrong error message: \(msg)")
            } else {
                XCTFail("Expected Processing error, got \(error)")
            }
        }
    }

    // MARK: - FPE Format Preservation

    func testFpePreservesCardFormat() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        let result = try sanitizeText(handle: handle, text: "Card: 4111-1111-1111-1111")
        XCTAssertFalse(result.sanitizedText.contains("4111-1111-1111-1111"))

        let pattern = #"Card: \d{4}-\d{4}-\d{4}-\d{4}"#
        let regex = try NSRegularExpression(pattern: pattern)
        let range = NSRange(result.sanitizedText.startIndex..., in: result.sanitizedText)
        XCTAssertNotNil(regex.firstMatch(in: result.sanitizedText, range: range),
            "FPE should preserve card format, got: \(result.sanitizedText)")
    }

    // MARK: - Thread Safety

    func testConcurrentSanitize() throws {
        let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
        let errors = NSMutableArray() // thread-safe via lock below
        let lock = NSLock()
        let group = DispatchGroup()

        for i in 0..<10 {
            group.enter()
            DispatchQueue.global().async {
                defer { group.leave() }
                do {
                    let result = try sanitizeText(
                        handle: handle,
                        text: "Card \(i): 4111-1111-1111-1111"
                    )
                    if result.piiCount < 1 {
                        lock.lock()
                        errors.add("Thread \(i): piiCount = \(result.piiCount)")
                        lock.unlock()
                    }
                } catch {
                    lock.lock()
                    errors.add("Thread \(i): \(error)")
                    lock.unlock()
                }
            }
        }

        group.wait()
        XCTAssertEqual(errors.count, 0, "Concurrent errors: \(errors)")

        let stats = getStats(handle: handle)
        XCTAssertGreaterThanOrEqual(stats.totalPiiFound, 10)
    }

    // MARK: - Image Pipeline (with bundled test data)

    func testSanitizeImageWithJpegHeader() throws {
        // Minimal JPEG header — should fail with image decode error
        // (not "not enabled" since we're testing the pipeline rejection path)
        let handle = try createOpenobscure(
            configJson: #"{"image_enabled": true}"#,
            fpeKeyHex: testKeyHex
        )
        let minimalJpeg = Data([0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]) // truncated JPEG
        XCTAssertThrowsError(try sanitizeImage(handle: handle, imageBytes: minimalJpeg))
    }

    func testSanitizeImageWithPngHeader() throws {
        // Minimal PNG header — should fail with decode error
        let handle = try createOpenobscure(
            configJson: #"{"image_enabled": true}"#,
            fpeKeyHex: testKeyHex
        )
        let pngHeader = Data([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) // PNG magic
        XCTAssertThrowsError(try sanitizeImage(handle: handle, imageBytes: pngHeader))
    }

    func testSanitizeImageEmptyData() throws {
        let handle = try createOpenobscure(
            configJson: #"{"image_enabled": true}"#,
            fpeKeyHex: testKeyHex
        )
        XCTAssertThrowsError(try sanitizeImage(handle: handle, imageBytes: Data()))
    }
}
