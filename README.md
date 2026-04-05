# Sui Arbitrage Bot

High-performance Rust arbitrage bot for Sui DEX pools, targeting **Cetus** and **Turbos** concentrated liquidity market makers (CLMMs).

## Architecture Overview

```
                         ┌─────────────────────────────┐
                         │         bin/arb              │
                         │       (Event Loop)           │
                         └──────┬──────┬──────┬─────────┘
                                │      │      │
              ┌─────────────────┤      │      ├─────────────────┐
              │                 │      │      │                 │
       ┌──────▼──────┐  ┌──────▼──────▼──┐  ┌▼──────────┐  ┌──▼──────────┐
       │ shio-client  │  │  arb-engine    │  │ptb-builder │  │ gas-manager │
       │ (MEV Feed)   │  │ (Strategy)     │  │ (TX Build) │  │ (Gas Coins) │
       └──────────────┘  └───────┬────────┘  └──────┬─────┘  └─────────────┘
                                 │                  │
                          ┌──────▼──────┐    ┌──────▼──────┐
                          │ pool-manager │    │ dex/common  │
                          │ (State)      │    │ (DexTrait)  │
                          └──────┬───────┘    └──┬──────┬───┘
                                 │            ┌──▼──┐ ┌─▼─────┐
                                 │            │dex/ │ │dex/   │
                                 │            │cetus│ │turbos │
                                 │            └──┬──┘ └───┬───┘
                                 │               │        │
                          ┌──────▼───────────────▼────────▼───┐
                          │            sui-client              │
                          │     (RPC, Sign, Submit, Fetch)     │
                          └───────────────────────────────────┘
                                         │
                          ┌──────────────┼──────────────┐
                          │              │              │
                     clmm-math      arb-types      config/
                   (Pure Math)    (Shared Types)   (TOML)
```

### Core Design Principles

- **Local CLMM math** — simulate swaps in microseconds, not via on-chain devInspect
- **Cetus as flash loan source** — zero-capital atomic arbitrage via hot-potato receipt pattern
- **Configurable addresses** — all package IDs in TOML, never hardcoded (they change with upgrades)
- **Golden section search** — find optimal swap amounts in ~50 iterations of local math
- **Multi-source events** — Shio feed (sub-300ms) + event polling + periodic refresh

## Tech Stack

| Component | Technology |
|-----------|-----------|
| Language | Rust (2021 edition) |
| Async Runtime | Tokio |
| Sui SDK | `sui-sdk` (git dep) or modular crates (`sui-sdk-types`, `sui-transaction-builder`) |
| Serialization | BCS (on-chain), TOML (config), serde_json |
| Concurrency | DashMap, tokio::sync::mpsc |
| MEV | Shio protocol (custom WebSocket + JSON-RPC client) |
| Math | Q64.64 fixed-point, ported from Cetus open-source Move contracts |

## Implementation Progress

### Phase 1: Foundation (data flowing)
- [x] `arb-types` — shared types crate
- [x] `sui-client` — RPC wrapper (object fetch, dry run, submit)
- [x] `dex/common` — DexCommands trait, shared DEX types
- [x] `dex/cetus` — BCS deserialization of Cetus pools + ticks
- [ ] `dex/turbos` — BCS deserialization of Turbos pools + ticks
- [x] `pool-manager` — pool discovery + initial state loading
- [x] Verify: fetch real SUI/USDC pool from mainnet, deserialize, print state

### Phase 2: Math (simulate locally)
- [ ] `clmm-math` — port tick math + compute_swap_step from Cetus sources
- [ ] Verify: compare local simulate_swap output against devInspectTransactionBlock

### Phase 3: Strategy (find opportunities)
- [ ] `arb-engine` — graph construction, cycle finding, binary search amount optimization
- [ ] `bin/arb` — event-driven loop (swap event → path lookup → simulate → evaluate)
- [ ] Verify: feed historical swap events, log detected opportunities with expected profit

### Phase 4: Execution (build and submit)
- [ ] `ptb-builder` — Cetus flash swap + Turbos swap commands
- [ ] `gas-manager` — coin splitting + acquisition
- [ ] Verify: build a real 2-hop PTB, dry-run on mainnet

### Phase 5: Shio (competitive execution)
- [ ] `shio-client` — WebSocket feed + bid submission
- [ ] Backrun integration in event loop
- [ ] Verify: connect to Shio feed, log auction events

### Phase 6: Production hardening
- [ ] Metrics, logging, alerting
- [ ] Reconnection logic for all WebSocket/gRPC streams
- [ ] Config hot-reload for published_at addresses

## Getting Started

### Prerequisites

- Rust 1.82+ (for modular SDK compatibility)
- Sui CLI (optional, for manual testing)
- GitHub CLI (`gh`) for PR workflow

### Setup

```bash
# Clone
git clone <repo-url>
cd sui-arbitrage-bot

# Configure
cp config/mainnet.toml.example config/mainnet.toml
# Edit config/mainnet.toml with current published_at addresses

# Build
cargo build --release

# Run (dry-run mode)
cargo run --release --bin arb -- --config config/mainnet.toml --dry-run
```

### Configuration

All on-chain addresses live in `config/mainnet.toml`. The `published_at` addresses for Cetus and Turbos change with every package upgrade — verify on-chain before deployment.

## Documentation

- [Architecture](docs/agent/ARCHITECTURE.md) — full technical design, crate structure, data types, implementation phases
- [Research: Cetus, Turbos, Sui Mechanics](docs/agent/RESEARCH.md) — protocol details, package IDs, swap functions, events
- [Research: Additional Findings](docs/agent/RESEARCH_2.md) — supplementary research with PTB examples and deserialization patterns

## License

Private — not for distribution.
