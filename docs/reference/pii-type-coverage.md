# PII Type Coverage

Every PII type OpenObscure detects, how it is detected, and how it is protected.

Source: [pii_types.rs](../../openobscure-proxy/src/pii_types.rs), [scanner.rs](../../openobscure-proxy/src/scanner.rs), [multilingual/](../../openobscure-proxy/src/multilingual/)

---

## Core PII Types

Detected in all languages. Defined in `PiiType` enum.

| PII Type | Category String | Detection Method | Protection | Radix | Validation | Notes |
|----------|----------------|------------------|------------|-------|------------|-------|
| Credit Card | `credit_card` | Regex | FPE | 10 | Luhn checksum | Rejects invalid card numbers |
| SSN | `ssn` | Regex | FPE | 10 | Range validation | Rejects 000, 666, 900+ area numbers |
| Phone Number | `phone` | Regex | FPE | 10 | Structural | Requires separator (`-`, `(`, `)`, space) or `+` prefix |
| Email | `email` | Regex | FPE (local part) | 36 | RFC-like pattern | Domain preserved, local part encrypted |
| API Key | `api_key` | Regex | FPE (suffix) | 62 | Prefix match | Known prefixes preserved: `sk-ant-`, `sk-`, `AKIA`, `ghp_`, `gho_`, `xoxb-`, `xoxp-` |
| IPv4 Address | `ipv4_address` | Regex | FPE | 10 | Structural | Rejects loopback/broadcast |
| IPv6 Address | `ipv6_address` | Regex | FPE | 16 | Full + compressed | Handles `::` shorthand; lowercased before FPE |
| GPS Coordinate | `gps_coordinate` | Regex | FPE | 10 | Precision check | Requires 4+ decimal digits |
| MAC Address | `mac_address` | Regex | FPE | 16 | Three formats | Colon, dash, dot separators; lowercased before FPE |
| IBAN | `iban` | Regex | FPE | 36 | Country code | 2-letter prefix preserved; lowercased before FPE |
| Health Keyword | `health_keyword` | Keywords | Hash token | — | Dictionary lookup | ~350 health terms, 9 languages. Prefix: `HLT` |
| Child Keyword | `child_keyword` | Keywords | Hash token | — | Dictionary lookup | ~350 child-related terms, 9 languages. Prefix: `CHL` |
| Person | `person` | NER / CRF / Gazetteer | Hash token | — | Model confidence | Named entity. Prefix: `PER` |
| Location | `location` | NER / CRF | Hash token | — | Model confidence | Named entity. Prefix: `LOC` |
| Organization | `organization` | NER / CRF | Hash token | — | Model confidence | Named entity. Prefix: `ORG` |

**FPE** = FF1 Format-Preserving Encryption (reversible, structure-preserving).
**Hash token** = `PREFIX_<hex>` deterministic token (reversible via mapping store).

---

## Multilingual PII Types

Activated by `whatlang` language detection (confidence threshold 0.15, minimum 20 characters). All use regex detection with post-validation. All are FPE-eligible.

### Spanish (`es`)

| PII Type | Detection | Validation | Pattern | Notes |
|----------|-----------|------------|---------|-------|
| DNI | Regex | Mod-23 check letter | `[0-9]{8}-?[A-Za-z]` | 8 digits + check letter |
| NIE | Regex | Mod-23 check letter | `[XYZ]-?[0-9]{7}-?[A-Za-z]` | Foreign resident ID |
| Phone (+34) | Regex | — | `+34 XXX XXX XXX` | International format |
| IBAN (ES) | Regex | Mod-97 check | `ES## #### #### #### #### ####` | 24 characters |

### French (`fr`)

| PII Type | Detection | Validation | Pattern | Notes |
|----------|-----------|------------|---------|-------|
| NIR | Regex | Modulus 97 | `[12] YY MM DDDDD OOO CC` | 15-digit social security number |
| Phone (+33) | Regex | — | `+33 X XX XX XX XX` | International format |
| Phone (0X) | Regex | — | `0X XX XX XX XX` | Local format |
| IBAN (FR) | Regex | Mod-97 check | `FR## #### #### #### #### #### ###` | 27 characters |

### German (`de`)

| PII Type | Detection | Validation | Pattern | Notes |
|----------|-----------|------------|---------|-------|
| Tax ID (Steuer-ID) | Regex | Digit frequency rules | 11 digits | First digit non-zero; at least one digit repeated 2+ times |
| Phone (+49) | Regex | — | `+49 XX XXXXXXXX` | Variable-length area code |
| IBAN (DE) | Regex | Mod-97 check | `DE## #### #### #### #### ##` | 22 characters |

### Portuguese (`pt`)

| PII Type | Detection | Validation | Pattern | Notes |
|----------|-----------|------------|---------|-------|
| CPF | Regex | Mod-11 weighted (2 check digits) | `XXX.XXX.XXX-XX` | Brazilian individual taxpayer; rejects all-same digits |
| CNPJ | Regex | Mod-11 weighted (2 check digits) | `XX.XXX.XXX/XXXX-XX` | Brazilian company ID, 14 digits |
| Phone (+55) | Regex | — | `+55 XX XXXXX-XXXX` | Brazil mobile |
| Phone (+351) | Regex | — | `+351 XXX XXX XXX` | Portugal |

### Japanese (`ja`)

| PII Type | Detection | Validation | Pattern | Notes |
|----------|-----------|------------|---------|-------|
| My Number (個人番号) | Regex | Weighted mod-11 | `XXXX-XXXX-XXXX` | 12 digits, last is check digit |
| Phone (+81) | Regex | — | `+81 XX XXXX XXXX` | International format |
| Phone (0X0) | Regex | — | `0[789]0 XXXX XXXX` | Mobile: 070, 080, 090 prefix |

### Chinese (`zh`)

| PII Type | Detection | Validation | Pattern | Notes |
|----------|-----------|------------|---------|-------|
| Citizen ID (居民身份证) | Regex | Weighted mod-11 + X | 18 chars | Region (6) + YYYYMMDD (8) + seq (3) + check (1, may be X) |
| Phone (+86) | Regex | — | `+86 1XX XXXX XXXX` | International, 1[3-9]X prefix |
| Phone (1XX) | Regex | — | `1XX XXXX XXXX` | Domestic mobile |

### Korean (`ko`)

| PII Type | Detection | Validation | Pattern | Notes |
|----------|-----------|------------|---------|-------|
| RRN (주민등록번호) | Regex | Weighted mod-11 | `YYMMDD-GXXXXXX` | 13 digits: DOB + gender/century digit + 6 |
| Phone (+82) | Regex | — | `+82 10 XXXX XXXX` | International format |
| Phone (010) | Regex | — | `010 XXXX XXXX` | Domestic mobile |

> **PIPA note (Korea's Personal Information Protection Act):** The RRN encodes full date of birth and a gender/century digit. Even when protected by FPE or hash-token, the token is stable across sessions (same person → same token within a key lifetime). For PIPA-regulated workloads where cross-session linkage is a concern, consider periodic key rotation to break token correlation.

### Arabic (`ar`)

| PII Type | Detection | Validation | Pattern | Notes |
|----------|-----------|------------|---------|-------|
| Saudi National ID | Regex | Prefix check | 10 digits | Starts with 1 (citizen) or 2 (resident) |
| UAE Emirates ID | Regex | Prefix check | `784-YYYY-NNNNNNN-C` | 15 digits, starts with 784 |
| Phone (+966) | Regex | — | `+966 5X XXX XXXX` | Saudi Arabia |
| Phone (+971) | Regex | — | `+971 5X XXX XXXX` | UAE |

---

## Language Detection & Dispatch

### Dispatch Parameters

These values are **hardcoded** in [`lang_detect.rs`](../../openobscure-proxy/src/lang_detect.rs) and are not exposed in any TOML config key or `MobileConfig` field.

| Parameter | Value | Source location | Rationale |
|-----------|-------|-----------------|-----------|
| Minimum text length | **20 bytes** | `lang_detect.rs:76` | `whatlang` requires ~20 chars for reliable trigram-based identification; shorter input returns `None` and skips the multilingual pass entirely |
| Minimum detection confidence | **0.15** | `lang_detect.rs:85` | Deliberately loose: PII-bearing texts contain digits and punctuation that suppress `whatlang` confidence. Language-specific validation functions (check digits, IBAN mod-97, Luhn) prevent false positives that a stricter threshold would otherwise guard against |

To change either value, edit the constants directly in `lang_detect.rs` and recompile. There is no runtime override.

#### Fallback behavior when thresholds are not met

When `detect_language()` returns `None` (either condition triggers), the call chain is:

```
detect_language(text) → None
  └─ languages_to_scan(None)     # multilingual/mod.rs:42 → return vec![]
       └─ for lang in []          # hybrid_scanner.rs:227 — loop body never executes
```

The multilingual pass is **skipped entirely** — zero language-specific patterns (national IDs, country phone numbers, IBANs) are applied to that text.

**What still runs** (these steps are unconditional and execute before language detection in the pipeline):

| Engine | Affected by threshold miss? |
|--------|-----------------------------|
| Regex scanner (10 core types) | No — always runs on every text |
| Keyword dictionary (health/child) | No — always runs if `keywords_enabled = true` |
| Name gazetteer | No — always runs if `gazetteer_enabled = true` |
| NER / CRF semantic backend | No — runs on the full text regardless of language |
| Multilingual patterns | **Yes — entirely skipped** |

**Effect on English text:** English never reaches language-specific patterns regardless of confidence, because `languages_to_scan` also returns `vec![]` when `detected == Language::English` (`multilingual/mod.rs:45`). Short or ambiguous English texts that fail the length or confidence gate behave identically to texts that pass and are detected as English — no multilingual patterns run in either case.

### Language Table

| Language | `whatlang` Code | Confusable Pairs | National IDs | Phone Formats | IBAN |
|----------|----------------|------------------|--------------|---------------|------|
| Spanish | `Spa` | Portuguese | DNI, NIE | +34 | ES |
| French | `Fra` | Spanish, Portuguese | NIR | +33, 0X | FR |
| German | `Deu` | — | Tax ID | +49 | DE |
| Portuguese | `Por` | Spanish | CPF, CNPJ | +55, +351 | — |
| Japanese | `Jpn` | — | My Number | +81, 0X0 | — |
| Chinese | `Cmn` | — | Citizen ID | +86, 1XX | — |
| Korean | `Kor` | — | RRN | +82, 010 | — |
| Arabic | `Ara` | — | Saudi ID, Emirates ID | +966, +971 | — |

Confusable pairs: when `whatlang` detects Spanish, Portuguese patterns are also scanned (and vice versa). French detection adds both Spanish and Portuguese. Companion scanning is suppressed for any language not in `scanner.enabled_languages` when that config key is set (see [Multilingual Scanner Configuration](../configure/detection-engine-configuration.md#multilingual-scanner-configuration)).

---

## Protection Methods

| Method | Mechanism | Reversible | Used By |
|--------|-----------|------------|---------|
| FF1 FPE | AES-256 format-preserving encryption (NIST SP 800-38G) | Yes (with key) | 10 core types + all multilingual types |
| Hash Token | `PREFIX_<hex>` deterministic token via mapping store | Yes (with mapping) | Person, Location, Organization, Health/Child keywords |
| Fail-open redaction | `[REDACTED:type]` label | No | FPE errors in `fail_mode = "closed"` |

FPE configuration: [FPE Configuration](../configure/fpe-configuration.md). Per-type enable/disable via `[fpe.type_overrides]` in [Config Reference](../configure/config-reference.md#fpe).

---

## Detection Engine Summary

| Engine | Detects | Confidence | Activation |
|--------|---------|------------|------------|
| Regex (`scanner.rs`) | 10 structured core types | 1.0 (deterministic) | Always on |
| Keywords (`keyword_dict.rs`) | Health/child terms (9 languages) | 1.0 | Always on (configurable) |
| NER (`ner_scanner.rs`) | Person, Location, Organization, Health, Child | Model score (0.0–1.0) | Tier-dependent |
| CRF (`crf_scanner.rs`) | Same as NER | Model score | Fallback when NER unavailable |
| Gazetteer (`name_gazetteer.rs`) | Person names (supporting evidence) | — | Default on |
| Multilingual (`multilingual/`) | National IDs, country phones, IBANs | 1.0 (deterministic) | Auto via `whatlang` detection |

Engine configuration: [Detection Engine Configuration](../configure/detection-engine-configuration.md).
