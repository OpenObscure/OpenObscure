# Export Control and Cryptography Notice

> **Status: DRAFT** — The BIS notification and self-classification process have not yet been completed. This notice documents the cryptographic functionality for planning purposes. Binary distribution must not begin until the EAR process is finalized.

**OpenObscure** includes cryptographic software subject to the United States Export Administration Regulations (EAR).

## Cryptographic Functionality

This software utilizes strong encryption to protect data in transit (L0 Proxy) and at rest (L2 Encryption Layer). The following algorithms are implemented:

* **Symmetric Encryption (Data at Rest):** AES-256-GCM (Galois/Counter Mode).
* **Format-Preserving Encryption (Data in Transit):** FF1 (NIST SP 800-38G) for PII obfuscation.
    * *Note: FF3 is explicitly excluded/withdrawn per NIST SP 800-38G Rev 2*.
* **Key Derivation:** Argon2id (OWASP recommended parameters: 19MB memory, 2 iterations).
* **Transport Security:** TLS 1.2/1.3 via standard Rust libraries (`hyper`, `rustls`) for communication with upstream LLM providers.

**Additional crypto-relevant components:** ONNX Runtime (used for NER and image processing models) may include its own cryptographic functionality for TLS when fetching pre-compiled binaries at build time. ONNX Runtime is MIT-licensed.

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
