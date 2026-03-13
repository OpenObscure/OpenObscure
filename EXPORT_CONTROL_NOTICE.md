# Export Control and Cryptography Notice

**OpenObscure** includes cryptographic functionality subject to the United States Export Administration Regulations (EAR).

## Cryptographic Functionality

* **Format-Preserving Encryption:** FF1 (NIST SP 800-38G) with AES-256, used for PII obfuscation
* **Transport Security (Gateway only):** TLS 1.2/1.3 via standard Rust libraries (`rustls`)

All cryptographic implementations use standard, published algorithms with no proprietary or non-standard cryptography.

## TSU Notification

Pursuant to 15 CFR § 742.15(b) and § 740.13(e), BIS and the ENC Encryption Request Coordinator have been notified that this encryption source code is publicly available. Notification submitted 2026-03-13.

If the cryptographic functionality or repository location changes, an updated notification will be submitted.

## Disclaimer

*This notice is provided for informational purposes only and does not constitute legal advice. Users are responsible for determining whether their use, distribution, or re-export of this software complies with applicable export control laws in their jurisdiction.*
