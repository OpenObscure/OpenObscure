# Key Design Decisions

Rationale for the core architectural choices in OpenObscure. These decisions are not arbitrary — each reflects a constraint, failure mode, or threat that shaped the design.

| Decision | Rationale |
|----------|-----------|
| FF1 only, never FF3 | FF3 withdrawn by NIST (SP 800-38G Rev 2, Feb 2025) |
| Fail-open default | Proxy must never block AI functionality due to FPE edge cases |
| Vault unavailable → 503 | No privacy guarantees without FPE key — blocking is correct |
| Passthrough-first auth | No duplicate key management; OpenObscure is transparent to the host agent |
| Per-record FPE tweaks | Prevents frequency analysis across requests |
| L1 redacts, not encrypts | Tool results are internal — redaction is simpler and guarantees removal |
| Synchronous hooks only | OpenClaw-specific: OpenClaw silently skips async hook returns |
| INT8 quantization for NER | FP32 TinyBERT NER = ~200MB; INT8 = ~50MB — difference between fitting and OOM. R2 uses FP32 (see below) |
| FP32 for R2, not INT8 | INT8 dynamic quantization produced 7.45 max logit error — too much accuracy loss for multi-label classification. FP32 is accurate (0.000013 max diff) at 54.9 MB |
| R1→R2 cascade | R1 is <1ms (dictionary). R2 is ~30ms (ONNX). Cascade avoids R2 overhead on clean responses at low/medium sensitivity |
| Interior mutability for R2 (`Mutex<Option<RiModel>>`) | R2 ONNX session needs `&mut self`. Mutex allows `scan(&self)` on Arc-shared scanner across async request handlers |
| Solid-fill redaction (all regions) | Gaussian blur is partially reversible by AI deblurring models; solid color fill destroys original pixels completely, compresses better in base64, and clearly signals intentional redaction. Applied to faces, OCR text, and NSFW images. |
| On-demand model loading | Face + OCR models load/evict per image, saving ~43MB between images |
| Sequential model loading | Face model loaded/used/dropped before OCR model loaded — never both in RAM |
| Two-pass body processing | Images processed first (replaces base64 strings), then text PII (replaces substrings by offset) |
| EXIF strip via decode/encode | `image` crate loads pixels only, discarding all EXIF metadata — no explicit strip step |
| 960px image cap | A 12MP ARGB bitmap = 48MB; resizing before load prevents OOM |
