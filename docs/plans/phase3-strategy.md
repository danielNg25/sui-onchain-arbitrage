# Phase 3: Strategy Engine — Plan

## Goal

Build `arb-engine`, the crate that turns raw pool state into profitable opportunities. Given a swap event on any pool, the engine: (1) looks up all precomputed arbitrage cycles containing that pool, (2) golden-section searches for the optimal input amount on each cycle, (3) returns ranked opportunities with USD-denominated profit.

## What Phase 2 Provides

- `Pool::estimate_swap(token_in, amount_in) -> SwapEstimate` on every Cetus/Turbos pool — sub-microsecond local simulation via `clmm_math::simulate_swap`
- `PoolManager` with `pool(id)`, `get_pools_for_pair(a, b)`, `get_pools_for_token(token)`, `apply_event()`, full DashMap indexing
- `SwapEventData` with `pool_id`, `a_to_b`, `amount_in`, `amount_out`
- `StrategyConfig` with `max_hops`, `min_profit_mist`, `binary_search_iterations`, `whitelisted_tokens`

## Crate: `arb-engine` (`crates/arb-engine/`)

**Dependencies**: `arb-types`, `pool-manager`, `dex-common`, `dashmap`, `tokio`, `reqwest`, `serde`, `tracing`, `thiserror`

```
crates/arb-engine/src/
  lib.rs           — ArbEngine top-level struct, process_event()
  graph.rs         — ArbGraph: token adjacency from pools
  cycle.rs         — Cycle, RotatedCycle, CycleIndex, DFS cycle detection
  profit_token.rs  — ProfitTokenRegistry, GeckoTerminal price fetch
  simulator.rs     — SimCache, simulate_cycle()
  search.rs        — golden-section search for optimal amount
  opportunity.rs   — Opportunity result struct
  error.rs         — EngineError
```

## Key Design Decisions

1. **Precomputed cycles at startup** — DFS cycle detection runs once. Cycles indexed by pool_id for O(1) event-driven lookup.
2. **Event-scoped SimCache** — Pool state changes with every event. Cache is created per event, shared across parallel cycle sims, then dropped.
3. **Golden-section search** — Profit function is unimodal (not monotonic). Golden section finds maxima; binary search finds zeros.
4. **Two-phase search** — Strategic sampling identifies profitable region, then golden-section refines within it.
5. **Profit token rotation** — Cycles rotated so highest-priority profit token (SUI, USDC) is start/end token.
6. **max_amount = event.amount_in** — Arb opportunity bounded by the price dislocation from the original swap.

## Config Additions

Added to `StrategyConfig`:
- `pool_discovery_mode` — auto/preconfigured/both
- `preconfigured_pools` — static pool IDs per DEX
- `profit_tokens` — list of profit token configs (token, symbol, decimals, default_price_usd, min_profit, gecko_pool_address)
- `min_profit_usd` — USD threshold for opportunities
- `price_update_interval_secs` — GeckoTerminal refresh interval
- `event_timeout_ms` — timeout per event batch
- `search_strategy` — fast/normal/thorough

## Implementation Steps

1. ✅ Extend `StrategyConfig` with new fields
2. ✅ Update `config/mainnet.toml`
3. ✅ Create `arb-engine` crate skeleton
4. ✅ Implement `graph.rs` — ArbGraph::build
5. ✅ Implement `cycle.rs` — DFS cycle detection + CycleIndex
6. ✅ Implement `profit_token.rs` — ProfitTokenRegistry + GeckoTerminal
7. ✅ Implement `simulator.rs` — SimCache + simulate_cycle
8. ✅ Implement `search.rs` — golden-section search
9. ✅ Implement `opportunity.rs` + `lib.rs` — ArbEngine
10. ✅ Unit tests (18 tests across all modules)
11. ✅ Scaffold `bin/arb` event loop
12. ✅ Integration tests against mainnet

## Test Plan

### Unit Tests (18 passing)
- **graph**: Build from edges, verify adjacency/neighbors/counts
- **cycle**: Triangle detection (2 directions), max_hops limit, deduplication, profit token rotation, fallback, CycleIndex lookup, Cycle::rotate
- **profit_token**: to_usd/from_usd, min_profit_for_usd, lookup, best_profit_token, get_usd_value
- **simulator**: SimCache hit/miss, different pools
- **search**: SearchConfig from strategy variants

### Integration Tests (`#[ignore]`)
- Build ArbGraph from mainnet pools
- Find cycles with max_hops=3
- Simulate a known cycle
- Search optimal amount on a known cycle

## Verification Criteria

Phase 3 is complete when:
1. `cargo test -p arb-engine` — all unit tests pass
2. `cargo test --workspace` — all existing tests still pass
3. `cargo clippy --workspace` — no warnings
4. `bin/arb` builds and can discover pools + build engine against mainnet
