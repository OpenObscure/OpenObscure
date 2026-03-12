# Alternatives Comparison

How OpenObscure compares to other PII protection tools. This is a factual comparison — each tool solves a real problem and has genuine strengths.

---

## Comparison Table

| Tool | Deployment | PII handling | Images / Voice | LLM response scanning | Language | License | Maturity |
|------|-----------|--------------|---------------|----------------------|----------|---------|---------|
| **OpenObscure** | On-device (proxy or native lib) | FF1 FPE — encrypted, format-preserving | Yes (face/OCR/NSFW) / Yes (KWS) | Yes — cognitive firewall | Rust + TypeScript | MIT OR Apache-2.0 | Pre-1.0, no formal audit |
| **Microsoft Presidio** | Self-hosted (Python service or library) | Redaction / replacement / anonymization | No / No | No | Python | MIT | Mature, widely deployed |
| **AWS Comprehend PII** | Cloud API | Redaction or entity detection | No / No | No | API (any language) | Commercial | Production-grade, AWS-managed |
| **Google Cloud DLP** | Cloud API | Redaction, tokenization, bucketing | Limited (OCR via Vision API) / No | No | API (any language) | Commercial | Production-grade, Google-managed |
| **Private AI** | Cloud or on-premise | Redaction / replacement | No / No | No | API | Commercial | Mature, enterprise focus |
| **Piiano Vault** | Cloud or self-hosted | Tokenization (vault-based, reversible) | No / No | No | API | Commercial (free tier) | Production-grade |
| **LLM Guard** | Self-hosted (Python library) | Redaction / prompt injection detection | No / No | Yes (partial — injection/jailbreak focus) | Python | MIT | Active, growing |

---

## Tool-by-Tool Notes

### Microsoft Presidio

Presidio is a strong choice for Python-native applications that need flexible, customizable PII detection and anonymization. It has a large community, supports custom recognizers, and integrates well with spaCy NER pipelines. It uses redaction (replacing values with entity labels like `<PERSON>`) rather than format-preserving encryption, which means the LLM sees `<SSN>` tokens instead of realistic-looking data. There is no image pipeline, voice support, or LLM response scanning.

Presidio is the right choice when: you need deep Python integration, you want extensive customization of the detection pipeline, and your threat model doesn't require format-preserving encryption or response scanning.

### AWS Comprehend PII Detection

Comprehend PII is a managed AWS service that detects and optionally redacts PII in text. It's easy to integrate for teams already in the AWS ecosystem and handles a broad range of entity types. All data leaves your infrastructure and transits to AWS — this is the fundamental trade-off. There is no support for images, voice, or LLM response scanning.

Comprehend is the right choice when: you're already on AWS, you're comfortable with cloud-based PII processing, and your compliance requirements permit sending data to a third-party service.

### Google Cloud DLP

Google DLP provides detection, redaction, tokenization, and de-identification for structured and unstructured text. It is the most feature-complete cloud PII tool in terms of transformation options. Like Comprehend, data is sent to Google's infrastructure. The Vision API can be used alongside DLP for image OCR, but this requires separate integration.

Cloud DLP is the right choice when: you need sophisticated de-identification transformations (k-anonymity, l-diversity), you're in GCP, and cloud data transmission is acceptable.

### Private AI

Private AI is a commercial on-premise option with strong language coverage (50+ languages) and accuracy claims. It focuses on PII redaction and replacement. An on-premise deployment means data stays in your infrastructure, which is its main advantage over the cloud-API tools. No image pipeline, no voice, no LLM response scanning. Pricing is commercial.

Private AI is the right choice when: you need on-premise deployment, broad language coverage matters, and you want a supported commercial product with SLAs.

### Piiano Vault

Piiano Vault takes a fundamentally different approach — it's a data vault that tokenizes PII. Applications store PII in the vault and use opaque tokens everywhere else. This is excellent for long-term PII governance and reduces the blast radius of data breaches. It's less suited for the AI agent use case where you need the LLM to process contextually meaningful data, not opaque tokens.

Piiano is the right choice when: your primary concern is storing PII safely and minimizing exposure at rest, not real-time processing of agent traffic.

### LLM Guard

LLM Guard is the closest tool in spirit to OpenObscure — it's an open-source Python library that scans both prompts and LLM outputs. Its focus is on prompt injection, jailbreak detection, and output toxicity, with some PII detection support. It does not support images, voice, or format-preserving encryption. The scanning approach is Python-based and runs in-process with the agent.

LLM Guard is the right choice when: you're in Python, your primary concern is prompt injection and LLM output safety (rather than PII encryption), and you don't need image or voice coverage.

---

## When to Choose OpenObscure

OpenObscure is the right fit when:

- **On-device is a hard requirement** — data must not leave the machine, including to a self-hosted backend
- **Format-preserving encryption matters** — you need the LLM to reason about encrypted data structurally, not see placeholder tokens
- **Multi-modal coverage is required** — you're processing images (faces, OCR, NSFW) or voice alongside text
- **LLM response integrity matters** — you want to scan responses for manipulation techniques, not just sanitize inputs
- **Mobile deployment is needed** — you need the same protection logic embedded in an iOS or Android app via native bindings
- **You want a proxy, not a library** — any agent or tool that makes HTTP requests to an LLM API can use OpenObscure without code changes

OpenObscure's weaknesses to be honest about: it is pre-1.0 with no formal security audit, has a smaller community than Presidio or the cloud tools, and the enterprise compliance features (ROPA, DPIA, breach notification) are not in the community edition.
