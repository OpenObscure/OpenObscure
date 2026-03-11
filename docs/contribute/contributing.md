# Contributing to OpenObscure

Thank you for your interest in contributing. This guide covers everything you need to go from zero to merged PR.

---

## Prerequisites

You need Rust (stable), Node.js 20+, and a working build of the proxy. If you haven't set up a development environment yet, follow the [Gateway Quick Start](../get-started/gateway-quick-start.md) first — it covers cloning, building, and verifying the proxy runs.

---

## Fork → Build → Test → PR

1. **Fork** the repository on GitHub and clone your fork locally.
2. **Create a branch** from `main` with a descriptive name:
   ```bash
   git checkout -b fix/phone-regex-false-positive
   ```
3. **Build** the proxy to confirm your environment works:
   ```bash
   cd openobscure-proxy && cargo build
   ```
4. **Make your changes.** Keep commits focused — one logical change per commit.
5. **Run the tests** for every component you touched:
   ```bash
   # L0 proxy — lib tests (unit + integration)
   cargo test --lib --all-features

   # L0 proxy — binary tests
   cargo test --bin openobscure-proxy

   # L1 plugin
   cd openobscure-plugin && npm test

   # L2 crypto
   cd openobscure-crypto && cargo test
   ```
6. **Add tests** for new behavior. Bug fixes should include a regression test.
7. **Check formatting and lints:**
   ```bash
   cargo fmt --check
   cargo clippy --all-features -- -D warnings
   ```
8. **Commit** with a clear message describing *why*, not just *what*.
9. **Push** your branch and open a Pull Request against `main`.
10. **Respond to review feedback** — maintainers may request changes before merging.

---

## Good First Issues

Look for issues labeled **`good first issue`** on GitHub. These are scoped, well-defined tasks suitable for new contributors. If none are open, documentation improvements and test coverage gaps are always welcome.

---

## Feature Gating Protocol

> **Internal process — read before adding features.**

Every feature must be tier-gated via `FeatureBudget` in `device_profile.rs`. No exceptions. This ensures OpenObscure runs correctly across Full (8GB+), Standard (4–8GB), and Lite (<4GB) devices.

The full protocol — including the 6-step checklist, enforcement layers, and code template — lives in [Feature Gating Protocol](feature-gating-protocol.md).

---

## Test Conventions

Test commands are listed in step 5 above. For the full testing guide — including accuracy benchmarks, model-gated tests, gateway integration tests, and the test repo workflow — see [Testing Guide](../../test/TESTING_GUIDE.md).

Key rules:
- Never modify code in the test repo — commit and push from dev, pull in test.
- For GPL-licensed models: run the download script instead of committing model files.

---

## Project Structure

```
openobscure-proxy/     L0: Rust PII proxy (core detection + encryption)
openobscure-plugin/    L1: TypeScript gateway plugin
openobscure-crypto/    L2: Encrypted storage (AES-256-GCM + Argon2id)
openobscure-napi/      NAPI addon (L1 native bridge)
openobscure-ner/       NER training pipeline (Python, dev-only)
```

Each component has a dedicated architecture page under [docs/architecture/](../architecture/system-overview.md).

---

## Export Control

OpenObscure includes cryptographic functionality subject to export regulations. See [EXPORT_CONTROL_NOTICE.md](../../EXPORT_CONTROL_NOTICE.md) for details.
