# Contributing to OpenObscure

Fork the repo, create a branch, make your change, run tests, open a PR against `main`. Maintainers review within a few days; commits are squash-merged.

## Quick Workflow

1. Fork and clone the repository
2. Create a branch: `git checkout -b fix/your-change`
3. Build to confirm your environment works: `cd openobscure-proxy && cargo build`
4. Make your change. Keep commits focused — one logical change per commit.
5. Run tests for what you touched:
   ```bash
   cargo test --lib --all-features   # L0 proxy (expect 700+ passing)
   cd openobscure-plugin && npm test  # L1 plugin
   ```
6. Check formatting and lints:
   ```bash
   cargo fmt --check
   cargo clippy --all-features -- -D warnings
   ```
7. Push your branch and open a PR. Describe *why* the change is needed, not just what changed.

CI must pass before review begins — it runs `cargo test`, `clippy`, `fmt`, and `npm test`.

## Good First Issues

Look for **`good first issue`** labels on GitHub. Documentation improvements and test coverage gaps are always welcome.

## Adding a New Capability?

Every new feature must be tier-gated via `FeatureBudget` in `device_profile.rs`. Read the [Feature Gating Protocol](docs/contribute/feature-gating-protocol.md) before writing any code — this is mandatory, not optional.

## Full Contributing Guide

**[docs/contribute/contributing.md](docs/contribute/contributing.md)** — complete guide covering test conventions, model-gated tests, enterprise-only features, export control, and the full PR review process.
