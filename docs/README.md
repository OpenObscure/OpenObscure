# OpenObscure Documentation

This index groups the documentation in `docs/` by topic so contributors and operators can find the right guide from one page.

## Get Started

- [Before You Start](get-started/before-you-start.md) - privacy, compliance, and deployment questions to answer before enabling the proxy.
- [Docker Quick Start](get-started/docker-quick-start.md) - local Docker setup for trying OpenObscure quickly.
- [Docker Setup Testing Guide](get-started/docker-setup-testing-guide.md) - validation steps for Docker-based setup and smoke tests.
- [Gateway Quick Start](get-started/gateway-quick-start.md) - first-run gateway workflow for routing provider traffic.
- [Gateway Operations](get-started/gateway-operations.md) - operational notes for running and monitoring the gateway.
- [Embedded Quick Start](get-started/embedded-quick-start.md) - setup path for embedded or local integrations.
- [Deployment Models](get-started/deployment-models.md) - supported local, gateway, embedded, and managed deployment patterns.
- [Deployment Tiers](get-started/deployment-tiers.md) - Lite, Standard, and Full tier capabilities and auto-detection.
- [Alternatives](get-started/alternatives.md) - comparison with adjacent privacy, redaction, and proxy approaches.
- [FAQ](get-started/faq.md) - common setup, capability, and operational questions.
- [Roadmap](get-started/roadmap.md) - planned milestones and future direction.

## Configure

- [Configuration Reference](configure/config-reference.md) - complete TOML key reference and environment variable overrides.
- [Detection Engine Configuration](configure/detection-engine-configuration.md) - scanner engines, tier mapping, ensemble behavior, and custom detection.
- [FPE Configuration](configure/fpe-configuration.md) - format-preserving encryption, key management, and fail-open/fail-closed behavior.

## Integrate

- [Provider Integration](integrate/provider_integration.md) - routing external LLM providers through OpenObscure.
- [Embedding Architecture](integrate/embedding/architecture.md) - architecture for embedded OpenObscure integration.
- [Embedded Integration](integrate/embedding/embedded_integration.md) - implementation guide for embedding OpenObscure in host applications.
- [Embedding Examples](integrate/embedding/examples/README.md) - runnable examples and reference snippets for embedded integrations.

## Operate

- [Crash Recovery](operate/crash-recovery.md) - crash buffer behavior and post-mortem recovery guidance.

## Architecture

- [System Overview](architecture/system-overview.md) - high-level components and request flow.
- [L0 Core](architecture/l0-core.md) - Rust core proxy architecture and responsibilities.
- [L1 Plugin](architecture/l1-plugin.md) - TypeScript gateway plugin architecture and responsibilities.
- [Semantic PII Detection](architecture/semantic-pii-detection.md) - semantic scanner design and model behavior.
- [Image Pipeline](architecture/image-pipeline.md) - image detection, OCR, face detection, and safety pipeline.
- [NAPI Scanner](architecture/napi-scanner.md) - native scanner bridge and packaging details.
- [Response Integrity](architecture/response-integrity.md) - response-integrity scanning and warning behavior.
- [Threat Model](architecture/threat-model.md) - security boundaries, threat assumptions, and mitigations.
- [Design Decisions](architecture/design-decisions.md) - key architecture decisions and rationale.

## Reference

- [API Reference](reference/api-reference.md) - HTTP API surface and request/response behavior.
- [Audit Log Schema](reference/audit-log-schema.md) - JSONL audit record shapes and field meanings.
- [Fail Behavior Reference](reference/fail-behavior-reference.md) - failure modes and expected proxy responses.
- [Performance](reference/performance.md) - performance expectations, budgets, and measurement notes.
- [PII Type Coverage](reference/pii-type-coverage.md) - supported PII classes and detection coverage.

## Contribute

- [Contributing Guide](contribute/contributing.md) - contributor workflow, AI policy, tests, and review expectations.
- [Feature Gating Protocol](contribute/feature-gating-protocol.md) - required tier gating process for new capabilities.
- [Maintainer Guide](contribute/maintainer-guide.md) - maintainer responsibilities and contributor ladder.
- [Release Process](contribute/release-process.md) - release workflow and publishing checklist.
