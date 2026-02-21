# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| Latest release | Yes |
| Previous minor | Security fixes only |
| Older | No |

## Reporting a Vulnerability

If you discover a security vulnerability in OpenObscure, **please report it responsibly** using GitHub's private vulnerability reporting.

### How to Report

1. Go to the [OpenObscure GitHub repository](https://github.com/openobscure/openobscure) → **Security** tab → **Report a vulnerability**
2. Include:
   - Description of the vulnerability
   - Steps to reproduce
   - Affected component(s) (L0 proxy, L1 plugin, L2 crypto)
   - Potential impact assessment
   - Suggested fix (if any)

Do not open a public issue for security vulnerabilities — use GitHub's private reporting to ensure the issue can be assessed and fixed before public disclosure.

### Response Process

OpenObscure is a community-maintained open-source project. Response times depend on contributor availability and the nature of the vulnerability. We aim to:

1. **Acknowledge** reports promptly
2. **Assess** severity and impact
3. **Develop and release** a fix, coordinating disclosure with the reporter
4. **Disclose publicly** after the fix is available

Critical vulnerabilities (key extraction, proxy bypass, plaintext PII leaks) will be prioritized. Community contributions to fixes are welcome and encouraged.

### What Qualifies

- Bypass of PII detection (input that should be caught but isn't)
- FPE key extraction without OS keychain access
- Transcript decryption without the passphrase
- L0 proxy bypass (traffic reaching LLM without passing through proxy)
- File Access Guard bypass (reading denied paths)
- Memory leaks of plaintext PII
- Denial of service against the proxy
- Dependency vulnerabilities with exploitable paths in OpenObscure
- ONNX model substitution attacks (replacing SCRFD/BlazeFace/PaddleOCR models with malicious ones)
- Image processing bypass (crafting images where faces/text aren't detected)
- Resource exhaustion via image processing (OOM, CPU spin on adversarial inputs)
- Base64 bomb attacks (crafted base64 strings that decode to extremely large images)

### What Does NOT Qualify

- Attacks requiring root/admin access to the host OS (see Threat Model — this is explicitly out of scope)
- Attacks requiring a compromised host agent process
- PII types not yet covered by the current phase (e.g., names in Phase 1 — known limitation, tracked in roadmap)
- Regex evasion using obfuscated/unusual PII formats (report these as feature requests, not vulnerabilities)
- Vulnerabilities in LLM providers, the host agent itself, or upstream dependencies without a demonstrated exploit path through OpenObscure

## Security Design Principles

### Kerckhoffs's Principle

OpenObscure's security depends on the secrecy of **keys**, never on the secrecy of **code**. All algorithms used (FF1, AES-256-GCM, Argon2id) are public NIST/OWASP standards. Publishing source code does not weaken the system.

### Defense in Depth

Three independent layers (L0-L2) ensure that a failure in one layer does not expose all PII. L0 (proxy) and L1 (plugin) operate at different interception points — compromising one does not bypass the other.

### Minimal Trust

- **No telemetry** — zero outbound connections beyond forwarded LLM requests
- **No default credentials** — FPE key must be explicitly generated with `--init-key`
- **No cloud dependencies** — everything runs locally on the user's device
- **Passthrough-first auth** — OpenObscure never provisions or stores LLM API keys by default

### Secrets Management

OpenObscure supports multiple secret storage backends to accommodate both desktop and headless (Docker, CI, VPS) environments. The priority chain for each secret is:

**FPE master key** (32 bytes, AES-256):
1. `OPENOBSCURE_MASTER_KEY` environment variable (64 hex chars) — for headless/Docker/CI
2. OS keychain (macOS Keychain, Windows Credential Store, Linux keyutils/Secret Service) — for desktop
3. Encrypted file (`~/.openobscure/master.key.enc`) — fallback for environments without keychain or env vars
4. Fail (503 — proxy refuses to start without a key)

**L0/L1 auth token** (32 bytes, hex):
1. `OPENOBSCURE_AUTH_TOKEN` environment variable — for headless environments
2. `~/.openobscure/.auth-token` file (permissions 0600 on Unix) — auto-generated on first run
3. Auto-generate with `OsRng` and write to file

**Security trade-offs:** Environment variables are intentionally supported because containerized and headless environments lack OS keychain access. Env vars are visible to process inspectors (e.g., `/proc/*/environ` on Linux). This is the standard pattern used by Docker secrets, Kubernetes secrets, and HashiCorp Vault. For maximum security on desktop environments, prefer OS keychain over env vars.

Secrets are **never** written to:
- Source code or config files
- Log output (all logging passes through PII scrubbing)

The FPE key and transcript passphrase are independent — compromising one does not expose the other.

## Secure Development Practices

### Dependencies

- All dependencies are audited for license and security before inclusion (see `LICENSE_AUDIT.md` in each component)
- Rust dependencies use `cargo audit` for known vulnerability scanning
- L1 plugin uses `better-sqlite3` as its only runtime dependency (native module for consent storage). All other Node.js dependencies are dev-only (build/test).
- ONNX models (Phase 2+) are sourced from HuggingFace with checksum verification

### Code

- L0 and L2 are written in Rust — memory-safe by default (no buffer overflows, use-after-free, or data races)
- L1 is TypeScript with strict mode — type-checked at compile time
- PII values are never logged — only match counts and types are recorded
- Integration tests verify FPE roundtrip correctness and PII redaction completeness

### Build

- Release binaries built with `opt-level = "s"`, LTO, symbol stripping, `panic = "abort"`
- No debug symbols in release builds
- Binary verifiability is a goal — see [open_source_strategy.md](review-notes/open_source_strategy.md) for the build verification approach

### Image Processing Attack Surface (Phase 3)

The image pipeline introduces additional attack surfaces that are actively mitigated:

| Attack | Mitigation |
|--------|-----------|
| **Malicious images** (crafted PNG/JPEG headers to exploit decoders) | `image` crate's decoders are Rust (memory-safe). Images resized to 960px max before processing. |
| **Base64 bombs** (small base64 that decodes to massive images) | Size check after base64 decode, before `image::load_from_memory()`. Max dimension enforced. |
| **Adversarial inputs to face/OCR models** | Fail-open — undetected faces/text pass through. Models are supplementary to text-based PII scanning. |
| **ONNX model substitution** (replacing `.onnx` files with malicious models) | Models verified by SHA256 checksum against trusted manifest. Model paths are admin-configured, not user-supplied. |
| **Resource exhaustion** (large images causing OOM or CPU spin) | 960px resize cap, sequential model loading (never both face + OCR in RAM), 224MB hard ceiling. |
| **EXIF-based attacks** (crafted EXIF metadata) | EXIF read via `kamadak-exif` for screenshot detection only; EXIF is stripped by re-encoding (pixels only). |

## Threat Model

For the full threat model including what OpenObscure protects against, what it does not, and the complete secrets inventory, see the [Threat Model section in ARCHITECTURE.md](ARCHITECTURE.md#threat-model).

## Audit Status

OpenObscure has **not** undergone a formal third-party security audit. The cryptographic implementation uses well-tested library implementations (RustCrypto ecosystem) rather than custom primitives. Community review is welcomed and encouraged.

If you or your organization are interested in sponsoring a formal audit, please reach out.
