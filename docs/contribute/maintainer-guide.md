# Becoming a Maintainer

> This document describes the path from contributor to maintainer. It will be activated once the project has an established contributor base (target: 3-5 repeat contributors).

---

## Contributor Ladder

### Contributor

Anyone who submits a pull request that gets merged.

- No prerequisites beyond following the [Contributing Guide](contributing.md)
- All contributions go through standard PR review

### Reviewer

Trusted contributors who can approve PRs but not merge.

**Requirements:**
- 5+ merged PRs across at least 2 components (L0 Core, L1 Plugin, docs, CI)
- Demonstrated understanding of the security model (FPE, fail-open, threat boundaries)
- Has read the [Threat Model](../architecture/threat-model.md)
- Nominated by the maintainer, accepted after a 1-week objection period

**Responsibilities:**
- Review PRs for correctness, security, and adherence to project conventions
- Flag security-sensitive changes for maintainer review
- Help triage issues

### Maintainer

Full commit access. Can merge PRs and cut releases.

**Requirements:**
- 6+ months as reviewer
- Understands all 3 layers (L0 Core, L1 Plugin, embedded integrations)
- Has reviewed security-sensitive changes (FPE engine, scanner, image pipeline, cognitive firewall)
- Has read and understood the [Threat Model](../architecture/threat-model.md) and [Design Decisions](../architecture/design-decisions.md)
- Nominated by an existing maintainer, accepted by consensus

**Responsibilities:**
- Merge PRs after review approval
- Cut patch and minor releases per the [Release Process](release-process.md)
- Triage and respond to security reports within 48 hours
- Maintain CI/CD pipelines and pre-commit hooks

---

## What Maintainers Cannot Do Unilaterally

These changes require explicit consensus among all maintainers:

- **Change cryptographic primitives** (e.g., replacing FF1, changing key derivation)
- **Modify the threat model** (adding/removing threat categories or trust boundaries)
- **Add telemetry, analytics, or phone-home behavior** of any kind
- **Add cloud dependencies** (all processing must remain on-device)
- **Change the fail-open/fail-closed default** for any pipeline stage
- **Modify the FPE key management** (rotation, storage, derivation)

These restrictions exist because OpenObscure is a security tool — users trust it with their most sensitive data. Cryptographic and architectural decisions require deliberate consensus, not unilateral action.

---

## Security-Specific Requirements

All maintainers must understand:

1. **Why FF1, not FF3** — FF3 is NIST-withdrawn (SP 800-38G Rev 2, Feb 2025)
2. **Why fail-open** — FPE errors skip the match and forward original text; blocking the agent is worse than a missed detection
3. **Why per-record tweaks** — prevents frequency analysis across requests
4. **Why no cloud** — the entire value proposition is that PII never leaves the device
5. **Why @Transient/display-only restore** — DB must store tokens, never restored plaintext

---

## Current Status

OpenObscure is a single-maintainer project (pre-1.0). This document is published for transparency about the project's governance intentions. The contributor ladder will be activated when the community is ready.
