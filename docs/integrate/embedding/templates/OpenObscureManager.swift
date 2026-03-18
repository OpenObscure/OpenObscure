// OpenObscureManager.swift — Template for iOS/macOS integration
// Copy this file into your Xcode project and adjust the service identifier.

import Foundation
import Security
import OpenObscureLib

final class OpenObscureManager {
    static let shared = OpenObscureManager()
    let handle: OpenObscureHandle
    /// Accumulated token→plaintext mappings across all sanitize calls in a conversation.
    private var accumulatedMappings: [[String]] = []

    private init() {
        let key = OpenObscureManager.getOrCreateKey()
        // Bundle all models under an "OpenObscureModels" folder reference in Xcode.
        // The tier system auto-detects device RAM and loads only what fits:
        //   Full (≥8 GB) → DistilBERT NER, SCRFD, full OCR, NSFW, RI
        //   Standard (4–8 GB) → TinyBERT NER, SCRFD, detect-only OCR
        //   Lite (<4 GB) → TinyBERT NER, BlazeFace, minimal pipeline
        // EXIF metadata is always stripped from images regardless of tier.
        let modelsDir = Bundle.main.resourcePath.map { $0 + "/OpenObscureModels" }
            ?? Bundle.main.bundlePath + "/Contents/Resources/OpenObscureModels"
        handle = try! createOpenobscure(
            configJson: """
            {"scanner_mode": "auto", "models_base_dir": "\(modelsDir)"}
            """,
            fpeKeyHex: key
        )

        // Drain Rust-side debug log — model loading diagnostics, verification results.
        // On iOS, stderr goes to /dev/null so this is the only way to see Rust logs.
        let debugLog = getDebugLog()
        if !debugLog.isEmpty {
            print("[OpenObscure] \(debugLog)")
        }
    }

    /// Sanitize a full conversation history — user + system messages are sanitized,
    /// assistant messages pass through unchanged (they already contain FPE tokens from DB).
    ///
    /// **Important:** The caller must store raw LLM responses (with FPE tokens) in the DB,
    /// not restored plaintext. Use `restore()` only for display. This ensures assistant
    /// messages in history never leak plaintext PII to the LLM on subsequent turns.
    func sanitizeMessages(_ messages: [(role: String, content: String)]) -> [(role: String, content: String)] {
        let ffiMessages = messages.map { ChatMessageFfi(role: $0.role, content: $0.content) }
        let result = try! OpenObscureLib.sanitizeMessages(handle: handle, messages: ffiMessages)
        if result.piiCount > 0 {
            mergeMappings(result.mappingJson)
        }
        return result.messages.map { ($0.role, $0.content) }
    }

    /// Sanitize text — returns (sanitized text, PII count).
    /// Mappings are accumulated across calls for the current conversation.
    func sanitize(_ text: String) -> (sanitizedText: String, piiCount: UInt32) {
        let result = try! sanitizeText(handle: handle, text: text)
        if result.piiCount > 0 {
            mergeMappings(result.mappingJson)
        }
        return (result.sanitizedText, result.piiCount)
    }

    /// Restore PII in LLM response text using accumulated mappings.
    func restore(_ text: String) -> String {
        let json = (try? JSONSerialization.data(withJSONObject: accumulatedMappings)) ?? Data("{}".utf8)
        return restoreText(handle: handle, text: text, mappingJson: String(data: json, encoding: .utf8) ?? "{}")
    }

    /// Scan LLM response for persuasion/manipulation (cognitive firewall).
    /// Returns nil if no manipulation detected or RI is disabled.
    func scanResponse(_ text: String) -> RiReportFfi? {
        return OpenObscureLib.scanResponse(handle: handle, responseText: text)
    }

    /// Sanitize a speech transcript (convenience wrapper).
    func sanitizeTranscript(_ transcript: String) -> String {
        let result = try! sanitizeAudioTranscript(handle: handle, transcript: transcript)
        if result.piiCount > 0 {
            mergeMappings(result.mappingJson)
            return result.sanitizedText
        }
        return transcript
    }

    /// Reset mappings when starting a new conversation.
    func resetMappings() {
        accumulatedMappings = []
    }

    /// Merge new mappings into the accumulated set. New tokens overwrite existing ones.
    private func mergeMappings(_ json: String) {
        guard let data = json.data(using: .utf8),
              let newPairs = try? JSONSerialization.jsonObject(with: data) as? [[String]] else { return }
        var tokenIndex: [String: Int] = [:]
        for (i, pair) in accumulatedMappings.enumerated() {
            if pair.count >= 2 { tokenIndex[pair[0]] = i }
        }
        for pair in newPairs where pair.count >= 2 {
            if let idx = tokenIndex[pair[0]] {
                accumulatedMappings[idx] = pair
            } else {
                tokenIndex[pair[0]] = accumulatedMappings.count
                accumulatedMappings.append(pair)
            }
        }
    }

    // MARK: - Keychain key storage

    /// Change this to match your app's bundle identifier.
    private static let keychainService = "com.yourapp.openobscure"

    private static func getOrCreateKey() -> String {
        let account = "fpe-key"

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: keychainService,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true
        ]
        var result: AnyObject?
        if SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
           let data = result as? Data {
            return String(data: data, encoding: .utf8)!
        }

        var bytes = [UInt8](repeating: 0, count: 32)
        _ = SecRandomCopyBytes(kSecRandomDefault, 32, &bytes)
        let hex = bytes.map { String(format: "%02x", $0) }.joined()

        let addQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: keychainService,
            kSecAttrAccount as String: account,
            kSecValueData as String: hex.data(using: .utf8)!
        ]
        SecItemAdd(addQuery as CFDictionary, nil)
        return hex
    }
}
