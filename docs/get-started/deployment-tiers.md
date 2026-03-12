# Deployment Tiers

OpenObscure detects device hardware at startup and automatically selects which features to activate. A phone with 12GB RAM gets the same PII detection efficacy as a desktop server. No manual configuration is required — but you can override if needed.

---

**Contents**

- [How Tiers Are Determined](#how-tiers-are-determined)
- [Feature Matrix by Tier](#feature-matrix-by-tier)
- [Features Available on All Tiers](#features-available-on-all-tiers)
- [Overriding Auto-Detection](#overriding-auto-detection)
- [How Tier Gating Works](#how-tier-gating-works)
- [Next Steps](#next-steps)

## How Tiers Are Determined

Tier classification uses **total physical RAM** — a stable device indicator that doesn't fluctuate with app usage.

| Total RAM | Tier |
|-----------|------|
| ≥8GB | **Full** |
| ≥4GB and <8GB | **Standard** |
| <4GB | **Lite** |

**Boundary devices are inclusive on the upper tier.** A device with exactly 8192MB (8GB) is classified as Full; a device with exactly 4096MB (4GB) is classified as Standard. The classification uses `>=` comparisons (`device_profile.rs:233–234`), confirmed by tests `test_tier_full_boundary_8gb` and `test_tier_standard_boundary_4gb`.

RAM detection is platform-native:

| Platform | Detection Method |
|----------|-----------------|
| macOS / iOS | `sysctl hw.memsize` |
| Linux / Android | `/proc/meminfo MemTotal` |
| Windows | `GlobalMemoryStatusEx` |

---

## Feature Matrix by Tier

### Gateway (Desktop / Server)

| Feature | Full (8GB+) | Standard (4–8GB) | Lite (<4GB) |
|---------|:-----------:|:-----------------:|:-----------:|
| **Max RAM budget** | 275MB | 200MB | 80MB |
| **NER model** | DistilBERT | TinyBERT | TinyBERT |
| **NER pool size** | 2 | 1 | 1 |
| **CRF scanner** | Yes | Yes | Yes |
| **Ensemble voting** | Yes | No | No |
| **Image pipeline** | Yes | Yes | Yes |
| **Face detector** | SCRFD-2.5GF | SCRFD-2.5GF | Ultra-Light RFB-320 ¹ |
| **OCR tier** | Full recognition | Full recognition | Detect and fill |
| **NSFW classifier** | Yes | Yes | No ² |
| **Screen guard** | Yes | Yes | No ² |
| **Voice KWS** | Yes | Yes | No ² |
| **Cognitive firewall (R2)** | Yes | Yes | No ² |
| **Name gazetteer** | Yes | Yes | Yes |
| **Keyword dictionary** | Yes | Yes | Yes |
| **Model idle timeout** | 300s | 120s | 60s |

> ¹ **Ultra-Light RFB-320 vs SCRFD-2.5GF:** Lite uses a 320×240 input model vs Full/Standard's 640×640, with a higher confidence threshold (0.7 vs 0.5). It has lower recall on small, distant, or profile faces. For deployments where face redaction is safety-critical, use Standard or Full tier.
>
> ² **Safety implication on Lite:** NSFW detection, screen guard, voice keyword spotting, and the cognitive firewall (R2) are all disabled on Lite. Image content and voice transcripts are not scanned on Lite. Also note that on Standard and Lite, **ensemble voting is disabled** (`ensemble_enabled = false`), so the +0.15 confidence agreement bonus is never applied — borderline-confidence matches (in the 0.35–0.50 range) that multiple engines agree on will not receive a boost and may be discarded by the `min_confidence` filter. Operators with strict content safety or high-recall requirements should enforce Standard or Full tier via `scanner_mode` override or deployment policy.

### Embedded (Mobile / Library)

Embedded budgets are **20% of total device RAM**, clamped to a 12–275MB range. A 12GB phone gets 275MB (capped). A 3GB phone gets 3072MB/5 = 614MB → clamped to 122MB.

> **Embedded Lite ≠ Gateway Lite.** Gateway Lite has a fixed 80MB budget. Embedded Lite uses 20% of device RAM (minimum 12MB), so a 3GB phone gets a 122MB embedded budget — sufficient for features that Gateway Lite cannot run (e.g., NER when budget ≥25MB, image pipeline when budget ≥40MB). The tier label is the same but the capability ceiling differs between deployment models.

Feature availability on embedded also depends on whether the budget can fit each model:

| Feature | Full (8GB+) | Standard (4–8GB) | Lite (<4GB) |
|---------|:-----------:|:-----------------:|:-----------:|
| **Max RAM budget** | 275MB (cap) | 20% of RAM | 20% of RAM |
| **NER model** | DistilBERT | DistilBERT if ≥120MB, else TinyBERT | TinyBERT (if ≥25MB) |
| **CRF scanner** | Yes | Yes | If budget ≥25MB |
| **Ensemble voting** | Yes | No | No |
| **Image pipeline** | Yes | If budget ≥100MB | If budget ≥40MB |
| **Face detector** | SCRFD-2.5GF | SCRFD-2.5GF | Ultra-Light RFB-320 |
| **NSFW classifier** | Yes | If budget ≥150MB | No |
| **Voice KWS** | Yes | If budget ≥50MB | No |
| **Cognitive firewall (R2)** | Yes | If budget ≥80MB | No |
| **Model idle timeout** | 300s | 120s | 60s |

---

## Features Available on All Tiers

These work regardless of tier because they require no ML models:

- **Regex scanner** — credit cards (Luhn-validated), SSNs (range-validated), phones, emails, API keys
- **Network/device identifiers** — IPv4, IPv6, GPS coordinates, MAC addresses, IBANs
- **Multilingual national IDs** — 9 languages with check-digit validation (DNI, NIR, CPF, My Number, etc.)
- **Keyword dictionary** — ~700 health/child terms (multilingual)
- **Name gazetteer** — embedded name lists, no model files needed
- **FPE encryption** — FF1 with format preservation for all 10 PII types
- **EXIF stripping** — automatic metadata removal from images
- **SSE streaming** — frame accumulation for cross-frame PII detection

---

## Overriding Auto-Detection

### Force a Scanner Mode (Gateway)

Hardware auto-detection is the default when `scanner_mode = "auto"`. Override with an explicit mode in your config:

```toml
# config/openobscure.toml
[scanner]
scanner_mode = "ner"    # Force NER regardless of device tier
# scanner_mode = "crf"  # Force CRF only
# scanner_mode = "regex" # Force regex + keywords only (no ML models)
```

> **Warning — forcing NER on Lite bypasses the budget check.** If the NER model file is present, the system attempts to load it regardless of available RAM. On a Gateway Lite device (80MB budget), loading TinyBERT (13.7MB) succeeds comfortably; loading DistilBERT (63.7MB) is near the budget ceiling. If the load fails (OOM or ONNX error), the system falls back to regex+keywords with a WARN log — no crash occurs, but semantic entity detection is lost for the session.

### Disable Auto-Detection (Embedded)

On mobile, set `auto_detect: false` in `MobileConfig` to disable hardware profiling and use config defaults:

```json
{
  "auto_detect": false,
  "scanner_mode": "regex"
}
```

### Verify Active Tier

The health endpoint reports the detected tier and active feature budget:

```bash
curl -s http://127.0.0.1:18790/_openobscure/health | jq '.device_tier, .feature_budget'
```

```json
"full"
{
  "tier": "full",
  "max_ram_mb": 275,
  "ner_enabled": true,
  "crf_enabled": true,
  "ensemble_enabled": true,
  "image_pipeline_enabled": true
}
```

---

## How Tier Gating Works

Every feature requires **three gates** to all pass before it activates:

1. **Config:** `feature.enabled = true` (operator intent)
2. **Budget:** device tier supports the feature (hardware gate)
3. **Model file:** the ONNX model file must be present at the configured path

```rust
// Pseudocode — actual pattern in main.rs
let feature = if config.feature.enabled && budget.feature_enabled {
    Some(Feature::new())  // still a no-op if model file is missing
} else {
    None
};
```

**Gate 2 failure (budget):** If you enable a feature in config but the device tier doesn't support it, you'll see a log message at **INFO** level (not WARN):

```
INFO: <Feature> disabled by device budget (tier=lite)
```

No health endpoint counter is incremented for budget-disabled features — detection is log-based only. To confirm active features at runtime, query `/_openobscure/health` and inspect `feature_budget`.

**Gate 3 failure (model file):** If a model file is missing but the budget allows the feature, the system degrades silently with a WARN log: NER falls back to CRF then regex; face detection falls back to BlazeFace. No error is returned to the caller. See [Detection Engine Configuration](../configure/detection-engine-configuration.md) for model file paths.

This means you can ship the same config to all devices — the tier system handles the rest.

---

## Next Steps

- [Detection Engine Configuration](../configure/detection-engine-configuration.md) — fine-tune scanner settings, model paths, and thresholds
