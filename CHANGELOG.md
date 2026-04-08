# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
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
