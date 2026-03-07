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

OpenObscure uses small ONNX models for face detection, OCR text detection, and voice PII keyword spotting:

```bash
./build/download_models.sh       # ~25MB: BlazeFace, SCRFD, PaddleOCR, NudeNet, NER
./build/download_kws_models.sh   # ~5MB: sherpa-onnx KWS Zipformer
```

> **Note:** The gateway model uses all models. The embedded model auto-selects models based on device tier and available RAM.

---

Next: [Gateway Setup](gateway_setup.md) or [Embedded Setup](embedded_setup.md)
