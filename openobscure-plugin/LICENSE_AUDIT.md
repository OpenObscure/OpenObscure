# OpenObscure Plugin — Dependency License Audit

> **Audit date:** 2026-02-16
> **Project license:** MIT OR Apache-2.0
> **Verdict:** All dependencies are open source with permissive licenses. No copyleft (GPL/LGPL/AGPL/MPL) found.

---

## Direct Dependencies

None (zero runtime dependencies).

## Dev Dependencies

| # | Package | Version | License | Status |
|---|---------|---------|---------|--------|
| 1 | `typescript` | ^5.4 | Apache-2.0 | OK |
| 2 | `@types/node` | ^25.2.3 | MIT | OK |
| 3 | `tsx` | ^4.21.0 | MIT | OK |

---

## Notable Transitive Dependencies

| Package | License | Notes |
|---------|---------|-------|
| `esbuild` | MIT | Used by tsx for TypeScript transformation |

---

## License Distribution

| License | Count | Examples |
|---------|-------|---------|
| MIT | ~4 | @types/node, tsx, esbuild |
| Apache-2.0 | 1 | typescript |

---

## Action Items

1. No copyleft dependencies — plugin can be released under MIT OR Apache-2.0
2. Zero runtime dependencies — only dev/build tooling has external packages
3. No native modules or platform-specific concerns
