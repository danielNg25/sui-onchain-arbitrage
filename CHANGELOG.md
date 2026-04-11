# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- SwapEventData parsing from raw on-chain events for both Cetus and Turbos DEX crates
  - `dex_cetus::events::parse_swap_event_data()` — extracts all fields from Cetus SwapEvent JSON
  - `dex_turbos::events::parse_swap_event_data()` — extracts fields from Turbos SwapEvent JSON, derives amount_in/out from direction
  - Unit tests for both parsers with known JSON fixtures
- Event polling loop in `bin/arb`:
  - Polls ALL 6 event types (swap + liquidity) to keep pool state in sync
  - Applies events via `pool_manager.apply_event()` — zero RPC in the hot path
  - Triggers `engine.process_event()` on swap events to detect arbitrage opportunities
  - Logs detected opportunities with profit, USD value, and trigger pool
- `arb-engine` crate: strategy engine for finding arbitrage opportunities
  - `graph` module: token adjacency graph built from all discovered pools
  - `cycle` module: DFS-based cycle detection with deduplication, profit token rotation, and pool-indexed lookup (O(1) per event)
  - `profit_token` module: registry with GeckoTerminal price fetching, USD conversion, priority-based profit token selection
  - `simulator` module: event-scoped simulation cache (DashMap), multi-leg cycle simulation via `Pool::estimate_swap`
  - `search` module: two-phase golden-section search (strategic sampling + refinement) for optimal input amount
  - `opportunity` module: ranked opportunity output with USD-denominated profit
  - 18 unit tests covering graph construction, cycle detection, profit token math, simulation caching, search config
  - 4 integration tests (ignored): mainnet graph build, cycle detection, engine initialization, cycle simulation
- `bin/arb`: main binary scaffold with pool discovery, tick loading, engine initialization, and cycle breakdown logging
- Extended `StrategyConfig` with Phase 3 fields: `pool_discovery_mode`, `preconfigured_pools`, `profit_tokens`, `min_profit_usd`, `price_update_interval_secs`, `event_timeout_ms`, `search_strategy`
- New config types: `PoolDiscoveryMode`, `PreconfiguredPools`, `ProfitTokenConfig`, `SearchStrategy`
- `docs/plans/phase3-strategy.md`: detailed implementation plan

### Changed
- Updated `config/mainnet.toml` with profit token definitions (SUI, USDC) and strategy parameters

- `clmm-math` crate: pure Rust CLMM math library ported from CetusProtocol/cetus-clmm-interface Move contracts
  - `tick_math` module: `tick_to_sqrt_price` and `sqrt_price_to_tick` using binary exponentiation with 19 precomputed ratio constants (Q64.64/Q96.96)
  - `swap_math` module: `compute_swap_step`, `get_amount_a_delta`, `get_amount_b_delta`, `get_next_sqrt_price_from_input/output`
  - `simulate` module: full multi-tick `simulate_swap` loop with tick crossing and liquidity updates
  - `math_u256` module: u256 helpers (`mul_div_floor`, `mul_div_ceil`, `checked_shlw`, `div_round`) via `ethnum`
  - 37 unit tests covering tick conversions, swap steps, multi-tick simulation, edge cases
- Wired `clmm-math` into `dex-cetus` and `dex-turbos` `estimate_swap()` implementations (replaces Phase 2 placeholder stubs)
- Mainnet verification tests (`swap_verification.rs`): local `simulate_swap` matches `devInspectTransactionBlock` with 0 diff for both Cetus (6 tests: 0.01/1/10 SUI a2b + 0.01/1/10 USDC b2a) and Turbos (4 tests: 0.01/1 SUI a2b + 0.01/1 USDC b2a)
- BCS transaction builder for devInspect: encodes `calculate_swap_result` (Cetus) and `compute_swap_result` (Turbos) calls

### Changed
- Turbos tick deserialization now computes `sqrt_price` via `clmm_math::tick_to_sqrt_price(tick_index)` instead of hardcoded `0`

### Previously added
- `arb-types` crate: shared types (Tick, CoinType, ObjectId, Dex, SwapEventData, SwapEstimate, AppConfig), hex helpers, config loading from TOML
- `sui-client` crate: thin JSON-RPC wrapper using reqwest (get_object, multi_get_objects, get_dynamic_fields, query_events, dev_inspect, execute_tx, checkpoint queries)
- `dex-common` crate: `DexRegistry` and `Pool` traits for DEX-agnostic pool management (supports CLMM, V2 AMM, orderbook), type string parsing
- `dex-cetus` crate: `CetusRegistry` + `CetusPool` implementing unified traits, manual BCS deserialization, SkipList tick fetching, event-based pool discovery, full event application (SwapEvent + AddLiquidityEvent + RemoveLiquidityEvent with tick/liquidity updates)
- `dex-turbos` crate: `TurbosRegistry` + `TurbosPool` implementing unified traits, BCS deserialization, tick fetching from pool dynamic fields, full event application (SwapEvent + MintEvent + BurnEvent with tick/liquidity updates)
- `pool-manager` crate: thin router over `DexRegistry` trait objects, global pair/token indexes, atomic checkpoint snapshots, event routing
- `config/mainnet.toml`: all Cetus/Turbos/Shio package IDs, shared objects, gas/strategy config
- Workspace Cargo.toml with shared dependency versions
- `docs/plans/phase1-foundation.md`: detailed implementation plan
- Integration tests (ignored): Cetus pool fetch + BCS deserialization, tick fetching (548 ticks verified), checkpoint queries
- Project scaffolding: workspace structure, git init, `.gitignore`
- `README.md` with architecture overview, tech stack, and implementation progress checklist
- `CLAUDE.md` with git workflow rules, commit conventions, and code guidelines
- `CHANGELOG.md` (this file)
- `.github/pull_request_template.md` for PR workflow
- `docs/agent/ARCHITECTURE.md` — full technical design document
- `docs/agent/RESEARCH.md` — Cetus/Turbos/Sui protocol research
- `docs/agent/RESEARCH_2.md` — supplementary research findings

### Changed
- Reorganized DEX crates into `crates/dex/` subfolder (`dex/common`, `dex/cetus`, `dex/turbos`)
- Added `dex-common` crate for shared `DexCommands` trait
- Updated architecture diagram, dependency DAG, and implementation checklist to reflect new structure
- Added testing requirements to commit rules: `cargo test --workspace` and `cargo clippy --workspace` must pass before every commit
- Expanded Testing section in `CLAUDE.md` with test categories, what to test per crate, and test data strategy
- Updated PR template with testing checklist items (`cargo test`, `cargo clippy`, unit/integration test checkboxes)
- Swapped Phase 3 (Strategy) and Phase 4 (Execution) — find opportunities before building PTBs
- Changed arb engine to event/tx-driven only — no periodic scanning
- Replaced golden section search with binary search over `[0, swap_amount]` range from triggering event
- Added mandatory phase planning requirement to `CLAUDE.md` — must create `docs/plans/phase<N>.md` and get approval before writing any code
