# Release Process

> This is a single-maintainer project. This document exists so contributors know what to expect and so the process is consistent as the project grows.

---

## Versioning Policy

OpenObscure follows [Semantic Versioning](https://semver.org) (`MAJOR.MINOR.PATCH`).

**Current status: v0.1 (pre-1.0).** The public API (UniFFI bindings, config keys, CLI flags) is not yet stable. Minor version bumps may include breaking changes. Each release's CHANGELOG entry will explicitly call out any breaking changes.

| Change type | Version bump | Example |
|-------------|-------------|---------|
| New capability, new detection type, new deployment model | **Minor** | 0.1.x → 0.2.0 |
| Bug fix, test addition, doc improvement, performance improvement | **Patch** | 0.1.0 → 0.1.1 |

The 1.0.0 release will be tagged when the UniFFI API, config schema, and CLI flags are considered stable. Until then, expect breaking changes between minor versions.

---

## Release Checklist

Before tagging a release:

1. **All CI checks pass** — `cargo test --lib --all-features`, `clippy`, `fmt --check`, `npm test` all green on `main`
2. **CHANGELOG updated** — Add a dated entry for the new version. Move items from `[Unreleased]` to the version section.
3. **Version bumped** in:
   - `openobscure-core/Cargo.toml` (`version = "x.y.z"`)
   - `openobscure-plugin/package.json` (`"version": "x.y.z"`)
   - `openobscure-napi/package.json` (`"version": "x.y.z"`)
4. **Commit** the version bump and CHANGELOG update:
   ```bash
   git add CHANGELOG.md openobscure-core/Cargo.toml openobscure-plugin/package.json openobscure-napi/package.json
   git commit -m "Release vX.Y.Z"
   ```
5. **Tag** the release:
   ```bash
   git tag -a vX.Y.Z -m "Release vX.Y.Z"
   git push origin vX.Y.Z
   ```
6. **GitHub Release** — create a release from the tag with release notes summarizing the CHANGELOG entry. The CI release workflow (`release.yml`) builds and attaches binaries automatically once the tag is pushed.

---

## Binary Distribution Policy

> **Current status:** Binary distribution (GitHub Releases, signed installers, app store submissions) requires completing the BIS self-classification and ERN notification process. See [EXPORT_CONTROL_NOTICE.md](../../EXPORT_CONTROL_NOTICE.md).

Once export control requirements are satisfied, binaries are built by the release CI workflow for:

- macOS (Apple Silicon, x86_64) — universal binary
- Linux (x64, ARM64)
- Windows (x64)
- iOS XCFramework (device + simulator slices)
- Android `.so` libraries (arm64-v8a, x86_64)
- UniFFI Swift/Kotlin bindings (attached as a zip artifact)

Source code distribution is not subject to these restrictions.

---

## NAPI Package Release

NAPI platform packages are published separately from the core release, triggered by a `napi-v*` tag (e.g. `napi-v0.1.1`). This runs `.github/workflows/napi-publish.yml` and publishes 3 platform packages plus the umbrella to npm.

**First publish is complete.** `@openobscure/scanner-napi` v0.1.1 and three platform packages are live on npm:
- `@openobscure/scanner-napi-darwin-arm64` — macOS Apple Silicon
- `@openobscure/scanner-napi-linux-x64-gnu` — Linux x64 glibc
- `@openobscure/scanner-napi-linux-arm64-gnu` — Linux ARM64 glibc

### Checklist for future NAPI releases

1. **Bump versions** in `openobscure-napi/package.json` and each `openobscure-napi/npm/*/package.json`, keeping them in sync.
2. **Ensure `NPM_TOKEN` secret** is set in GitHub repository → Settings → Secrets → Actions (already configured — verify it hasn't expired).
3. **Tag and push** `napi-vX.Y.Z` to trigger the workflow. Platform packages publish first, then the umbrella (`needs: [publish-platform-packages]`).

### Known limitations and deferred platforms

| Platform | Blocker |
|----------|---------|
| `linux-x64-musl` (Alpine) | `ort` 2.0 provides no prebuilt binaries for musl targets — build fails at link time |
| `linux-arm64-musl` (Alpine) | Same `ort` limitation |
| `darwin-x64` (macOS Intel) | Requires cross-compile infrastructure; low priority (ARM64 covers macOS) |

These platforms are excluded from the current workflow. Add them once `ort` adds musl support or an alternative ONNX Runtime integration path is available.

### Phase 4 — promote to hard dependency

Once the remaining platforms are published (or a decision is made to ship without them), move `@openobscure/scanner-napi` from `optionalDependencies` to `dependencies` in `openobscure-plugin/package.json`. This makes the 15-type Rust scanner the guaranteed default for all `openobscure-plugin` installs rather than an optional upgrade.

---

## Security Releases

For vulnerabilities requiring an expedited fix:

1. **Do not open a public issue.** Report via the GitHub Security Advisory process (see [SECURITY.md](../../SECURITY.md)).
2. Maintainer acknowledges within 48 hours and opens a private advisory.
3. Fix is developed in a private fork or the advisory's private branch.
4. A patch release is prepared and tested against the private branch.
5. The fix and advisory are disclosed simultaneously — the advisory publishes when the patched release tag is pushed.
6. A `PATCH` version is released even if the fix is a single line.

CVE assignment is requested for vulnerabilities that meet the bar (RCE, key material exposure, bypass of the core FPE guarantee).

---

## Who Can Release

Releases are currently maintainer-only. To delegate:

- Add the person as a repository collaborator with Write access
- Ensure they have read this document
- For the first delegated release, do a dry-run with a pre-release tag (`vX.Y.Z-rc.1`) to verify the release workflow works end-to-end
