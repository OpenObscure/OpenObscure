# OpenObscure PII Detection Testing Guide

> Comprehensive guide for testing PII detection across all supported data categories
> using Gateway (FPE) and Embedded (label redaction) architectures.

---

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [FPE Testing Architecture](#fpe-testing-architecture)
3. [Prerequisites](#prerequisites)
4. [Test Data Inventory](#test-data-inventory)
5. [Gateway vs Embedded Comparison](#gateway-vs-embedded-comparison)
6. [Manual Testing](#manual-testing)
7. [Automated Testing](#automated-testing)
8. [Test Scripts Reference](#test-scripts-reference)
9. [Output Format](#output-format)
10. [Pass/Fail Validation](#passfail-validation)
11. [Troubleshooting](#troubleshooting)

---

## Architecture Overview

OpenObscure supports two deployment models for PII detection. The key difference
for testing is how PII is redacted in the output files:

| Aspect | Gateway (L0 Proxy) | Embedded (L1 Plugin) |
|--------|-------------------|---------------------|
| **Runtime** | Standalone Rust binary | In-process TypeScript/Node.js |
| **Default Port** | `127.0.0.1:18790` | N/A (library call) |
| **Detection Entry** | `POST /_openobscure/ner` | `redactPii()` / `redactPiiWithNer()` |
| **Redaction Entry** | Proxy pass-through (FPE) | `redactPii()` returns labeled text |
| **Detection** | Regex + Keywords + NER + CRF + Ensemble | Regex (+ NER via L0 bridge) |
| **Redaction Mode** | **FF1 FPE** for 5 types + labels for 9 types | **`[REDACTED-*]` labels** for all types |
| **Image Pipeline** | Face/OCR/NSFW blur via proxy | `sanitize_image()` on mobile |
| **Voice Pipeline** | Whisper STT + PII scan | N/A |
| **Auth** | `X-OpenObscure-Token` header | N/A |
| **Streaming** | SSE pass-through | N/A |

### Detection Coverage by Architecture

| PII Type | Gateway Redacted Output | Embedded Redacted Output |
|----------|------------------------|--------------------------|
| Credit Card | `4732-8294-5617-3048` (FPE, same format) | `[REDACTED-CC]` |
| SSN | `234-56-7891` (FPE, same format) | `[REDACTED-SSN]` |
| Phone | `+1-555-392-7104` (FPE, same format) | `[REDACTED-PHONE]` |
| Email | `xkrp.bwq@example.com` (FPE, local part) | `[REDACTED-EMAIL]` |
| API Key | `sk-ant-api03-Xk9mP...` (FPE, prefix kept) | `[REDACTED-KEY]` |
| IPv4 | `[IPv4]` | — / `[REDACTED-IPv4]` (with NER) |
| IPv6 | `[IPv6]` | — / `[REDACTED-IPv6]` (with NER) |
| GPS | `[GPS]` | — / `[REDACTED-GPS]` (with NER) |
| MAC | `[MAC]` | — / `[REDACTED-MAC]` (with NER) |
| Health Keyword | `[health_keyword]` | — / `[REDACTED-HEALTH]` (with NER) |
| Child Keyword | `[child_keyword]` | — / `[REDACTED-CHILD]` (with NER) |
| Person (NER) | `[PERSON_0]` | — / `[REDACTED-PERSON]` (with NER) |
| Location (NER) | `[LOCATION_0]` | — / `[REDACTED-LOCATION]` (with NER) |
| Organization (NER) | `[ORG_0]` | — / `[REDACTED-ORG]` (with NER) |

> **FPE** = Format-Preserving Encryption (FF1-AES256). The encrypted value has the
> same character set and length as the original — a 16-digit card number encrypts
> to another 16-digit number. This is reversible with the key.
>
> **Labels** = Fixed replacement tags. Not reversible.

---

## FPE Testing Architecture

### Why an Echo Server?

The proxy's normal flow is:

```
Client ──request──▶ Proxy ──FPE encrypt──▶ Upstream API
                    Proxy ◀──response────── Upstream API ──FPE decrypt──▶ Client
```

The proxy **decrypts FPE values in the response** using the same mapping, so the
client always sees original PII. To capture the FPE-encrypted intermediate state
(what upstream sees), we replace the upstream with an **echo server** that saves
the encrypted request body before the proxy can decrypt it.

### Component Diagram

```
                         ┌─────────────────────────────────────────┐
                         │           Test Script (bash)            │
                         │                                         │
                         │  1. POST /_openobscure/ner              │
                         │     → NER spans (json/ metadata)        │
                         │                                         │
                         │  2. POST /anthropic/v1/messages         │
                         │     → Wrap file content as message      │
                         │     → Header: X-Capture-Id: <unique>   │
                         └────────┬──────────────────┬─────────────┘
                                  │                  │
                         NER call │    FPE pass-     │
                                  │    through       │
                                  ▼                  ▼
                         ┌────────────────────────────────────────┐
                         │         L0 Proxy (port 18790)          │
                         │                                        │
                         │  Scanner: regex + keywords + NER/CRF   │
                         │  FPE: FF1-AES256 encrypt on outbound   │
                         │  Config: test/config/test_fpe.toml     │
                         └────────┬──────────────────┬────────────┘
                                  │                  │
                         NER JSON │   Encrypted body │
                         response │   forwarded      │
                                  │                  ▼
                                  │   ┌──────────────────────────┐
                                  │   │  Echo Server (port 18791)│
                                  │   │                          │
                                  │   │  Saves request body to:  │
                                  │   │  /tmp/oo_echo_captures/  │
                                  │   │  <capture_id>.json       │
                                  │   │                          │
                                  │   │  Returns minimal 200 OK  │
                                  │   │  (no PII in response)    │
                                  │   └──────────────────────────┘
                                  │                  │
                                  ▼                  ▼
                         ┌────────────────────────────────────────┐
                         │         Test Script reads outputs      │
                         │                                        │
                         │  json/<name>_gateway.json   ◀── NER    │
                         │  redacted/<name>.<ext>      ◀── FPE    │
                         │    (extracted from capture file)       │
                         └────────────────────────────────────────┘
```

### Data Flow Per File (2 Calls)

| Step | Endpoint | Purpose | Output |
|:----:|----------|---------|--------|
| 1 | `POST /_openobscure/ner` | Get PII match spans, types, confidence | `json/<name>_gateway.json` |
| 2 | `POST /anthropic/v1/messages` | Trigger proxy FPE pipeline | Echo server captures encrypted body |
| 3 | Read `CAPTURE_DIR/<id>.json` | Extract FPE-encrypted message content | `redacted/<name>.<ext>` |

### FPE vs Label Types

The proxy applies FPE only to types where format preservation is meaningful:

| Redaction Method | PII Types | Example |
|:----------------:|-----------|---------|
| **FF1 FPE** | CreditCard, SSN, Phone, Email, ApiKey | `4111-1111-1111-1111` → `4732-8294-5617-3048` |
| **Label tag** | IPv4, IPv6, GPS, MAC, HealthKeyword, ChildKeyword, Person, Location, Organization | `192.168.1.42` → `[IPv4]` |

Both methods appear in the same redacted output file. A credit card file will
show FPE-encrypted numbers; a mixed PII file will show both encrypted values
and label tags.

### Agent JSON FPE Strategy

Agent tool result files contain PII nested inside JSON structures. The script:

1. Serializes the entire JSON file as a single string
2. Wraps it as the `content` field of an Anthropic message
3. Sends through the proxy — the **nested JSON scanner** detects and FPE-encrypts PII within the serialized string
4. The echo server captures the encrypted body
5. The script extracts the content string and deserializes it back to JSON

This preserves the original JSON structure with PII values encrypted in-place.

---

## Prerequisites

### Build the Proxy

```bash
cargo build --release --manifest-path openobscure-proxy/Cargo.toml
```

### Build the Plugin

```bash
cd openobscure-plugin && npm install && npm run build && cd ..
```

### Tools Required

| Tool | Purpose | Install |
|------|---------|---------|
| `curl` | Gateway HTTP requests | Pre-installed on macOS/Linux |
| `jq` | JSON parsing/formatting | `brew install jq` / `apt install jq` |
| `node` | Echo server + L1 plugin runtime | `brew install node` / `nvm install 20` |
| `python3` | Text extraction in agent JSON scripts | Pre-installed on macOS/Linux |
| `ffprobe` | Audio file validation (optional) | `brew install ffmpeg` |

### Start the Test Environment

Three components need to be running for Gateway FPE tests:

```bash
# Terminal 1: Echo server — captures FPE-encrypted request bodies
node test/scripts/echo_server.mjs
# Output: Echo server listening on 127.0.0.1:18791

# Terminal 2: Proxy — uses test config routing to echo server
OPENOBSCURE_MASTER_KEY=$(openssl rand -hex 32) \
  ./target/release/openobscure-proxy --config test/config/test_fpe.toml serve
# Output: Listening on 127.0.0.1:18790

# Terminal 3: Run test scripts
./test/scripts/test_gateway_all.sh
```

> **Note:** `test_gateway_all.sh` auto-starts the echo server if it detects port 18791
> is not responding. You can skip Terminal 1 when using the batch scripts.

### Test Config (`test/config/test_fpe.toml`)

This config routes the Anthropic provider to the local echo server:

```toml
[providers.anthropic]
upstream_url = "http://127.0.0.1:18791"   # Echo server, not api.anthropic.com
route_prefix = "/anthropic"
```

All other settings (scanner, FPE, logging) match the default `openobscure.toml`.

### Embedded Tests (No Echo Server)

Embedded tests call `redactPii()` directly — no proxy or echo server needed:

```bash
node test/scripts/test_embedded_all.mjs
```

---

## Test Data Inventory

All test data resides under `test/data/input/`. Output directories under `test/data/output/`
have `json/` and `redacted/` subfolders for each category.

### Text-Based PII (15 files)

| File | PII Types | Est. Matches |
|------|-----------|:------------:|
| `PII_Detection/Credit_Card_Numbers.txt` | CreditCard | ~25 |
| `PII_Detection/Social_Security_Numbers.txt` | Ssn | ~20 |
| `PII_Detection/Phone_Numbers.txt` | PhoneNumber | ~30 |
| `PII_Detection/Email_Addresses.txt` | Email | ~25 |
| `PII_Detection/API_Keys_Tokens.txt` | ApiKey | ~20 |
| `PII_Detection/IPv4_Addresses.txt` | Ipv4Address | ~25 |
| `PII_Detection/IPv6_Addresses.txt` | Ipv6Address | ~20 |
| `PII_Detection/GPS_Coordinates.txt` | GpsCoordinate | ~25 |
| `PII_Detection/MAC_Addresses.txt` | MacAddress | ~20 |
| `PII_Detection/Health_Keywords.txt` | HealthKeyword | ~50 |
| `PII_Detection/Child_Keywords.txt` | ChildKeyword | ~40 |
| `PII_Detection/Person_Names.txt` | Person (NER) | ~30 |
| `PII_Detection/Locations.txt` | Location (NER) | ~30 |
| `PII_Detection/Organizations.txt` | Organization (NER) | ~30 |
| `PII_Detection/Mixed_Structured_PII.txt` | All types | ~100+ |

### Multilingual PII (8 files)

| File | Language | National ID Type |
|------|----------|-----------------|
| `Multilingual_PII/es_Spanish_PII.txt` | Spanish | DNI (mod-23), NIE |
| `Multilingual_PII/de_German_PII.txt` | German | Tax ID (digit-frequency) |
| `Multilingual_PII/fr_French_PII.txt` | French | NIR (mod-97) |
| `Multilingual_PII/pt_Portuguese_PII.txt` | Portuguese | CPF (mod-11), CNPJ |
| `Multilingual_PII/ja_Japanese_PII.txt` | Japanese | My Number (weighted mod-11) |
| `Multilingual_PII/ko_Korean_PII.txt` | Korean | RRN (weighted mod-11) |
| `Multilingual_PII/zh_Chinese_PII.txt` | Chinese | Citizen ID (weighted mod-11 + X) |
| `Multilingual_PII/ar_Arabic_PII.txt` | Arabic | Saudi/UAE ID (prefix validation) |

### Code & Config PII (8 files)

| File | Format | Key PII Types |
|------|--------|---------------|
| `Code_Config_PII/sample_python.py` | Python | ApiKey, Email, IPv4 |
| `Code_Config_PII/sample_config.yaml` | YAML | ApiKey, IPv4, MAC, GPS |
| `Code_Config_PII/sample.env` | Dotenv | ApiKey, Email, Phone |
| `Code_Config_PII/sample_access.log` | Log | IPv4, Email, ApiKey |
| `Code_Config_PII/sample_docs.md` | Markdown | ApiKey, IPv4 (in code fences) |
| `Code_Config_PII/sample_terraform.json` | JSON/HCL | ApiKey, IPv4, MAC, Email |
| `Code_Config_PII/sample_deploy.sh` | Shell | ApiKey, IPv4, Email |
| `Code_Config_PII/sample_git_diff.txt` | Git Diff | ApiKey, IPv4 |

### Structured Data PII (5 files)

| File | Format | Records | PII per Record |
|------|--------|:-------:|:--------------:|
| `Structured_Data_PII/employee_roster.csv` | CSV | 10 | SSN, Email, Phone, IP, MAC, GPS |
| `Structured_Data_PII/customer_database.csv` | CSV | 8 | CC, Email, Phone, GPS |
| `Structured_Data_PII/network_inventory.tsv` | TSV | 10 | IPv4, IPv6, MAC, Email, GPS |
| `Structured_Data_PII/patient_records.csv` | CSV | 8 | SSN, Email, Phone, Health |
| `Structured_Data_PII/transactions.csv` | CSV | 10 | CC, IPv4, Email, Phone, GPS |

### Agent Tool Results (9 JSON files)

| File | Format | Description |
|------|--------|-------------|
| `agent_anthropic_text_pii.json` | Anthropic | Single text block, all PII types |
| `agent_anthropic_multi_text.json` | Anthropic | Multiple text content blocks |
| `agent_tool_result_nested_pii.json` | Tool Result | Nested JSON string with PII |
| `agent_deeply_nested_json.json` | Tool Result | Double-escaped nested JSON |
| `agent_code_fence_credentials.json` | Assistant | YAML/bash code fences with keys |
| `agent_openai_text_apikey.json` | OpenAI | API keys in assistant response |
| `agent_tool_result_database_query.json` | Tool Result | Patient DB query with health PII |
| `agent_tool_result_network_scan.json` | Tool Result | Network scan with IPs, MACs, keys |
| `agent_multimodal_mixed.json` | Anthropic | Multi-block with image references |

### Visual PII (45 files)

| Category | Count | Types | Validated Metrics |
|----------|:-----:|-------|-------------------|
| `Visual_PII/Faces/` | 13 | Frontal, profile, group, small-in-landscape | `faces_blurred`, `text_regions_detected` |
| `Visual_PII/Screenshots/` | 7 | Desktop/mobile at various resolutions | `text_regions_detected`, `screenshot_detected` |
| `Visual_PII/Documents/` | 8 | DL, SSN card, passport, CC, W-2, etc. | `faces_blurred`, `text_regions_detected` |
| `Visual_PII/EXIF/` | 10 | Screenshot tools, cameras, no-EXIF controls | `screenshot_detected` (EXIF-based) |
| `Visual_PII/NSFW/` | 7 | 5 safe controls + 2 placeholders | `nsfw_blocked` |

### Audio PII (13 files)

| File | Format | Content |
|------|--------|---------|
| `audio_ssn_single.wav/.mp3` | WAV/MP3 | Single SSN spoken |
| `audio_cc_visa.wav` | WAV | Credit card number spoken |
| `audio_phone_us.wav/.ogg` | WAV/OGG | US phone number spoken |
| `audio_email_single.wav` | WAV | Email address spoken |
| `audio_address_single.wav` | WAV | Physical address spoken |
| `audio_name_single.wav` | WAV | Person name spoken |
| `audio_customer_service.wav/.mp3` | WAV/MP3 | Multi-PII customer call |
| `audio_medical_intake.wav/.ogg` | WAV/OGG | Medical intake with health PII |
| `audio_job_screening.wav` | WAV | Job interview with personal details |

---

## Gateway vs Embedded Comparison

### When to Use Each

| Scenario | Recommended | Why |
|----------|:-----------:|-----|
| Cloud-hosted AI agent | Gateway | Proxy intercepts all API calls transparently |
| Mobile app with on-device LLM | Embedded | No network hop, direct library call |
| Multi-agent system | Gateway | Single proxy protects all agents |
| Low-latency edge device | Embedded | No HTTP overhead |
| Need FPE (reversible encryption) | Gateway | Embedded uses label redaction only |
| Need NER/Ensemble detection | Gateway (or Embedded+NER bridge) | Full scanner stack on L0 |
| Testing regex-only detection | Either | Both support 5 core regex types |

### API Comparison

| Operation | Gateway (curl) | Embedded (Node.js) |
|-----------|---------------|-------------------|
| **Detect PII** | `POST /_openobscure/ner` with `{"text":"..."}` | `redactPii(text)` |
| **Detect + NER** | Same endpoint (auto-enables NER) | `redactPiiWithNer(text, proxyUrl)` |
| **Detect + FPE redact** | `POST /anthropic/v1/messages` (pass-through) | N/A (label redaction only) |
| **NER response** | `[{start, end, type, confidence}]` | N/A |
| **Redact response** | FPE-encrypted body (captured from echo) | `{text, count, types}` |
| **Image processing** | Base64 in JSON → blurred | `sanitize_image(bytes)` |
| **Health check** | `GET /_openobscure/health` | `HeartbeatMonitor.check()` |

---

## Manual Testing

### Gateway: NER Detection (spans only)

```bash
# 1. Ensure proxy is running
curl -s http://127.0.0.1:18790/_openobscure/health | jq .status

# 2. Send text to the NER endpoint → returns match positions
TEXT=$(cat test/data/input/PII_Detection/Credit_Card_Numbers.txt)
curl -s -X POST http://127.0.0.1:18790/_openobscure/ner \
  -H "Content-Type: application/json" \
  -d "{\"text\": $(echo "$TEXT" | jq -Rs .)}" | jq .

# 3. Inspect results: each match has start, end, type, confidence
```

### Gateway: FPE Pass-Through (encrypted output)

```bash
# Requires echo server running + proxy configured with test_fpe.toml
# Send through the proxy to see FPE encryption in the echo capture
curl -s -X POST http://127.0.0.1:18790/anthropic/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: test-fpe" \
  -H "anthropic-version: 2023-06-01" \
  -H "X-Capture-Id: manual_test_1" \
  -d '{
    "model": "test",
    "max_tokens": 1,
    "messages": [{"role": "user", "content": "Card: 4532015112830366, SSN: 412-55-8823"}]
  }'

# Read the FPE-encrypted body from echo server capture
cat /tmp/oo_echo_captures/manual_test_1.json | jq -r '.messages[0].content'
# Output: "Card: 8271039456217483, SSN: 578-21-3346"
#         (different digits, same format — FPE encrypted)
```

### Gateway: Agent JSON via Pass-Through

```bash
# Send agent JSON as message content for nested JSON FPE scanning
FILE_CONTENT=$(jq -Rs '.' test/data/input/Agent_Tool_Results/agent_anthropic_text_pii.json)
curl -s -X POST http://127.0.0.1:18790/anthropic/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: test-fpe" \
  -H "anthropic-version: 2023-06-01" \
  -H "X-Capture-Id: manual_agent_1" \
  -d "{\"model\":\"test\",\"max_tokens\":1,\"messages\":[{\"role\":\"user\",\"content\":$FILE_CONTENT}]}"

# Extract the FPE'd JSON content
jq -r '.messages[0].content' /tmp/oo_echo_captures/manual_agent_1.json | jq .
```

### Embedded: Label Redaction

```javascript
// Save as test_file.mjs and run: node test_file.mjs
import { redactPii } from "openobscure-plugin/core";
import { readFileSync } from "fs";

const text = readFileSync("test/data/input/PII_Detection/Credit_Card_Numbers.txt", "utf-8");
const result = redactPii(text);

console.log(`PII found: ${result.count}`);
console.log(`Types: ${JSON.stringify(result.types)}`);
console.log(`\nFirst 500 chars of redacted text:`);
console.log(result.text.substring(0, 500));
// Output: "... [REDACTED-CC] ... [REDACTED-CC] ..."
```

### Embedded: With NER Bridge

```javascript
// Requires L0 proxy running for NER endpoint
import { redactPiiWithNer } from "openobscure-plugin/core";
import { readFileSync } from "fs";

const text = readFileSync("test/data/input/PII_Detection/Person_Names.txt", "utf-8");
const result = redactPiiWithNer(text, "http://127.0.0.1:18790");

console.log(`PII found: ${result.count}`);
console.log(`Types: ${JSON.stringify(result.types)}`);
```

---

## Automated Testing

### Quick Start

```bash
# Gateway FPE (auto-starts echo server; proxy must be running with test_fpe.toml)
# Purges previous gateway results before running.
./test/scripts/test_gateway_all.sh

# Embedded labels (standalone, no proxy needed)
# Purges previous embedded results before running.
node test/scripts/test_embedded_all.mjs

# Validate against expected_results.json manifest (exit code 0 = pass, 1 = fail)
./test/scripts/validate_results.sh
```

### All Script Commands

```bash
# ── Gateway (FPE) ──
./test/scripts/test_gateway_all.sh                                              # All 5 text categories
./test/scripts/test_gateway_category.sh PII_Detection                           # One category
./test/scripts/test_gateway_file.sh <file> <output_dir>                         # One file
./test/scripts/test_agent_json.sh                                               # Agent JSON files
./test/scripts/test_visual.sh                                                   # Visual PII images

# ── Embedded (Labels) ──
node test/scripts/test_embedded_all.mjs                                         # All 5 text categories
node test/scripts/test_embedded_category.mjs PII_Detection                      # One category
node test/scripts/test_embedded_file.mjs <file> <output_dir>                    # One file
USE_NER=1 node test/scripts/test_embedded_all.mjs                               # With NER bridge

# ── Validation ──
./test/scripts/validate_results.sh                                              # Threshold validation (~85% min)
./test/scripts/validate_results.sh --strict                                     # Exact snapshot comparison
./test/scripts/validate_results.sh --summary                                    # Summary only
./test/scripts/validate_results.sh --json                                       # JSON report (for CI)
./test/scripts/validate_results.sh --gateway-only                               # Skip embedded checks
./test/scripts/validate_results.sh --strict --json                              # Strict + JSON (CI regression)
./test/scripts/generate_snapshot.sh                                             # Regenerate snapshot.json
```

### Output Structure

### Output Purge Behavior

Batch scripts **purge previous results** before running to ensure the validator
only sees results from the current run. Without this, stale outputs from a
prior run mask silent failures — if 5 files fail to process, the validator
would still see old outputs and report PASS.

| Script Level | Purge Scope | What Gets Deleted |
|:------------:|-------------|-------------------|
| `*_all` | All categories | `*/json/*_gateway.json` or `*_embedded.json` + `*/redacted/*` |
| `*_category` | One category | `<cat>/json/*_gateway.json` or `*_embedded.json` + `<cat>/redacted/*` |
| `*_file` | None | Overwrites single file only (implicit) |
| `test_agent_json.sh` (batch) | Agent_Tool_Results | `json/*_gateway.json` + `redacted/*.json` |
| `test_visual.sh` | Visual_PII | `json/*_visual.json` + `redacted/*` |

> **Important:** Gateway purge only deletes `*_gateway.json` metadata (not
> `*_embedded.json`), and vice versa. However, both architectures write to the
> same `redacted/` folder — the batch purge clears all redacted files regardless
> of which architecture created them. Run gateway and embedded tests in sequence,
> then validate, to see both sets of JSON metadata.

Each script produces **dual output** per file:

```
test/data/output/
├── PII_Detection/
│   ├── json/
│   │   ├── Credit_Card_Numbers_gateway.json     # NER spans + match metadata
│   │   ├── Credit_Card_Numbers_embedded.json    # Redaction counts + types
│   │   └── ...
│   └── redacted/
│       ├── Credit_Card_Numbers.txt              # FPE-encrypted (last architecture run)
│       └── ...
├── Agent_Tool_Results/
│   ├── json/
│   │   └── agent_anthropic_text_pii_gateway.json
│   └── redacted/
│       └── agent_anthropic_text_pii.json        # FPE-encrypted JSON structure
├── Visual_PII/
│   ├── json/
│   │   ├── face_single_frontal_01_visual.json   # Face/text blur stats
│   │   ├── doc_passport_01_visual.json          # Document OCR + face stats
│   │   ├── screenshot_email_inbox_1920x1080_visual.json  # Screenshot detection
│   │   ├── exif_screenshot_cleanshot_visual.json # EXIF-based detection
│   │   └── nsfw_positive_placeholder_01_visual.json      # NSFW detection
│   └── redacted/
│       └── face_single_frontal_01.jpg           # Blurred image
└── ...
```

> **Note:** Both architectures write to the same `redacted/` folder with the same
> filename. Running gateway then embedded (or vice versa) overwrites the previous
> redacted file. The `json/` folder preserves both with `_gateway.json` / `_embedded.json`
> suffixes.

### Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `PROXY_URL` | `http://127.0.0.1:18790` | Proxy base URL |
| `PROVIDER_PREFIX` | `/anthropic` | Route prefix for FPE pass-through |
| `ECHO_PORT` | `18791` | Echo server listen port |
| `CAPTURE_DIR` | `/tmp/oo_echo_captures` | Echo server capture directory |
| `AUTH_TOKEN` | `~/.openobscure/.auth-token` | Proxy auth token |
| `USE_NER` | `0` | Set to `1` for embedded NER bridge mode |
| `NO_AUTO_ECHO` | `0` | Set to `1` to skip auto-start of echo server |

---

## Test Scripts Reference

### Files

| Script | Type | Purpose |
|--------|:----:|---------|
| `echo_server.mjs` | Infra | Captures FPE-encrypted request bodies from proxy |
| `test_gateway_file.sh` | Gateway | NER metadata + FPE-encrypted file (single file) |
| `test_gateway_category.sh` | Gateway | Batch: all files in one input subfolder |
| `test_gateway_all.sh` | Gateway | Full suite: all 5 text categories (auto-starts echo) |
| `test_embedded_file.mjs` | Embedded | JSON metadata + label-redacted file (single file) |
| `test_embedded_category.mjs` | Embedded | Batch: all files in one input subfolder |
| `test_embedded_all.mjs` | Embedded | Full suite: all 5 text categories |
| `test_agent_json.sh` | Gateway | FPE for agent tool result JSON files |
| `test_visual.sh` | Gateway | Image pipeline: face/OCR/NSFW blur stats + blurred output |
| `validate_results.sh` | Both | Pass/fail validator (threshold or strict snapshot mode) |
| `generate_snapshot.sh` | Both | Generates snapshot.json from current output for `--strict` mode |

### Config & Data

| File | Purpose |
|------|---------|
| `test/config/test_fpe.toml` | Proxy config routing Anthropic provider to echo server |
| `test/expected_results.json` | Per-file expected minimum match counts, types, and must_detect items (threshold mode) |
| `test/snapshot.json` | Exact detection counts for regression testing (strict mode, generated) |

### How Each Script Produces Redacted Output

| Script Type | Redaction Method | Detail |
|------------|:----------------:|--------|
| **Gateway text** | FPE capture | Wraps file in Anthropic message → proxy FPE-encrypts → echo server saves → script extracts message content |
| **Gateway agent JSON** | FPE capture (nested) | Serializes entire JSON as string → proxy's nested JSON scanner FPE-encrypts PII within → deserialized back |
| **Gateway visual** | Proxy pipeline | Proxy blurs faces/text in base64 images → script captures from echo server response |
| **Embedded text** | `redactPii()` | Plugin returns `result.text` with `[REDACTED-*]` labels → written directly |
| **Both** | No proxy/plugin code modified | All scripts are API consumers only |

---

## Output Format

### Gateway JSON Metadata (`json/*_gateway.json`)

```json
{
  "file": "Credit_Card_Numbers.txt",
  "architecture": "gateway",
  "redaction_mode": "fpe",
  "total_matches": 25,
  "type_summary": { "credit_card": 25 },
  "matches": [
    { "start": 45, "end": 64, "type": "credit_card", "confidence": 1.0 },
    { "start": 120, "end": 131, "type": "ssn", "confidence": 1.0 }
  ]
}
```

### Embedded JSON Metadata (`json/*_embedded.json`)

```json
{
  "file": "Credit_Card_Numbers.txt",
  "architecture": "embedded",
  "mode": "redactPii",
  "total_matches": 25,
  "type_summary": { "credit_card": 25 }
}
```

### Redacted File Examples

**Gateway FPE** (`redacted/Credit_Card_Numbers.txt`):
```
The customer paid with card 4732-8294-5617-3048 on file.
Previous transaction used 5891-0237-4615-9820.
```
(Original numbers replaced with different digits, same format)

**Embedded Labels** (`redacted/Credit_Card_Numbers.txt`):
```
The customer paid with card [REDACTED-CC] on file.
Previous transaction used [REDACTED-CC].
```

### How to Verify FPE Output

FPE-encrypted values look like real data — you can't spot them by eye. To verify:

```bash
# Diff the original against the redacted file
diff test/data/input/PII_Detection/Credit_Card_Numbers.txt \
     test/data/output/PII_Detection/redacted/Credit_Card_Numbers.txt

# Look for label tags (non-FPE types like IPv4, GPS, Person)
grep -E '\[(IPv4|IPv6|GPS|MAC|health_keyword|child_keyword|PERSON_|LOCATION_|ORG_)' \
     test/data/output/PII_Detection/redacted/Mixed_Structured_PII.txt
```

---

## Pass/Fail Validation

### Overview

The validator (`validate_results.sh`) checks test results against a **manifest of
expected outcomes** (`expected_results.json`). It produces a per-file PASS/FAIL
report and exits with code 0 (all pass) or 1 (any failure).

This replaces ad-hoc "eyeball the numbers" verification with deterministic,
CI-compatible assertions.

### Expected Results Manifest (`test/expected_results.json`)

The manifest defines per-file expectations for all 45 text-based input files:

```json
{
  "PII_Detection/Credit_Card_Numbers.txt": {
    "min_matches": 15,
    "expected_types": ["credit_card"],
    "must_detect": [
      {"text": "4532015112830366", "type": "credit_card"},
      {"text": "4111-1111-1111-1111", "type": "credit_card"}
    ]
  },
  "Structured_Data_PII/employee_roster.csv": {
    "min_matches": 59,
    "expected_types": ["ssn", "email", "phone"]
  }
}
```

| Field | Purpose |
|-------|---------|
| `min_matches` | Minimum `total_matches` in gateway JSON. Set to ~85% of actual PII count to catch regressions while allowing minor variance. |
| `expected_types` | PII type names that MUST appear in `type_summary`. Uses the 1-2 most reliably detected types per file. |
| `must_detect` | (Optional) Array of `{text, type}` objects. The validator confirms each PII string is covered by a scanner match at the correct offset and type. |

Thresholds are set at ~85% of actual counts across all categories.

### Snapshot-Based Regression Testing (`test/snapshot.json`)

The snapshot captures exact detection counts from a known-good test run:

```json
{
  "_meta": {"version": "1.0", "generated": "..."},
  "gateway": {"PII_Detection/Credit_Card_Numbers.txt": {"total_matches": 17, "type_summary": {"credit_card": 17}}},
  "audio": {"Audio_PII/audio_ssn_single.wav": {"pii_detected": true, "keywords": "SOCIAL SECURITY", "action": "PII_DETECTED"}},
  "visual": {
    "Visual_PII/face_single_frontal_01.jpg": {
      "subcategory": "Faces", "faces_blurred": 1, "text_regions_detected": 0,
      "nsfw_blocked": false, "screenshot_detected": false
    },
    "Visual_PII/screenshot_email_inbox_1920x1080.png": {
      "subcategory": "Screenshots", "faces_blurred": 0, "text_regions_detected": 5,
      "nsfw_blocked": false, "screenshot_detected": true
    }
  }
}
```

| Section | What's Compared | Tolerance |
|---------|----------------|-----------|
| `gateway` | `total_matches` + per-type counts | Exact match |
| `audio` | `pii_detected`, `action`, `keywords` | Exact match (keywords warns only) |
| `visual` | `faces_blurred`, `text_regions_detected`, `nsfw_blocked`, `screenshot_detected` | Exact match |

To regenerate the snapshot after scanner changes:

```bash
./test/scripts/generate_snapshot.sh
```

### Validation Checks

**Threshold mode** (default) runs these checks sequentially per file:

| # | Check | What it catches |
|---|-------|----------------|
| 1 | Gateway JSON output exists | Tests not run for this category |
| 2 | `total_matches >= min_matches` | Broken scanner, missing regex patterns, model not loaded |
| 3 | All `expected_types` present in `type_summary` | Type misclassification, missing scanner module |
| 4 | `must_detect` strings covered by matches | Specific PII item regression (offset + type) |
| 5 | Redacted output file exists | FPE capture file missing, echo server not saving |
| 6 | Redacted file differs from original | Echo server down, proxy not FPE-encrypting, wrong config |
| 7 | Visual: `faces_blurred >= min_faces` | Face detector regression (Faces, Documents) |
| 8 | Visual: `text_regions >= min_text_regions` | OCR pipeline regression (Documents, Screenshots) |
| 9 | Visual: `nsfw_blocked` matches expected | NSFW detector false positive/negative |
| 10 | Visual: `screenshot_detected` matches expected | Screenshot heuristic regression (EXIF, Screenshots) |

**Strict mode** (`--strict`) checks exact counts against the snapshot:

| # | Check | What it catches |
|---|-------|----------------|
| 1 | Gateway `total_matches` matches snapshot | Any change in detection count |
| 2 | Gateway per-type counts match snapshot | Type distribution shifts |
| 3 | Audio `pii_detected` + `action` match | KWS detection regression |
| 4 | Visual `faces_blurred` + `text_regions` match | Image pipeline face/OCR regression |
| 5 | Visual `nsfw_blocked` + `screenshot_detected` match | NSFW/screenshot detection regression |

Additional checks (both modes):
- **FPE HTTP status**: warns (non-blocking) if `fpe_http_status != 200` in gateway JSON
- **Embedded validation**: if embedded results exist, checks `total_matches >= 30%` of gateway threshold
- **Coverage check**: warns if any input file has no manifest entry

### Running the Validator

```bash
# Threshold validation with per-file output (default)
./test/scripts/validate_results.sh

# Strict snapshot comparison (exact counts)
./test/scripts/validate_results.sh --strict

# Summary only (no per-file lines)
./test/scripts/validate_results.sh --summary

# JSON report for CI pipelines
./test/scripts/validate_results.sh --json

# Skip embedded/audio/visual checks
./test/scripts/validate_results.sh --gateway-only

# Strict + JSON (for CI regression testing)
./test/scripts/validate_results.sh --strict --json

# Regenerate snapshot after scanner changes
./test/scripts/generate_snapshot.sh
```

### Exit Codes

| Code | Meaning |
|:----:|---------|
| `0` | All files passed |
| `1` | One or more files failed |
| `2` | No test results found (tests not run yet) |

### Example Output

```
============================================
  OpenObscure PII Detection Validator
  2026-02-21T23:50:02Z
============================================

Found: 45 gateway JSON, 45 embedded JSON, 90 redacted files

--- PII_Detection ---
  PASS  PII_Detection/Credit_Card_Numbers.txt              22 matches (credit_card:22)
  PASS  PII_Detection/Social_Security_Numbers.txt          20 matches (ssn:20)
  FAIL  PII_Detection/Phone_Numbers.txt                    matches: 5 < min 30
  ...

--- Coverage Check ---
  All text input files have manifest entries.

--- Gateway Type Totals ---
  credit_card          47
  ssn                  35
  phone                82
  email                91
  ...

============================================
  Result: FAIL

  Passed:    44 / 45
  Failed:    1 / 45

  Failed files:
    - PII_Detection/Phone_Numbers.txt: matches: 5 < min 30

============================================
```

### JSON Report Format (`--json`)

```json
{
  "timestamp": "2026-02-21T23:50:10Z",
  "result": "FAIL",
  "pass": 44,
  "fail": 1,
  "warn": 0,
  "skip": 0,
  "total": 45,
  "uncovered_files": 0,
  "failures": [
    "PII_Detection/Phone_Numbers.txt: matches: 5 < min 30"
  ],
  "warnings": []
}
```

### Updating the Manifest

When adding new test input files or adjusting detection thresholds:

1. Add an entry to `test/expected_results.json` with conservative `min_matches`
2. Run the test suite: `./test/scripts/test_gateway_all.sh`
3. Check actual counts: `jq '.total_matches, .type_summary' test/data/output/<category>/json/<name>_gateway.json`
4. Set `min_matches` to ~85% of the actual count
5. Set `expected_types` to the 1-2 most reliably detected types
6. Optionally add `must_detect` entries for high-value PII strings
7. Run the validator to confirm: `./test/scripts/validate_results.sh`
8. Regenerate the snapshot: `./test/scripts/generate_snapshot.sh`
9. Verify strict mode: `./test/scripts/validate_results.sh --strict`

### Quality Criteria (Manual Review)

Beyond the automated pass/fail checks, these criteria apply to manual review of results:

| Criterion | Pass Condition |
|-----------|---------------|
| **FPE format preservation** | Encrypted CC is still 16 digits; encrypted SSN is still `NNN-NN-NNNN` |
| **FPE uniqueness** | Same original value encrypted to different outputs (per-record tweaks) |
| **Luhn validation** | Only valid card numbers detected (no random 16-digit sequences) |
| **SSN range** | No 000/666/900+ area numbers matched |
| **Phone separators** | Bare digit runs NOT matched (requires `-`, `.`, ` `, or `+`) |
| **Confidence scores** | Regex matches = 1.0, NER matches >= 0.85 |
| **Code fence awareness** | PII inside ``` blocks detected per `respect_code_fences` setting |

---

## Troubleshooting

### Proxy & Echo Server

| Issue | Cause | Fix |
|-------|-------|-----|
| `Connection refused` on 18790 | Proxy not running | `OPENOBSCURE_MASTER_KEY=$(openssl rand -hex 32) ./target/release/openobscure-proxy --config test/config/test_fpe.toml serve` |
| `Connection refused` on 18791 | Echo server not running | `node test/scripts/echo_server.mjs` (or let `test_gateway_all.sh` auto-start it) |
| `401 Unauthorized` | Auth token mismatch | Add `-H "X-OpenObscure-Token: $(cat ~/.openobscure/.auth-token)"` or set `AUTH_TOKEN` env var |
| `413 Payload Too Large` | Text exceeds 64KB | Scripts auto-truncate; for manual tests, split large files |
| FPE capture file missing | Echo server didn't save | Check `CAPTURE_DIR` path matches between script and echo server; verify proxy routes to echo server (`test_fpe.toml`) |
| Redacted file = original | Proxy not routing to echo | Verify proxy was started with `--config test/config/test_fpe.toml`, not default config |
| `502 Bad Gateway` | Echo server unreachable | Check echo server is running on the port configured in `test_fpe.toml` |
| Validator passes but tests actually failed | Stale results from previous run | Batch scripts auto-purge; if running single-file scripts, manually delete old outputs first |

### Embedded Plugin

| Issue | Cause | Fix |
|-------|-------|-----|
| `Cannot find module` | Plugin not built | `cd openobscure-plugin && npm run build` |
| NER types missing | Regex-only mode | Use `USE_NER=1 node test/scripts/test_embedded_all.mjs` with proxy running |
| Empty NER response | Scanner mode mismatch | Check `scanner_mode` in config (`auto`/`ner`/`crf`/`regex`) |
| Low confidence scores | NER model not loaded | Ensure model files exist and device meets RAM tier |

### Detection

| Issue | Cause | Fix |
|-------|-------|-----|
| Multilingual PII missed | Language detection disabled | Ensure `whatlang` feature enabled in proxy build |
| Code fence PII missed | `respect_code_fences = true` | Set to `false` in config to scan inside code fences |
| Agent nested JSON missed | Nesting depth > 2 | Known limitation — proxy scans 2 levels of nested JSON |
