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

NAPI platform packages are published separately from the core release, triggered by a `napi-v*` tag (e.g. `napi-v0.1.0`). This runs `.github/workflows/napi-publish.yml` and publishes 6 platform packages plus the umbrella to npm.

### Pre-publish checklist

Before pushing the first `napi-v*` tag, the following must be resolved:

1. **Update `NER_MODEL_REPO` in `napi-publish.yml`**
   The current value (`openobscure/tinybert-ner-int8`) is a placeholder. Upload the TinyBERT INT8 model files (`model_int8.onnx`, `vocab.txt`) to a Hugging Face repository and update the env var. The `download-ner-model` job will fail with a 404 until this is done.

2. **Verify `CARGO=cargo-zigbuild` is respected by napi-rs CLI**
   The cross-compilation jobs (`linux-arm64-gnu`, `linux-arm64-musl`) rely on napi-rs 2.x honouring the `CARGO` environment variable override. If it does not, those jobs will build with plain `cargo` against the wrong target or fail. Alternative: create a `.cargo/config.toml` in `openobscure-napi/` with the zigbuild target-specific runner, or use the `--use-cross` flag with the `cross` tool instead.

3. **Review Alpine container + rustup interaction for `linux-x64-musl`**
   The `ghcr.io/napi-rs/napi-rs/nodejs-rust:lts-alpine` container already ships a musl-capable Rust toolchain. Running `rustup toolchain install stable` on top may install a glibc toolchain that produces glibc binaries. Verify the generated `.node` file links against musl (`file scanner.linux-x64-musl.node`) before publishing. If it links glibc, remove the `rustup toolchain install` step and rely on the container's pre-installed toolchain.

4. **Add `NPM_TOKEN` secret** to GitHub repository → Settings → Secrets → Actions. Both publish jobs use `${{ secrets.NPM_TOKEN }}` as `NODE_AUTH_TOKEN`.

5. **Publish order**: platform packages must be published before the umbrella, because the umbrella's `optionalDependencies` reference the platform package versions. The workflow enforces this via `needs: [publish-platform-packages]` on the `publish-umbrella` job.

### Post-publish: Phase 4

After all 6 platform packages and the umbrella are live on npm, move `@openobscure/scanner-napi` from `optionalDependencies` to `dependencies` in `openobscure-plugin/package.json` and cut a new plugin release. This makes the 15-type Rust scanner the guaranteed default for all `openobscure-plugin` installs.

### Known limitation: NER model path for npm-installed users

`autoDetectNerModelDir()` in `redactor.ts` searches `<umbrellaPackageDir>/models/ner/` first, but models are bundled in the *platform* package dir (e.g. `node_modules/@openobscure/scanner-napi-darwin-arm64/models/ner/`). NER will not auto-load from the bundled model until this path is corrected to resolve the platform package directory. The dev-layout fallback (`../openobscure-core/models/ner/`) continues to work for local builds. Tracked as Future Work in `openobscure-napi/ARCHITECTURE.md`.

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
