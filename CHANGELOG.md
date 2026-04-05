# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- `arb-types` crate: shared types (PoolState, Tick, CoinType, SwapEventData, AppConfig), hex helpers, config loading from TOML
- `sui-client` crate: thin JSON-RPC wrapper using reqwest (get_object, multi_get_objects, get_dynamic_fields, query_events, dev_inspect, execute_tx, checkpoint queries)
- `dex-common` crate: PoolDeserializer and TickFetcher traits, SwapEventParser trait, type string parsing for Cetus 2-param and Turbos 3-param pools
- `dex-cetus` crate: manual BCS deserialization of Cetus Pool objects, SkipList tick node deserialization, swap event parsing
- `dex-turbos` crate: BCS deserialization structs for Turbos Pool objects, tick fetching, swap event parsing
- `pool-manager` crate: pool discovery from Cetus/Turbos registries, DashMap-based pool cache with token/pair indexes, atomic checkpoint snapshots for event sync, pool state updates from swap events
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
