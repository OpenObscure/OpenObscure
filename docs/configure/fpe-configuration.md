# FPE Configuration

OpenObscure uses **FF1 Format-Preserving Encryption** (NIST SP 800-38G) to encrypt PII before it reaches an LLM. Unlike traditional encryption that produces random-looking ciphertext, FPE preserves the format of the original value — a phone number encrypts to another phone number, an email to another email. This lets the LLM reason about data structure (e.g., "this looks like a US phone number") without ever seeing the real value.

FF1 operates on a configurable alphabet (digits, hex, alphanumeric) with AES-256 as the underlying block cipher. Each PII match is encrypted with a per-record tweak derived from `request_uuid || SHA-256(json_path)[0..16]`, which prevents frequency analysis — the same SSN in two different requests or JSON paths produces different ciphertext. For the full cryptographic design, see [System Overview](../architecture/system-overview.md).

---

**Contents**

- [Key Generation](#key-generation)
- [Key Rotation](#key-rotation)
- [TOML Configuration](#toml-configuration)
- [Per-PII-Type Behavior](#per-pii-type-behavior)
- [Fail-Open vs Fail-Closed](#fail-open-vs-fail-closed)
- [Verifying FPE Status](#verifying-fpe-status)

## Key Generation

### First-time setup (desktop / server)

```bash
cd openobscure-core
cargo build --release
./target/release/openobscure --init-key
```

### Headless / Docker / CI

```bash
# Generate and export a 32-byte (64 hex character) key
export OPENOBSCURE_MASTER_KEY=$(openssl rand -hex 32)
```

### Embedded (iOS / Android)

Generate the key in your app and store it securely:

```swift
// iOS: generate once with SecRandomCopyBytes or openssl rand -hex 32,
// then persist in the iOS Keychain — never hard-code in source.
let fpeKey = KeychainHelper.load(key: "openobscure-fpe-key")!
let handle = try createOpenobscure(configJson: config, fpeKeyHex: fpeKey)
```

```kotlin
// Android: generate once, then persist in Android Keystore or EncryptedSharedPreferences.
// Never hard-code in source or use plain SharedPreferences in production.
val fpeKey = keystoreHelper.load("openobscure-fpe-key")
val handle = createOpenobscure(configJson = config, fpeKeyHex = fpeKey)
```

> **Key hygiene:** The FPE key is a 32-byte AES-256 secret. Hard-coding it in source exposes the key in version control and crash logs. On iOS, use the Keychain API (`kSecClassGenericPassword`). On Android, use `EncryptedSharedPreferences` backed by the Android Keystore. Generate the key once with a cryptographically secure random source (`SecRandomCopyBytes` on iOS, `SecureRandom` on Android, or `openssl rand -hex 32`).

**Key resolution order** (gateway mode):
1. `OPENOBSCURE_MASTER_KEY` env var
2. `/run/secrets/openobscure-master-key` file (Kubernetes / Docker Secrets standard path)
3. `OPENOBSCURE_KEY_FILE` env var pointing to a custom key file path
4. `~/.openobscure/master-key` file (useful when home directory is volume-mounted)
5. OS keychain (using `keychain_service` / `keychain_user` from config)

If none of the above sources yields a valid key, the proxy exits with an error listing all five lookup locations.

> **Single global key.** OpenObscure uses one AES-256 master key for all PII types and all languages. There is no per-country or per-locale key configuration. Country-aware behavior affects detection only (multilingual scanners in `multilingual/`) — not the encryption key or tweak derivation.

---

## Key Rotation

Zero-downtime key rotation with a 30-second overlap window:

### Gateway

```bash
openobscure key-rotate
```

This generates a new random 32-byte key, stores it in the vault (env var or keychain), and restarts the FPE engine. During the 30-second overlap window, the previous key remains available for decrypting in-flight responses that were encrypted with the old key.

### Embedded

```swift
try rotateKey(handle: handle, newKeyHex: newKey)
```

```kotlin
rotateKey(handle = handle, newKeyHex = newKey)
```

**Overlap behavior:** After rotation, the `KeyManager` retains the previous `VersionedEngine` for 30 seconds. Decrypt calls automatically try the current key first, then fall back to the previous key if within the overlap window. After 30 seconds, the old key is discarded and ciphertexts encrypted with it can no longer be decrypted.

> **Security implication of the overlap window:** During the 30-second window, the proxy can decrypt ciphertexts encrypted with the old key. If the old key was compromised, rotating immediately does not eliminate the compromise for 30 seconds. The overlap window exists to allow in-flight requests (those encrypted with the old key that are still awaiting LLM responses) to complete successfully — without it, responses to in-flight requests would fail decryption.

> **The 30-second overlap window is hardcoded** (`key_manager.rs: DEFAULT_OVERLAP_SECS = 30`) and is not configurable via TOML. High-latency deployments where request round-trips may exceed 30 seconds risk losing decryption capability for responses encrypted with the old key before rotation completed.

> **Warning — proxy restart drops all in-flight mappings.**
>
> The per-request FPE mapping store is held entirely in memory. If the proxy process exits (crash, OOM, `kill`, or intentional restart) while LLM requests are being processed, all ciphertext-to-plaintext mappings for those requests are permanently lost.
>
> **What clients observe:** The TCP connection resets. The client never receives undecrypted ciphertext — the proxy decrypts before forwarding in all code paths. The lost requests appear as connection errors or incomplete responses.
>
> **The FPE key is not affected.** The key is loaded from the vault (OS keychain or `OPENOBSCURE_MASTER_KEY`) on each startup. Clients should retry failed requests; the retry will encrypt PII with new per-record tweaks and complete normally.
>
> **Detecting lost requests:** After restart, the proxy reads the request journal (`~/.openobscure/request_journal.buf`) and emits a WARN log for each entry that has a start record but no completion record. There is no health endpoint counter for this — detection is log-based only. See [Crash Recovery](../operate/crash-recovery.md).

---

## TOML Configuration

All FPE options live in `config/openobscure.toml`:

```toml
[fpe]
enabled = true                        # Master switch for FPE encryption
keychain_service = "openobscure"      # OS keychain service name
keychain_user = "fpe-master-key"      # OS keychain account name

# Per-type overrides: disable FPE for specific PII types
# [fpe.type_overrides]
# credit_card = true
# ssn = true
# phone = true
# email = true
# api_key = true
# ipv4_address = true
# ipv6_address = true
# gps_coordinate = true
# mac_address = true
# iban = true
```

The `fail_mode` setting governs behavior at two code paths, and is configured in `[proxy]`:

```toml
[proxy]
fail_mode = "open"   # "open" (default) or "closed"
```

### Option Reference

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `fpe.enabled` | bool | `true` | Master switch. When `false`, all PII detection still runs but no encryption is applied. |
| `fpe.keychain_service` | string | `"openobscure"` | OS keychain service name for key storage. |
| `fpe.keychain_user` | string | `"fpe-master-key"` | OS keychain account/user for key storage. |
| `fpe.type_overrides.<type>` | bool | (all `true`) | Per-type enable/disable. Set to `false` to skip FPE for a specific type. |
| `proxy.fail_mode` | `"open"` \| `"closed"` | `"open"` | Controls behavior for per-span FPE encryption failures and for whole-body processing errors. See [Fail-Open vs Fail-Closed](#fail-open-vs-fail-closed). |

**Valid `type_overrides` keys:** `credit_card`, `ssn`, `phone`, `email`, `api_key`, `ipv4_address`, `ipv6_address`, `gps_coordinate`, `mac_address`, `iban`.

---

## Per-PII-Type Behavior

OpenObscure recognizes 15 PII types. Ten have structured formats suitable for FF1 encryption; five use hash-token redaction instead.

### FPE-encrypted types

| PII Type | Config key | Radix | Alphabet | Min length | Notes |
|----------|-----------|-------|----------|------------|-------|
| Credit card | `credit_card` | 10 | `0-9` | 15 | Luhn-validated before encryption |
| SSN | `ssn` | 10 | `0-9` | 9 | Range-validated (no 000/666/900+) |
| Phone number | `phone` | 10 | `0-9` | 10 | Separators stripped, format preserved |
| Email | `email` | 36 | `0-9a-z` | 4 | Local part only; `@domain` preserved. Local part is **lowercased before encryption** — `Admin@example.com` encrypts as `<ciphertext>@example.com`. |
| API key | `api_key` | 62 | `0-9A-Za-z` | 6 | Case-sensitive alphanumeric |
| IPv4 address | `ipv4_address` | 10 | `0-9` | 4 | Dot separators preserved |
| IPv6 address | `ipv6_address` | 16 | `0-9a-f` | 2 | Colon separators preserved. **Lowercased before encryption.** |
| GPS coordinate | `gps_coordinate` | 10 | `0-9` | 6 | Decimal point position preserved |
| MAC address | `mac_address` | 16 | `0-9a-f` | 6 | Colon/dash separators preserved. **Lowercased before encryption.** |
| IBAN | `iban` | 36 | `0-9a-z` | 6 | 2-letter country code preserved, rest encrypted. **Lowercased before encryption.** |

**Domain size safety:** FF1 requires `radix^length >= 1,000,000` for security. Values below this threshold trigger a `DomainTooSmall` error (see [Fail-Open vs Fail-Closed](#fail-open-vs-fail-closed)).

**Format preservation:** Separators (dashes, dots, colons, spaces) are stripped before encryption and re-inserted at their original positions afterward. The ciphertext has the same visual format as the plaintext — `123-45-6789` encrypts to `847-29-3156`, not `8472931560`.

### Hash-token redacted types

These types lack a fixed character alphabet suitable for FF1, so they are replaced with deterministic hash tokens or labels:

| PII Type | Redaction output | Source |
|----------|-----------------|--------|
| Person (name) | `[PERSON]` | NER model |
| Location | `[LOCATION]` | NER model |
| Organization | `[ORG]` | NER model |
| Health keyword | `[HEALTH]` | Keyword dictionary |
| Child keyword | `[CHILD]` | Keyword dictionary |

Hash-token types are always redacted regardless of `fpe.type_overrides` — they do not participate in FPE.

---

## Fail-Open vs Fail-Closed

`proxy.fail_mode` governs two code paths:

1. **Per-span FPE error** — FF1 encryption fails for a single PII match (`body.rs`).
2. **Whole-body processing error** — `process_request_body()` returns a fatal error from any pipeline phase: image, voice, or text scanning (`proxy.rs`).

### `"open"` (default)

Prioritizes AI functionality — never blocks a request due to encryption or pipeline failure.

| Scope | Error | Behavior |
|-------|-------|----------|
| Per-span FPE | `DomainTooSmall` (value too short for FF1) | Falls back to hash-token redaction (e.g., `EMAIL_a7f2`). PII is still protected. Logged at INFO. |
| Per-span FPE | Other FPE errors (`InvalidCharacter`, `NumeralString`, etc.) | Skips encryption, forwards original plaintext. Logged at WARN. Increments `fpe_unprotected_total` in health stats. Sets `X-OpenObscure-PII-Unprotected` response header. |
| Whole-body | Any fatal error from image, voice, or text pipeline | WARN logged; original body forwarded unmodified. All PII in the body is unprotected for that request. |

### `"closed"`

Prioritizes strict privacy — no plaintext PII or unscanned body ever leaves the device.

| Scope | Error | Behavior |
|-------|-------|----------|
| Per-span FPE | Any FPE error | Destructive redaction: replaces plaintext with `[REDACTED:<type>]` (e.g., `[REDACTED:email]`). The LLM cannot reason about the value's structure, but no real PII is forwarded. |
| Whole-body | Any fatal error from image, voice, or text pipeline | ERROR logged; **502 Bad Gateway** returned. Request is not forwarded to upstream. |

### FPE Error Types

| Error | Cause |
|-------|-------|
| `DomainTooSmall` | Value has fewer characters than `min_length` for its radix (e.g., 3-char email local part) |
| `InvalidCharacter` | Character not in the type's alphabet (e.g., non-hex digit in MAC address) |
| `InvalidNumeral` | Decrypted numeral maps to invalid character |
| `NumeralString` | FF1 internal encryption/decryption error |
| `UnsupportedRadix` | Radix not in {10, 16, 36, 62} |
| `UnsupportedType` | PII type has no alphabet mapper |

---

## Verifying FPE Status

### Gateway

```bash
# Add -H "X-OpenObscure-Token: $OPENOBSCURE_AUTH_TOKEN" if auth token is configured
curl -s http://127.0.0.1:18790/_openobscure/health | python3 -m json.tool
```

Look for:
- `pii_matches_total` — total PII items detected
- `fpe_encrypted_count` — items successfully encrypted
- `fpe_unprotected_total` — items where FPE failed with an error other than `DomainTooSmall` (in fail-open: plaintext forwarded; in fail-closed: `[REDACTED]` substituted). **`DomainTooSmall` fallbacks are not counted here** — those produce hash tokens and are logged at INFO level. A `fpe_unprotected_total = 0` therefore does not guarantee all PII was FPE-encrypted.
- `key_version` — current FPE key version (increments on rotation)
- `overlap_active` — `true` during the 30-second post-rotation window

### Embedded

```swift
let stats = getStats(handle: handle)
print(stats.piiMatchesTotal)
```
