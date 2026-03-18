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
   cd openobscure-core && cargo build
   ```
4. **Make your changes.** Keep commits focused — one logical change per commit.
5. **Run the tests** for every component you touched:
   ```bash
   # L0 Core proxy — lib tests (unit + integration)
   cargo test --lib --all-features

   # L0 Core proxy — binary tests
   cargo test --bin openobscure

   # L1 plugin
   cd openobscure-plugin && npm test

   # NAPI addon
   cd openobscure-napi && npm test

   # L2 crypto (enterprise only — skip if you don't have enterprise/ in your clone)
   cd enterprise/openobscure-crypto && cargo test
   ```

   > **What passing looks like:** `cargo test --lib --all-features` should report 700+ passing
   > tests. If you see significantly fewer, model-gated tests may be skipping silently — run
   > `./build/download_models.sh` first to enable the image pipeline and NER test coverage.
   > Tests requiring the `voice` feature are gated separately and skip without KWS models.

6. **Add tests** for new behavior. Bug fixes should include a regression test.
7. **Check formatting and lints:**
   ```bash
   cargo fmt --check
   cargo clippy --all-features -- -D warnings
   ```
8. **Commit** with a clear message describing *why*, not just *what*.
9. **Push** your branch and open a Pull Request against `main`. The PR description should
   explain why the change is needed, not just what changed.
10. **CI must pass** — the pipeline runs `cargo test --lib --all-features`,
    `cargo clippy --all-features -- -D warnings`, `cargo fmt --check`, and `npm test`
    for the plugin. All checks must be green before review begins.
11. **Respond to review feedback.** Maintainers will review within a few days. Commits are
    squash-merged — your branch history doesn't need to be clean, but the final commit
    message should describe the complete change.

---

## Feature Gating Protocol

> **Adding a new capability** (new detection type, new model, new config option)? The Feature
> Gating Protocol is mandatory — read it before writing any code.
> **Bug fix or test improvement?** You can skip this section.

Every new capability must be tier-gated via `FeatureBudget` in `device_profile.rs`. This ensures OpenObscure runs correctly across Full (≥4GB), Standard (2–4GB), and Lite (<2GB) devices.

The full protocol — including the 6-step checklist, enforcement layers, and code template — lives in [Feature Gating Protocol](feature-gating-protocol.md).

---

## Test Conventions

Test commands are listed in step 5 above. For the full testing guide — including accuracy benchmarks, model-gated tests, and gateway integration tests — see [Testing Guide](../../test/TESTING_GUIDE.md).

**Key rule:** For GPL-licensed models, run the download script instead of committing model files to the repo.

---

## Project Structure

```
openobscure-core/          L0: Rust PII proxy (core detection + encryption)
openobscure-plugin/         L1: TypeScript gateway plugin
openobscure-napi/           NAPI addon (L1 native bridge)
openobscure-ner/            NER training pipeline (Python, dev-only)
enterprise/
  openobscure-crypto/       L2: Encrypted storage (enterprise only)
```

Each component has a dedicated architecture page under [docs/architecture/](../architecture/system-overview.md).

---

## Export Control

OpenObscure includes cryptographic functionality subject to export regulations. See [EXPORT_CONTROL_NOTICE.md](../../EXPORT_CONTROL_NOTICE.md) for details.
