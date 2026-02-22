import Foundation
import OpenObscure

// Simple test runner that works without XCTest or Swift Testing.
// Run with: cd test/apps/ios && swift run RunTests

// 32-byte test key as hex (64 hex chars)
let testKeyHex = String(repeating: "42", count: 32)

var passed = 0
var failed = 0

func test(_ name: String, _ body: () throws -> Void) {
    do {
        try body()
        passed += 1
        print("  PASS  \(name)")
    } catch {
        failed += 1
        print("  FAIL  \(name): \(error)")
    }
}

func expect(_ condition: Bool, _ message: String = "", file: String = #file, line: Int = #line) throws {
    guard condition else {
        throw TestError.assertion("Assertion failed at \(file):\(line) \(message)")
    }
}

enum TestError: Error, CustomStringConvertible {
    case assertion(String)
    var description: String {
        switch self { case .assertion(let msg): return msg }
    }
}

print("=== OpenObscure Mobile Integration Tests ===")
print("")

// -- Initialization --

print("Initialization:")

test("createWithDefaults") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    try expect(handle != nil)
}

test("createWithExplicitConfig") {
    let config = #"{"keywords_enabled": true, "scanner_mode": "regex", "auto_detect": false}"#
    let handle = try createOpenobscure(configJson: config, fpeKeyHex: testKeyHex)
    let stats = getStats(handle: handle)
    try expect(stats.scannerMode == "regex", "Expected regex, got \(stats.scannerMode)")
    try expect(stats.deviceTier == "manual", "Expected manual, got \(stats.deviceTier)")
}

test("createWithInvalidKeyFails") {
    do {
        _ = try createOpenobscure(configJson: "{}", fpeKeyHex: "short")
        throw TestError.assertion("Expected error for short key")
    } catch is MobileBindingError {
        // expected
    }
}

test("createWithInvalidJsonFails") {
    do {
        _ = try createOpenobscure(configJson: "not json", fpeKeyHex: testKeyHex)
        throw TestError.assertion("Expected error for invalid JSON")
    } catch is MobileBindingError {
        // expected
    }
}

// -- Text Sanitization --

print("\nText Sanitization:")

test("sanitizeNoPii") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    let result = try sanitizeText(handle: handle, text: "Hello world, no PII here")
    try expect(result.piiCount == 0, "piiCount = \(result.piiCount)")
    try expect(result.sanitizedText == "Hello world, no PII here")
    try expect(result.categories.isEmpty, "categories not empty")
}

test("sanitizeCreditCard") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    let result = try sanitizeText(handle: handle, text: "My card is 4111-1111-1111-1111")
    try expect(result.piiCount >= 1, "piiCount = \(result.piiCount)")
    try expect(!result.sanitizedText.contains("4111-1111-1111-1111"),
        "Original CC still present: \(result.sanitizedText)")
    try expect(result.categories.contains("credit_card"),
        "Missing credit_card category: \(result.categories)")
}

test("sanitizeEmail") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    let result = try sanitizeText(handle: handle, text: "Email: john.doe@example.com")
    try expect(result.piiCount >= 1)
    try expect(!result.sanitizedText.contains("john.doe@example.com"))
}

test("sanitizeSsn") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    let result = try sanitizeText(handle: handle, text: "SSN: 123-45-6789")
    try expect(result.piiCount >= 1)
    try expect(!result.sanitizedText.contains("123-45-6789"))
}

test("sanitizeMultiplePii") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    let result = try sanitizeText(
        handle: handle,
        text: "Card 4111-1111-1111-1111 and SSN 123-45-6789"
    )
    try expect(result.piiCount >= 2, "piiCount = \(result.piiCount)")
    try expect(!result.sanitizedText.contains("4111"))
    try expect(!result.sanitizedText.contains("123-45-6789"))
}

test("sanitizeEmptyText") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    let result = try sanitizeText(handle: handle, text: "")
    try expect(result.piiCount == 0)
    try expect(result.sanitizedText == "")
}

// -- Text Restoration --

print("\nText Restoration:")

test("restoreRoundTrip") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    let sanitized = try sanitizeText(handle: handle, text: "My card is 4111-1111-1111-1111")
    try expect(sanitized.piiCount >= 1)

    let restored = restoreText(
        handle: handle,
        text: sanitized.sanitizedText,
        mappingJson: sanitized.mappingJson
    )
    try expect(restored.contains("4111-1111-1111-1111"),
        "Restored text missing original CC: \(restored)")
}

test("restoreWithEmptyMapping") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    let result = restoreText(handle: handle, text: "Hello world", mappingJson: "{}")
    try expect(result == "Hello world")
}

test("restoreWithInvalidJson") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    let result = restoreText(handle: handle, text: "Hello world", mappingJson: "not json")
    try expect(result == "Hello world")
}

// -- Statistics --

print("\nStatistics:")

test("statsInitialState") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    let stats = getStats(handle: handle)
    try expect(stats.totalPiiFound == 0)
    try expect(stats.totalImagesProcessed == 0)
    try expect(!stats.imagePipelineAvailable)
    try expect(!stats.deviceTier.isEmpty)
}

test("statsAfterSanitize") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    _ = try sanitizeText(handle: handle, text: "Card: 4111-1111-1111-1111")
    let stats = getStats(handle: handle)
    try expect(stats.totalPiiFound >= 1)
}

test("statsAccumulate") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    _ = try sanitizeText(handle: handle, text: "Card: 4111-1111-1111-1111")
    _ = try sanitizeText(handle: handle, text: "SSN: 123-45-6789")
    let stats = getStats(handle: handle)
    try expect(stats.totalPiiFound >= 2, "totalPiiFound = \(stats.totalPiiFound)")
}

test("deviceTierIsValid") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    let stats = getStats(handle: handle)
    let validTiers = ["full", "standard", "lite", "manual"]
    try expect(validTiers.contains(stats.deviceTier),
        "Unexpected device tier: \(stats.deviceTier)")
}

// -- Image Sanitization --

print("\nImage Sanitization:")

test("imageNotEnabledReturnsError") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    do {
        _ = try sanitizeImage(handle: handle, imageBytes: Data([0xFF, 0xD8, 0xFF]))
        throw TestError.assertion("Expected error for disabled image pipeline")
    } catch let error as MobileBindingError {
        switch error {
        case .Processing(let msg):
            try expect(msg.contains("not enabled"), "Wrong error message: \(msg)")
        default:
            throw TestError.assertion("Expected Processing error, got \(error)")
        }
    }
}

test("imageEnabledRejectsTruncatedJpeg") {
    let handle = try createOpenobscure(
        configJson: #"{"image_enabled": true}"#,
        fpeKeyHex: testKeyHex
    )
    do {
        _ = try sanitizeImage(handle: handle, imageBytes: Data([0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]))
        throw TestError.assertion("Expected error for truncated JPEG")
    } catch is MobileBindingError {
        // expected — truncated image can't be decoded
    }
}

test("imageEnabledRejectsTruncatedPng") {
    let handle = try createOpenobscure(
        configJson: #"{"image_enabled": true}"#,
        fpeKeyHex: testKeyHex
    )
    do {
        _ = try sanitizeImage(handle: handle, imageBytes: Data([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]))
        throw TestError.assertion("Expected error for truncated PNG")
    } catch is MobileBindingError {
        // expected — truncated image can't be decoded
    }
}

test("imageEnabledRejectsEmptyData") {
    let handle = try createOpenobscure(
        configJson: #"{"image_enabled": true}"#,
        fpeKeyHex: testKeyHex
    )
    do {
        _ = try sanitizeImage(handle: handle, imageBytes: Data())
        throw TestError.assertion("Expected error for empty image data")
    } catch is MobileBindingError {
        // expected — empty data isn't a valid image
    }
}

test("imageStatsZeroWhenDisabled") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    let stats = getStats(handle: handle)
    try expect(stats.totalImagesProcessed == 0, "totalImagesProcessed should be 0")
    try expect(!stats.imagePipelineAvailable, "imagePipelineAvailable should be false")
}

test("imageStatsShowPipelineAvailableWhenEnabled") {
    let handle = try createOpenobscure(
        configJson: #"{"image_enabled": true}"#,
        fpeKeyHex: testKeyHex
    )
    let stats = getStats(handle: handle)
    try expect(stats.imagePipelineAvailable, "imagePipelineAvailable should be true")
}

// -- FPE Format Preservation --

print("\nFPE Format Preservation:")

test("fpePreservesCardFormat") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    let result = try sanitizeText(handle: handle, text: "Card: 4111-1111-1111-1111")
    let sanitized = result.sanitizedText
    try expect(!sanitized.contains("4111-1111-1111-1111"))

    // FPE should preserve the digit-dash format: XXXX-XXXX-XXXX-XXXX
    let pattern = #"Card: \d{4}-\d{4}-\d{4}-\d{4}"#
    let regex = try NSRegularExpression(pattern: pattern)
    let range = NSRange(sanitized.startIndex..., in: sanitized)
    try expect(regex.firstMatch(in: sanitized, range: range) != nil,
        "FPE should preserve card format, got: \(sanitized)")
}

// -- Thread Safety --

print("\nThread Safety:")

test("concurrentSanitize") {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: testKeyHex)
    let group = DispatchGroup()
    var errors: [String] = []
    let lock = NSLock()

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
                    errors.append("Thread \(i): piiCount = \(result.piiCount)")
                    lock.unlock()
                }
            } catch {
                lock.lock()
                errors.append("Thread \(i): \(error)")
                lock.unlock()
            }
        }
    }

    group.wait()
    try expect(errors.isEmpty, "Concurrent errors: \(errors)")

    let stats = getStats(handle: handle)
    try expect(stats.totalPiiFound >= 10, "totalPiiFound = \(stats.totalPiiFound)")
}

// -- Summary --

print("")
print("=== Results: \(passed) passed, \(failed) failed ===")

if failed > 0 {
    exit(1)
}
