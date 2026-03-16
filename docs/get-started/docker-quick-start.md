# Docker Quick Start

Run the OpenObscure Gateway as a container in two commands — no Rust toolchain required.

For build options, full-image setup, Git LFS details, CI/CD integration, and troubleshooting, see the [Docker Setup and Testing Guide](./docker-setup-testing-guide.md).

---

## Prerequisites

- Docker 24+ (or Docker Desktop)
- An existing OpenObscure install for `print-key`, **or** a 64-hex-char key you already have

If you don't have a local install yet, generate a key with:
```bash
openssl rand -hex 32
```

---

## Quick start (slim image)

```bash
# 1. Pull the latest slim image (regex + keywords + NER, no voice models, ~80MB)
docker pull ghcr.io/openobscure/openobscure:slim

# 2. Start the gateway
docker run -d \
  --name openobscure \
  -e OPENOBSCURE_MASTER_KEY=$(openobscure print-key) \
  -e OPENOBSCURE_AUTH_TOKEN=mytoken \
  -p 18790:18790 \
  ghcr.io/openobscure/openobscure:slim

# 3. Verify it's running
curl -sf -H "X-OpenObscure-Token: mytoken" http://localhost:18790/_openobscure/health
```

Point your agent at `http://127.0.0.1:18790`. See [Integration Reference](../integrate/provider_integration.md) for provider-specific setup.

---

## Image variants

| Tag | Contents | Size (compressed) |
|-----|----------|-------------------|
| `slim` | Regex + keywords + NER (TinyBERT INT8) | ~80MB |
| `full` | + NSFW, face detection, OCR, voice KWS, RI model | ~600MB |

> **`full` is not published to GHCR yet** — large model files require Git LFS. Build locally with `make docker-full`.

---

## Secret management

The container has no OS keychain. The FPE key must be supplied through one of these methods:

| Method | How |
|--------|-----|
| **Env var** (CI/CD, ECS, Cloud Run) | `-e OPENOBSCURE_MASTER_KEY=<64-hex>` |
| **Docker Secrets / K8s Secrets** | Mount key file at `/run/secrets/openobscure-master-key` |
| **Custom file path** | `-e OPENOBSCURE_KEY_FILE=/etc/myapp/fpe.key -v ...` |
| **Volume-mounted home** | Mount a volume at `/home/oo` containing `.openobscure/master-key` |

**Do not** pass the key as a plain `docker run` argument — it will appear in `docker inspect`. Use an environment variable from a secret store instead.

---

## Persistent configuration

Override any TOML setting by mounting a custom config file:

```bash
docker run -d \
  --name openobscure \
  -e OPENOBSCURE_MASTER_KEY=<key> \
  -e OPENOBSCURE_AUTH_TOKEN=mytoken \
  -v $(pwd)/openobscure-core/config:/home/oo/config:ro \
  -p 18790:18790 \
  ghcr.io/openobscure/openobscure:slim
```

---

## docker-compose (local development)

The repo includes a `docker-compose.yml` that wires the gateway with an optional echo upstream.

```bash
# 1. Copy and fill in the env file
cp .env.example .env
# Edit .env: set OPENOBSCURE_MASTER_KEY

# 2. Start
docker compose up -d

# 3. Check health
docker compose ps
curl -H "X-OpenObscure-Token: devtoken" http://localhost:18790/_openobscure/health

# 4. Stop
docker compose down
```

---

## Production secret management

For production deployments, inject the FPE key from your existing secret store:

| Platform | Guide |
|----------|-------|
| HashiCorp Vault | [deploy/vault/](../../deploy/vault/) — Vault Agent sidecar writes to `/run/secrets/` |
| Kubernetes Secrets | [deploy/k8s/](../../deploy/k8s/) — volume-mounted Secret at `/run/secrets/` |
| AWS Secrets Manager | [deploy/aws/](../../deploy/aws/) — ECS task env var injection |
| GCP Secret Manager | [deploy/gcp/](../../deploy/gcp/) — CSI driver file mount |

---

## Build locally

```bash
# Slim (no models, fast build)
make docker-slim

# Full (requires git lfs pull first, bakes in all models)
make docker-full

# Run slim using key from local keychain
make docker-run-slim
```
