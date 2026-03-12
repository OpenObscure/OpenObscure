# Release Process

> This is a single-maintainer project. This document exists so contributors know what to expect and so the process is consistent as the project grows.

---

## Versioning Policy

OpenObscure follows [Semantic Versioning](https://semver.org) (`MAJOR.MINOR.PATCH`):

| Change type | Version bump | Example |
|-------------|-------------|---------|
| Breaking change to public API (UniFFI bindings, config keys, CLI flags) | **Major** | 0.x.y → 1.0.0 |
| New capability, new detection type, new deployment model | **Minor** | 0.1.x → 0.2.0 |
| Bug fix, test addition, doc improvement, performance improvement | **Patch** | 0.1.0 → 0.1.1 |

Pre-1.0: minor version bumps may include breaking changes. Each release's CHANGELOG entry will explicitly call out any breaking changes.

---

## Release Checklist

Before tagging a release:

1. **All CI checks pass** — `cargo test --lib --all-features`, `clippy`, `fmt --check`, `npm test` all green on `main`
2. **CHANGELOG updated** — Add a dated entry for the new version. Move items from `[Unreleased]` to the version section.
3. **Version bumped** in:
   - `openobscure-proxy/Cargo.toml` (`version = "x.y.z"`)
   - `openobscure-plugin/package.json` (`"version": "x.y.z"`)
   - `openobscure-napi/package.json` (`"version": "x.y.z"`)
4. **Commit** the version bump and CHANGELOG update:
   ```bash
   git add CHANGELOG.md openobscure-proxy/Cargo.toml openobscure-plugin/package.json openobscure-napi/package.json
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
