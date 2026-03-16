# OpenObscure Setup

OpenObscure has two deployment models. Choose the one that fits your use case:

| Model | Use Case | Guide |
|-------|----------|-------|
| **Gateway (Proxy)** | Run OpenObscure as a sidecar HTTP proxy alongside an AI agent (OpenClaw, etc.). PII is intercepted in transit. | [gateway_setup.md](gateway_setup.md) |
| **Embedded (Native Library)** | Compile OpenObscure into your iOS/macOS/Android app as a static library. No HTTP proxy needed. | [embedded_setup.md](embedded_setup.md) |

## Common Prerequisites

Both models require:

- **macOS** (Apple Silicon or Intel, macOS 13+)
- **Rust toolchain** (1.75+)

### Install Developer Tools

```bash
xcode-select --install
```

### Install Homebrew

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

Follow the on-screen instructions. If it tells you to run extra commands to add Homebrew to your path, run those too.

### Install Rust

```bash
brew install rustup
rustup-init -y
source "$HOME/.cargo/env"
```

Verify:

```bash
rustc --version   # 1.75+
```

### Download OpenObscure

```bash
cd ~/Desktop
git clone https://github.com/OpenObscure/OpenObscure.git
cd OpenObscure
```

### Download AI Models

All ONNX models (face detection, OCR, NSFW classifier, NER, KWS, response integrity) are stored in Git LFS. Pull them after cloning:

```bash
git lfs install   # one-time: register LFS hooks in your git config
git lfs pull      # ~120MB: all models (NER, BlazeFace, SCRFD, PaddleOCR, NSFW, KWS, RI)
```

> **No LFS?** If you downloaded a ZIP or your CI environment lacks LFS support, use the fallback download scripts instead:
> ```bash
> ./build/download_models.sh full   # BlazeFace, SCRFD, PaddleOCR, NSFW (~14MB from web)
> ./build/download_kws_models.sh    # KWS Zipformer INT8 (~5MB from github.com/k2-fsa)
> ```
> Note: NER and response-integrity models are only available via Git LFS.

---

Next: [Gateway Setup](gateway_setup.md) or [Embedded Setup](embedded_setup.md)
