# Contributing to OpenObscure

## Feature Gating Protocol

Every feature MUST be tier-gated via `FeatureBudget` in `device_profile.rs`. No exceptions.

The system auto-detects device RAM, classifies a tier (Full >= 8GB, Standard 4-8GB, Lite < 4GB), and derives a `FeatureBudget` that controls which features activate. Features use a dual-gate pattern: `config.<feature>.enabled && budget.<feature>_enabled` — config is the operator's intent, budget is the hardware gate. Both must be true.

### Adding a New Feature (Checklist)

1. Add `<feature>_enabled: bool` field to `FeatureBudget` struct in `openobscure-proxy/src/device_profile.rs`
2. Set it per-tier in all **6 budget arms**: 3 in `budget_for_gateway()` + 3 in `budget_for_embedded()`
3. Gate initialization in `main.rs` using: `if config.<feature>.enabled && budget.<feature>_enabled { ... }`
4. Add the field name to `GATED_FEATURES` in `test_feature_gate_registry_parity` (same file)
5. Add the field to `FeatureBudgetSummary` in `health.rs`
6. Add assertions to existing budget tests: `test_budget_gateway_full`, `test_budget_gateway_standard`, `test_budget_gateway_lite`

### Enforcement Layers

- **Compile-time**: `FeatureBudget` has no `Default` impl. All 6 struct literals must initialize every field or compilation fails.
- **Test-time**: `test_feature_gate_registry_parity` verifies every registered feature exists in the budget AND differs between Full and Lite tiers (catches always-on fields).

### Template (Image Pipeline Pattern)

```rust
// main.rs — the canonical pattern
let feature = if config.<feature>.enabled && budget.<feature>_enabled {
    match Feature::new(&config.<feature>) {
        Ok(f) => Some(Arc::new(f)),
        Err(e) => { oo_warn!(...); None }
    }
} else if config.<feature>.enabled && !budget.<feature>_enabled {
    oo_info!(..., "<Feature> disabled by device budget", tier = %tier);
    None
} else {
    None
};
```

## Testing

### Running Tests

```bash
# L0 proxy — lib tests (unit + integration)
cargo test --lib --all-features

# L0 proxy — binary tests
cargo test --bin openobscure-proxy

# L1 plugin
cd openobscure-plugin && npm test

# L2 crypto
cd openobscure-crypto && cargo test
```

### Test Conventions

- Never modify code in the test repo (`/Users/admin/Test/OpenObscure`) — commit, push, pull there
- For GPL-licensed models: run the download script instead of committing model files

## Project Structure

```
openobscure-proxy/     L0: Rust PII proxy (core PII detection + encryption)
openobscure-plugin/    L1: TypeScript gateway plugin
openobscure-crypto/    L2: Encrypted storage (AES-256-GCM + Argon2id)
openobscure-napi/      NAPI addon (L1 native bridge)
openobscure-ner/       NER training pipeline (Python, dev-only)
```

Each component has its own `ARCHITECTURE.md` with module-level details. See the root [ARCHITECTURE.md](ARCHITECTURE.md) for system-level overview.
