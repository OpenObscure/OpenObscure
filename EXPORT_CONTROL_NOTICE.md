# Export Control and Cryptography Notice

> **Status: DRAFT** — The BIS notification and self-classification process have not yet been completed. This notice documents the cryptographic functionality for planning purposes. Binary distribution must not begin until the EAR process is finalized. Last reviewed: 2026-03-10.

**OpenObscure** includes cryptographic software subject to the United States Export Administration Regulations (EAR).

## Cryptographic Functionality

This software utilizes strong encryption to protect PII in transit. The following algorithms are implemented in the core (non-enterprise) codebase:

* **Format-Preserving Encryption (PII in transit):** FF1 (NIST SP 800-38G) for PII obfuscation. Used in both Gateway (L0 proxy) and Embedded (mobile library) deployment models.
    * *Note: FF3 is explicitly excluded/withdrawn per NIST SP 800-38G Rev 2*.
* **Transport Security (Gateway only):** TLS 1.2/1.3 via standard Rust libraries (`hyper`, `rustls`) for communication with upstream LLM providers. Not used in the Embedded model (the host app handles networking).

**Enterprise-only algorithms** (not in the main open-source distribution):
* AES-256-GCM (data at rest encryption)
* Argon2id key derivation

**Additional crypto-relevant components:** ONNX Runtime (used for NER and image processing models) may include its own cryptographic functionality for TLS when fetching pre-compiled binaries at build time. ONNX Runtime is MIT-licensed. sherpa-onnx (used for KWS keyword spotting in the voice pipeline) performs audio inference only and does not introduce additional cryptographic functionality.

**Mobile app store note:** iOS App Store and Google Play require declaring encryption usage. The FF1 FPE implementation in the Embedded library qualifies as encryption under both Apple's and Google's export compliance requirements.

## Export Restrictions — Source Code vs Binaries

The EAR treats open-source source code and compiled binaries differently:

### Source Code (publicly available)

Under **15 CFR § 742.15(b)**, publicly available encryption source code is eligible for exemption from EAR classification requirements. The required steps are:

1. Send a notification email to `crypt@bis.doc.gov` and `enc@nsa.gov` with the URL of the public repository
2. No further classification or registration is needed for source-only distribution

### Compiled Binaries

Compiled binaries containing cryptographic functionality require the full **License Exception ENC** self-classification process:

1. Register for SNAP-R (Simplified Network Application Process - Redesign)
2. File an Annual Self-Classification Report (due February 1st each year)
3. Obtain ECCN classification confirmation

As of 2026, this software is expected to be classified under **ECCN 5D002**.

## Encryption Registration

* **ERN (Encryption Registration Number):** PENDING — not yet filed
* **CCATS:** PENDING — not yet filed

**Important:** Binary distribution (GitHub Releases, signed installers, app store submissions) must not begin until the BIS notification and self-classification process are complete.

## Embargo Restrictions

Users residing in countries subject to U.S. embargoes or trade sanctions are prohibited from downloading, accessing, or using this software. For the current list of sanctioned countries and regions, refer to the [BIS Sanctioned Destinations](https://www.bis.doc.gov/index.php/policy-guidance/country-guidance/sanctioned-destinations) page.

## ITAR

This software is not designed for military or defense applications and is not subject to the International Traffic in Arms Regulations (ITAR).

## Disclaimer

*This notice is provided for informational purposes only and does not constitute legal advice. Users are solely responsible for determining whether their use, distribution, or re-export of this software complies with applicable export control laws and regulations of their jurisdiction.*
