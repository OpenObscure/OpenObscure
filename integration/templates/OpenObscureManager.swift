// OpenObscureManager.swift — Template for iOS/macOS integration
// Copy this file into your Xcode project and adjust the service identifier.

import Foundation
import Security
import OpenObscureLib

final class OpenObscureManager {
    static let shared = OpenObscureManager()
    let handle: OpenObscureHandle
    var lastMappingJson: String = "{}"

    private init() {
        let key = OpenObscureManager.getOrCreateKey()
        handle = try! createOpenobscure(
            configJson: #"{"scanner_mode": "regex"}"#,
            fpeKeyHex: key
        )
    }

    /// Sanitize text — returns (sanitized text, PII count).
    func sanitize(_ text: String) -> (sanitizedText: String, piiCount: UInt32) {
        let result = try! sanitizeText(handle: handle, text: text)
        if result.piiCount > 0 {
            lastMappingJson = result.mappingJson
        }
        return (result.sanitizedText, result.piiCount)
    }

    /// Restore PII in LLM response text using the last saved mapping.
    func restore(_ text: String) -> String {
        return restoreText(handle: handle, text: text, mappingJson: lastMappingJson)
    }

    /// Sanitize a speech transcript (convenience wrapper).
    func sanitizeTranscript(_ transcript: String) -> String {
        let result = try! sanitizeAudioTranscript(handle: handle, transcript: transcript)
        if result.piiCount > 0 {
            lastMappingJson = result.mappingJson
            return result.sanitizedText
        }
        return transcript
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
