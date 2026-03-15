# Docker Setup and Testing Guide

Complete reference for building, running, and testing OpenObscure Gateway as a Docker container — both `slim` and `full` image variants.

For the 3-command quick start, see [Docker Quick Start](./docker-quick-start.md).

---

## Contents

- [Image Variants](#image-variants)
- [Prerequisites](#prerequisites)
- [Running the Slim Image](#running-the-slim-image)
- [Key Resolution](#key-resolution)
- [Testing a Running Container](#testing-a-running-container)
- [docker-compose (Local Development)](#docker-compose-local-development)
- [Building Locally](#building-locally)
- [AI/ML Models — Inventory and Git LFS](#aiml-models--inventory-and-git-lfs)
- [Running the Full Image](#running-the-full-image)
- [Pulling from GHCR](#pulling-from-ghcr)
- [CI/CD Integration](#cicd-integration)
- [Production Deployment](#production-deployment)
- [Troubleshooting](#troubleshooting)
- [Dockerfile Reference](#dockerfile-reference)

---

## Image Variants

| Tag | Detection engines | Compressed size | Cargo features |
|-----|------------------|-----------------|----------------|
| `slim` | Regex, keywords, NER (TinyBERT INT8) | ~80 MB | `--no-default-features --features server` |
| `full` | + NSFW classifier, face detection, OCR, voice KWS, response integrity | ~600 MB | default features |

**When to use slim:** API proxy, edge deployments, resource-constrained environments. All structured PII types (SSN, credit card, email, phone, etc.) are covered.

**When to use full:** Images, audio transcripts, and NSFW content pass through the proxy and require the model-backed pipeline.

The `slim` tag is published to GHCR automatically on every push to `main` and on version tags. The `full` image must be built locally or published manually — see [Running the Full Image](#running-the-full-image).

---

## Prerequisites

### For pulling and running pre-built images

- Docker 24+ (or Docker Desktop)
- No Rust toolchain required

### For building locally

- Docker 24+ with Buildx plugin (`docker buildx version`)
- **Slim:** ~3 GB working disk (Rust toolchain + build artifacts)
- **Full:** ~5 GB working disk (same + model file copy into image layers)
- **Full only:** Git LFS installed (`git lfs version`) — required to download real model files before build

### Checking Git LFS

```bash
git lfs version
# git-lfs/3.x.x (...)
```

If not installed:

```bash
# macOS
brew install git-lfs

# Ubuntu/Debian
sudo apt-get install git-lfs

# After install
git lfs install
```

---

## Running the Slim Image

### Minimal run

```bash
docker run -d \
  --name openobscure \
  -e OPENOBSCURE_MASTER_KEY=$(openssl rand -hex 32) \
  -e OPENOBSCURE_AUTH_TOKEN=mytoken \
  -p 18790:18790 \
  ghcr.io/openobscure/openobscure:slim
```

Replace `$(openssl rand -hex 32)` with your actual key (see [Key Resolution](#key-resolution)).

### Checking the container started

```bash
docker ps --filter name=openobscure
# CONTAINER ID   IMAGE        ...   STATUS
# abc123         ...          ...   Up 3 seconds
```

If the status is `Exited`, the container crashed at startup — usually a missing or invalid key:

```bash
docker logs openobscure
```

### Stopping and removing

```bash
docker stop openobscure
docker rm openobscure
```

### Non-root user and volume mount permissions

The container runs as user `oo`, UID **10000**. When bind-mounting host files (config overrides, key files), the host file must be readable by UID 10000:

```bash
# Make a config file readable by the container user
chown 10000:10000 my-openobscure.toml
# or make it world-readable
chmod 644 my-openobscure.toml
```

### Mounting a custom config

```bash
# Linux / macOS (bash/zsh)
docker run -d \
  --name openobscure \
  -e OPENOBSCURE_MASTER_KEY=<key> \
  -e OPENOBSCURE_AUTH_TOKEN=mytoken \
  -v $(pwd)/openobscure-core/config:/home/oo/config:ro \
  -p 18790:18790 \
  ghcr.io/openobscure/openobscure:slim

# Windows PowerShell — use ${PWD} instead of $(pwd)
# docker run -d ... -v ${PWD}/openobscure-core/config:/home/oo/config:ro ...
```

### Listen address

The published image already sets `ENV OPENOBSCURE_LISTEN_ADDR=0.0.0.0` so the container is reachable from the host on port 18790. If you build a custom image, set this explicitly — the application default is `127.0.0.1` (loopback), which makes the container unreachable.

---

## Key Resolution

The container has no OS keychain. The FPE master key is resolved in this order:

| Step | Source | How to supply |
|------|--------|---------------|
| 1 | `OPENOBSCURE_MASTER_KEY` env var | `-e OPENOBSCURE_MASTER_KEY=<64-hex>` |
| 2 | `/run/secrets/openobscure-master-key` file | Docker Secrets or K8s Secret volume mount |
| 3 | `OPENOBSCURE_KEY_FILE` env var (custom path) | `-e OPENOBSCURE_KEY_FILE=/etc/myapp/fpe.key -v ...` |
| 4 | `~/.openobscure/master-key` file | Volume-mount a home directory containing the file |
| 5 | OS keychain | Not available in containers — always fails |

If no step resolves, the container exits at startup with an error listing all five options.

### Generating a key

```bash
# Without a local install
openssl rand -hex 32

# With a local install
openobscure print-key
```

### Security note

Never pass the key as a plain `docker run` argument — it appears in `docker inspect` output and shell history. Use `-e` (from a secret store) or a mounted file.

### Testing step 2 locally (without Docker Secrets)

```bash
echo "<your-key>" > /tmp/oo-key
docker run -d \
  -e OPENOBSCURE_AUTH_TOKEN=mytoken \
  -v /tmp/oo-key:/run/secrets/openobscure-master-key:ro \
  -p 18790:18790 \
  ghcr.io/openobscure/openobscure:slim
```

For production secret stores, see [Production Deployment](#production-deployment).

---

## Testing a Running Container

### Health check

```bash
curl -sf \
  -H "X-OpenObscure-Token: mytoken" \
  http://localhost:18790/_openobscure/health
```

Expected: HTTP 200 with a JSON body.

**Poll-until-ready** (same pattern used in CI):

```bash
# First, confirm the container is actually running (not Exited)
docker ps --filter name=openobscure --format "{{.Status}}"

for i in $(seq 1 15); do
  curl -sf \
    -H "X-OpenObscure-Token: mytoken" \
    http://localhost:18790/_openobscure/health \
    && echo "Ready" && break
  echo "Attempt $i — retrying in 2s..."
  sleep 2
done
```

### NER endpoint (slim and full)

Verify PII detection is working:

```bash
curl -sf \
  -X POST \
  -H "X-OpenObscure-Token: mytoken" \
  -H "Content-Type: application/json" \
  -d '{"text": "My name is John and my SSN is 123-45-6789"}' \
  http://localhost:18790/_openobscure/ner
```

Expected: JSON response listing detected PII spans and types.

### Image pipeline endpoint (full image only)

Send a base64-encoded image to verify the NSFW → face → OCR pipeline is reachable:

```bash
# Tiny 1×1 white PNG
TINY_PNG="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwADhQGAWjR9awAAAABJRU5ErkJggg=="

PAYLOAD=$(printf \
  '{"model":"test","messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,%s"}}]}]}' \
  "$TINY_PNG")

STATUS=$(curl -s -o /dev/null -w "%{http_code}" \
  -X POST \
  -H "X-OpenObscure-Token: mytoken" \
  -H "Content-Type: application/json" \
  -d "$PAYLOAD" \
  http://localhost:18790/openai/v1/chat/completions)

echo "Status: $STATUS"
# 200 or 502 (no upstream configured) = OK
# 500 = pipeline error
[[ "$STATUS" != "500" ]] || exit 1
```

A `502` response is expected and acceptable when no upstream LLM is configured — it means the proxy received the request and the image pipeline was entered. Only `500` indicates an internal error.

### docker-compose health state

```bash
docker compose ps          # shows "healthy" / "starting" / "unhealthy"
docker compose logs openobscure
```

---

## docker-compose (Local Development)

```bash
# 1. Copy the env template
cp .env.example .env

# 2. Set the FPE key
#    Either: openobscure print-key (local install)
#    Or:     openssl rand -hex 32
# Edit .env: OPENOBSCURE_MASTER_KEY=<64-hex-chars>

# 3. Start
docker compose up -d

# 4. Verify
docker compose ps
curl -H "X-OpenObscure-Token: devtoken" http://localhost:18790/_openobscure/health

# 5. Stop
docker compose down
```

### Echo upstream for testing without a real LLM

`docker-compose.yml` includes a commented-out `echo` service. Uncomment it and point `openobscure.toml`'s `upstream_url` at `http://echo:18791` to test the full proxy pipeline without an API key.

---

## Building Locally

### Slim image

```bash
make docker-slim
# or:
docker build --target slim -t openobscure:slim .
```

Expected build time: ~5 minutes on a modern laptop (Rust compile dominates).

### Full image

The full image bakes all model files into the image layer. Before building, the model files must be downloaded from Git LFS — otherwise the build succeeds but the container crashes at startup because the model files are LFS pointer stubs (~130 bytes), not real ONNX files.

```bash
# 1. Verify models are real (not pointer stubs)
ls -lh openobscure-core/models/nsfw_classifier/nsfw_5class_int8.onnx
# Must show ~83 MB — if it shows ~130 bytes, run: git lfs pull

# 2. Build
make docker-full
# make docker-full runs git lfs pull automatically before building.

# Alternatively, using raw docker build (git lfs pull must be run manually first):
# git lfs pull
# docker build --target full -t openobscure:full .
```

Expected build time: longer than slim due to copying ~229 MB of model files into the image layer.

### Identifying LFS pointer stubs

A pointer stub looks like this when inspected:

```
version https://git-lfs.github.com/spec/v1
oid sha256:80ae...
size 89155192
```

A real file will show the actual binary content. The quick check:

```bash
file openobscure-core/models/nsfw_classifier/nsfw_5class_int8.onnx
# Real:    data  (or: ONNX ML model)
# Stub:    ASCII text
```

### Multi-platform builds (amd64 + arm64)

```bash
docker buildx create --use
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  --target slim \
  -t ghcr.io/openobscure/openobscure:slim \
  --push \
  .
```

**Build time note:** arm64 cross-compilation via QEMU emulation takes **60–90 minutes on a cold build** with no cache. With GitHub Actions layer cache (`cache-from: type=gha`), subsequent runs complete in ~48 seconds. This is expected — do not cancel a first-time multi-platform build prematurely.

Note: `--load` and `--push` are mutually exclusive for multi-platform builds. Use `--push` to publish directly to a registry, or build single-platform with `--load` to test locally.

---

## AI/ML Models — Inventory and Git LFS

### Model inventory

All binary model files are stored in Git LFS. The table below covers every model in `openobscure-core/models/`:

| Model | Directory | Purpose | Tier | Size | ONNX IR | Opset | Producer |
|-------|-----------|---------|------|------|---------|-------|----------|
| TinyBERT 4L INT8 (NER) | `ner-lite/` | Named entity recognition | Lite | 14 MB | 7 | 14 | onnx.quantize |
| DistilBERT INT8 (NER) | `ner/` | Named entity recognition | Full/Standard | 64 MB | 7 | 14 | onnx.quantize |
| ViT-base INT8 (NSFW) | `nsfw_classifier/nsfw_5class_int8.onnx` | NSFW detection (5-class) | Full/Standard | 83 MB | 8 | 17 | onnx.quantize |
| ViT-base FP32 (NSFW) | `nsfw_classifier/nsfw_classifier.onnx` | NSFW detection (reference) | — | 21 MB | 7 | 12 | pytorch |
| NudeNet 320n | `nudenet/320n.onnx` | Alternative NSFW detector | — | 12 MB | 10 | 17 | pytorch |
| PP-OCRv4 detection | `paddleocr/det_model.onnx` | Text region detection | Full/Standard | 2.3 MB | 7 | 14 | — |
| PP-OCRv4 recognition | `paddleocr/rec_model.onnx` | Text recognition (CTC) | Full/Standard | 7.3 MB | 8 | 10 | — |
| TinyBERT INT8 (RI) | `ri/model_int8.onnx` | Response integrity firewall | Full/Standard | 14 MB | 7 | 14 | onnx.quantize |
| SCRFD-2.5GF | `scrfd/scrfd_2.5g.onnx` | Face detection | Full/Standard | 3.1 MB | 7 | 11 | pytorch |
| UltraLight RFB-320 | `ultralight/version-RFB-320.onnx` | Face detection | Lite | 1.2 MB | 4 | 9 | pytorch |
| BlazeFace | `blazeface/blazeface.onnx` | Face detection (legacy) | — | 408 KB | 4 | 10 | pytorch |
| Zipformer encoder INT8 | `kws/encoder-*.int8.onnx` | Voice keyword spotting | Full/Standard | 4.6 MB | 7 | 13 | onnx.quantize |
| Zipformer decoder INT8 | `kws/decoder-*.int8.onnx` | Voice keyword spotting | Full/Standard | 271 KB | 7 | 13 | onnx.quantize |
| Zipformer joiner INT8 | `kws/joiner-*.int8.onnx` | Voice keyword spotting | Full/Standard | 160 KB | 7 | 13 | onnx.quantize |
| SentencePiece vocab | `kws/bpe.model` | KWS tokenizer vocabulary | Full/Standard | 239 KB | — | — | — |

**Total on disk (uncompressed): ~229 MB**

Models marked "—" in Tier are present in the repository but not part of the primary inference pipeline. BlazeFace and the FP32 NSFW reference model are retained for comparison purposes.

### Git LFS — how it works for contributors

All `.onnx`, `.bin`, and `.model` files under `openobscure-core/models/` are tracked via Git LFS (see `.gitattributes`). When you clone the repository normally:

```bash
git clone https://github.com/openobscure/openobscure.git
```

Git LFS downloads **pointer stub files** (~130 bytes each) instead of the actual model binaries. The pointer contains the file hash and size but not the content. This keeps `git clone` fast (~few MB) even though the models total 229 MB.

To download the actual model files:

```bash
git lfs pull
```

This downloads all LFS-tracked files for the current commit. You only need this when:
- Building the `full` Docker image locally
- Running model-gated tests (`test_image_all_visual_pii_files`, `test_audio_transcript_all`)
- Working on model inference code directly

**You do NOT need `git lfs pull` to:**
- Build or run the `slim` image
- Run text-only tests
- Develop, build, or run the Rust code (models are loaded at runtime, not compile time)

### Git LFS bandwidth — community note

GitHub LFS has a **1 GB/month free bandwidth quota** per repository. Each `git lfs pull` that fetches all model files consumes ~229 MB of quota. For open-source contributors:

- If you only need to build the slim image or work on non-model code, skip `git lfs pull`
- Use `git lfs pull --include="openobscure-core/models/ner-lite/**"` to fetch only the specific model you need
- The published `full` Docker image on GHCR lets you run the full pipeline without pulling LFS at all

Checking what LFS files are currently downloaded vs. stub:

```bash
git lfs ls-files
# * = file is downloaded locally
# - = stub only (run git lfs pull to download)
#
# Example:
#   80ae8... * openobscure-core/models/nsfw_classifier/nsfw_5class_int8.onnx  (downloaded)
#   80ae8... - openobscure-core/models/nsfw_classifier/nsfw_5class_int8.onnx  (stub)
```

---

## Running the Full Image

### From GHCR (when published)

When a `full` image is published to GHCR, pulling requires no Git LFS and no local build:

```bash
docker pull ghcr.io/openobscure/openobscure:full

docker run -d \
  --name openobscure-full \
  -e OPENOBSCURE_MASTER_KEY=<key> \
  -e OPENOBSCURE_AUTH_TOKEN=mytoken \
  -p 18790:18790 \
  ghcr.io/openobscure/openobscure:full
```

> The `full` image is not published automatically on every commit. Check [GHCR](https://github.com/orgs/openobscure/packages) for the latest published tag.

### Built locally

After completing the [full image build steps](#full-image), run:

```bash
docker run -d \
  --name openobscure-full \
  -e OPENOBSCURE_MASTER_KEY=<key> \
  -e OPENOBSCURE_AUTH_TOKEN=mytoken \
  -p 18790:18790 \
  openobscure:full

# Stop and remove when done
docker stop openobscure-full && docker rm openobscure-full
```

### Resource requirements

The full image loads multiple ONNX models into memory on demand. Recommended minimums:

| Resource | Minimum | Recommended |
|----------|---------|-------------|
| RAM | 512 MB | 1 GB |
| CPU | 1 core | 2 cores |
| Disk (image) | 700 MB | — |

---

## Pulling from GHCR

### Package visibility

GHCR packages must be set to **public** for the community to pull without authentication:

> GitHub → your profile → **Packages** → select package → **Package settings** → **Change visibility → Public**

Once public, anyone can pull without a token:

```bash
docker pull ghcr.io/openobscure/openobscure:slim
docker pull ghcr.io/openobscure/openobscure:full
```

### Authenticating (maintainers / private packages)

```bash
echo $GITHUB_TOKEN | docker login ghcr.io -u <github-username> --password-stdin
```

In GitHub Actions:

```yaml
- uses: docker/login-action@v3
  with:
    registry: ghcr.io
    username: ${{ github.actor }}
    password: ${{ secrets.GITHUB_TOKEN }}
```

The job must have `permissions: packages: read` (pull) or `packages: write` (push).

### Image name casing

GHCR rejects uppercase letters in image references. `github.repository` returns the original casing (e.g. `OpenObscure/OpenObscure`).

In bash scripts, lowercase with parameter expansion:

```bash
IMAGE_NAME="${IMAGE_NAME,,}"   # bash 4+
# ghcr.io/openobscure/openobscure:slim
```

In GitHub Actions workflows, set it before any docker step:

```yaml
- name: Lowercase image name
  run: echo "IMAGE_NAME=${IMAGE_NAME,,}" >> $GITHUB_ENV
```

---

## CI/CD Integration

The repo includes `.github/workflows/docker.yml` as a complete worked example. Summary:

### `validate-features` job (every push and PR)

Runs `cargo build --release --no-default-features --features server` (slim) and `cargo check --release` (full features) on a plain Ubuntu runner.

`cargo check` is used for the full-features step rather than `cargo build` because the full feature set links against `libonnxruntime` and `libsherpa-onnx` native libraries, which are not installed on the stock runner. `cargo check` type-checks without linking and catches compilation errors without needing the native libraries.

### `build-and-push` job (depends on validate-features)

Builds the slim image for `linux/amd64` and `linux/arm64` using Docker Buildx with QEMU, pushes to GHCR. Uses `cache-from: type=gha` / `cache-to: type=gha,mode=max` to persist layer cache between runs.

### `smoke-test-slim` job (push to main only, not PRs)

Pulls the just-published slim image, starts a container, and verifies:
1. Health endpoint responds within 30 seconds
2. NER endpoint returns a result
3. `/run/secrets/` key resolution works via bind-mounted file

### `smoke-test-full` job (manual `workflow_dispatch` only)

Triggered manually with `build_full: true`. Builds the full image on the runner (using `lfs: true` checkout), smoke-tests health, NER, and image pipeline. Does **not** push to GHCR — the full image is published manually by maintainers on deliberate releases.

---

## Production Deployment

Deploy examples for common secret stores:

| Platform | Pattern | Reference |
|----------|---------|-----------|
| Kubernetes | K8s Secret volume-mounted at `/run/secrets/openobscure-master-key` | [deploy/k8s/](../../deploy/k8s/) |
| HashiCorp Vault | Vault Agent sidecar writes key to `/run/secrets/` before gateway starts | [deploy/vault/](../../deploy/vault/) |
| AWS ECS Fargate | Secrets Manager injects `OPENOBSCURE_MASTER_KEY` as env var | [deploy/aws/](../../deploy/aws/) |
| GCP GKE | Secrets Store CSI driver mounts key file at `/run/secrets/` | [deploy/gcp/](../../deploy/gcp/) |

Resource sizing starting points (from `deploy/k8s/deployment.yaml`):

| Image | RAM request | RAM limit | CPU request | CPU limit |
|-------|------------|-----------|------------|-----------|
| slim | 128 Mi | 512 Mi | 100m | 500m |
| full | 512 Mi | 1024 Mi | 200m | 1000m |

---

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `unauthorized` or `denied` pulling from GHCR | Package is private or no docker login | Make package public in GHCR settings, or `docker login ghcr.io` first |
| `invalid reference format: repository name must be lowercase` | Mixed-case image name (e.g. from `github.repository`) | Use `${IMAGE_NAME,,}` in bash; add lowercase step in GHA env |
| Container starts but host cannot connect on port 18790 | Default listen address is `127.0.0.1` (loopback) | Set `OPENOBSCURE_LISTEN_ADDR=0.0.0.0` — already baked into the published image |
| Container exits at startup with key-related error | No key supplied and OS keychain unavailable | Supply key via `OPENOBSCURE_MASTER_KEY`, `/run/secrets/`, or `OPENOBSCURE_KEY_FILE` |
| Full image starts but crashes on model load | `git lfs pull` not run before build — model files are ~130-byte LFS stubs | Run `git lfs pull`, verify with `file openobscure-core/models/nsfw_classifier/nsfw_5class_int8.onnx` → must show `data` not `ASCII text` |
| Volume-mounted files not readable in container | Host files not readable by UID 10000 | `chown 10000:10000 <file>` on host, or use `chmod 644` |
| `cargo build` fails in CI for full features | `libonnxruntime` / `libsherpa-onnx` not on plain runner | Use `cargo check --release` for type-checking; actual linking happens inside Docker build stage |
| Multi-platform build takes 60–90 minutes | QEMU arm64 emulation, cold cache | Expected on first run. Add `cache-from: type=gha` / `cache-to: type=gha,mode=max` — cached runs complete in ~48 seconds |
| `docker build` fails: cannot find example `demo_image_pipeline` | Missing `examples/` or `benches/` in build context | Dockerfile explicitly copies both directories; check `.dockerignore` is not excluding them |
| Image pipeline endpoint returns HTTP 500 | ONNX model session error inside full image | Check `docker logs` for model load errors; confirm you are using the full image, not slim |
| Full image build much slower than slim | ~229 MB of model files copied into image layers | Expected — no workaround. Plan for extra time on first full build. |
| `git lfs pull` fails with bandwidth error | GitHub LFS 1 GB/month free quota exceeded | Use `git lfs pull --include="path/to/specific/model"` to fetch only what you need; or pull the published full image from GHCR instead |

### Debugging key resolution

Container startup logs show which resolution step succeeded at `INFO` level and which steps were skipped. To see them:

```bash
docker logs openobscure 2>&1 | grep -iE "key|vault|secret"
```

### Verifying models loaded (full image)

```bash
docker logs openobscure-full 2>&1 | grep -iE "model|onnx|load"
# Expect: lines showing model paths and load times
# No "error" or "failed" lines
```

---

## Dockerfile Reference

The `Dockerfile` at the repo root uses three stages:

```
builder  →  slim  →  full
```

**Stage 1 — `builder` (Ubuntu 24.04)**

- Installs `curl pkg-config libssl-dev cmake g++` via apt
- Installs Rust stable via rustup (no pre-installed toolchain image, to stay on Ubuntu 24.04)
- Copies `openobscure-core/{Cargo.toml,Cargo.lock,src/,examples/,benches/,config/}`
- Both `examples/` and `benches/` must be copied — `Cargo.toml` references them and the build fails with a manifest error if they are absent
- Builds with `RUSTFLAGS="-C link-arg=-lstdc++"` and `--no-default-features --features server`

**Stage 2 — `slim` (Ubuntu 24.04 runtime)**

- Installs `ca-certificates curl` only (no build tools)
- Creates non-root user `oo`, UID **10000**
- Copies binary from builder and `config/` directory
- Sets `ENV OPENOBSCURE_LISTEN_ADDR=0.0.0.0` — overrides the application default of `127.0.0.1`
- Exposes port 18790
- Entrypoint: `openobscure serve`

**Stage 3 — `full` (`FROM slim AS full`)**

- Inherits everything from `slim` — only adds one layer
- Copies `openobscure-core/models/` into `/home/oo/models/`
- Requires `git lfs pull` on the build host before this stage runs — otherwise model files are LFS pointer stubs

---

## Related Documentation

- [Docker Quick Start](./docker-quick-start.md) — 3-command quick start
- [Configuration Reference](../configure/config-reference.md) — all env vars including `OPENOBSCURE_MASTER_KEY`, `OPENOBSCURE_AUTH_TOKEN`, `OPENOBSCURE_LISTEN_ADDR`, `OPENOBSCURE_KEY_FILE`
- [deploy/k8s/](../../deploy/k8s/) — Kubernetes deployment and secret manifests
- [deploy/vault/](../../deploy/vault/) — HashiCorp Vault Agent sidecar config
- [deploy/aws/](../../deploy/aws/) — ECS Fargate task definition and IAM policy
- [deploy/gcp/](../../deploy/gcp/) — GCP Secrets Store CSI driver config
