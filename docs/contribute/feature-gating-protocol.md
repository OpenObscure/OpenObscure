# Feature Gating Protocol

> **Internal process** — every feature MUST be tier-gated via `FeatureBudget` in `device_profile.rs`. No exceptions.

The system auto-detects device RAM, classifies a tier (Full ≥ 4GB, Standard 2–4GB, Lite < 2GB), and derives a `FeatureBudget` that controls which features activate. Features use a dual-gate pattern: `config.<feature>.enabled && budget.<feature>_enabled` — config is the operator's intent, budget is the hardware gate. Both must be true.

For tier definitions and hardware detection, see [Deployment Tiers](../get-started/deployment-tiers.md).

---

## Adding a New Feature (Checklist)

1. Add `<feature>_enabled: bool` field to `FeatureBudget` struct in `openobscure-core/src/device_profile.rs`
2. Set it per-tier in all **6 budget arms**: 3 in `budget_for_gateway()` + 3 in `budget_for_embedded()`
3. Gate initialization in `main.rs` using: `if config.<feature>.enabled && budget.<feature>_enabled { ... }`
4. Add the field name to `GATED_FEATURES` in `test_feature_gate_registry_parity` (same file)
5. Add the field to `FeatureBudgetSummary` in `health.rs`
6. Add assertions to existing budget tests: `test_budget_gateway_full`, `test_budget_gateway_standard`, `test_budget_gateway_lite`

---

## Enforcement Layers

- **Compile-time**: `FeatureBudget` has no `Default` impl. All 6 struct literals must initialize every field or compilation fails.
- **Test-time**: `test_feature_gate_registry_parity` verifies every registered feature exists in the budget AND differs between Full and Lite tiers (catches always-on fields).

---

## Template (Image Pipeline Pattern)

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
