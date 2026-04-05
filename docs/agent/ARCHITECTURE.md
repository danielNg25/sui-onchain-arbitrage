# Sui DEX Arbitrage Bot — Architecture & Implementation Plan

## Context

Building a Rust arbitrage bot on Sui targeting Cetus and Turbos CLMMs. Two research documents (RESEARCH.md, RESEARCH_2.md) provide the technical foundation. The developer is experienced with EVM arbitrage. Key insight from research: `fuzzland/sui-mev` is reference-only (no local swap math, requires patched node). We build from scratch with local CLMM math as the core competitive advantage.

**Critical finding reconciled across both research docs:** Turbos flash_swap existence is disputed. RESEARCH.md says it exists in the pool module, RESEARCH_2.md says it's not publicly exposed. **Safe approach: always use Cetus as the flash loan source.** If Turbos flash_swap works, we can add it later.

**`published_at` addresses differ between docs** (Cetus: `0xc6faf3...` vs `0x25ebb9...`) — confirms these change with upgrades. All addresses must be configurable.

---

## Workspace Structure

```
sui-arbitrage-bot/
├── Cargo.toml                     # Workspace root
├── config/
│   ├── mainnet.toml               # All package IDs, object IDs, strategy params
│   └── testnet.toml
├── crates/
│   ├── arb-types/                 # Shared types, zero I/O deps
│   ├── clmm-math/                 # Pure CLMM math (#[no_std]-compatible)
│   ├── pool-manager/              # Pool registry, state cache, tick storage
│   ├── dex-cetus/                 # Cetus BCS deser, PTB commands, event parsing
│   ├── dex-turbos/                # Turbos BCS deser, PTB commands, event parsing
│   ├── arb-engine/                # Graph, cycle detection, profit sim, amount optimization
│   ├── ptb-builder/               # Multi-hop PTB orchestration, flash swap flow
│   ├── shio-client/               # WebSocket feed + bid submission
│   ├── gas-manager/               # Pre-split coin pool, RGP multiplier logic
│   └── sui-client/                # Thin SDK wrapper: fetch, sign, submit, dry run
├── bin/
│   └── arb/src/main.rs            # Event loop, wires everything together
└── tests/
    └── integration/               # Mainnet fork tests via devInspect
```

### Dependency DAG
```
bin/arb
  ├── arb-engine → clmm-math, arb-types, pool-manager
  ├── ptb-builder → dex-cetus, dex-turbos, arb-types
  ├── shio-client → arb-types
  ├── gas-manager → sui-client
  ├── pool-manager → dex-cetus, dex-turbos, clmm-math, arb-types, sui-client
  └── sui-client → arb-types
```

`clmm-math` and `arb-types` have zero async/IO deps — pure computation, independently benchmarkable.

---

## Core Data Types (`arb-types`)

### Unified Pool
```rust
pub struct PoolState {
    pub id: ObjectId,
    pub dex: Dex,                    // Cetus | Turbos
    pub coin_a: CoinType,            // Arc<str> full Move type string
    pub coin_b: CoinType,
    pub sqrt_price: u128,            // Q64.64 — same format both DEXes
    pub tick_current: i32,
    pub liquidity: u128,
    pub fee_rate: u64,               // PPM, denominator 1_000_000
    pub tick_spacing: u32,
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub is_active: bool,
    pub ticks_table_id: ObjectId,
    pub metadata: DexPoolMetadata,
    pub object_version: u64,
}

pub enum DexPoolMetadata {
    Cetus { initial_shared_version: u64 },
    Turbos { fee_type: CoinType, initial_shared_version: u64 },
}
```

### Tick (normalized — same for both DEXes once fetched)
```rust
pub struct Tick {
    pub index: i32,
    pub liquidity_net: i128,
    pub liquidity_gross: u128,
    pub sqrt_price: u128,            // precomputed Q64.64
}
```

### Opportunity
```rust
pub struct Opportunity {
    pub legs: Vec<SwapLeg>,          // 2-3 hops forming a cycle
    pub start_token: CoinType,
    pub amount_in: u64,
    pub expected_profit: i64,
    pub gas_estimate: u64,
    pub trigger: OpportunityTrigger,
}

pub enum OpportunityTrigger {
    PeriodicScan,
    SwapEvent { tx_digest: [u8; 32] },
    ShioBackrun { opp_tx_digest: [u8; 32], gas_price: u64, deadline_ms: u64 },
}
```

---

## CLMM Math (`clmm-math`)

Both DEXes share identical math. Port from open-source Cetus contracts (`CetusProtocol/cetus-contracts`, `math/clmm_math.move`, `math/tick_math.move`) and `CetusProtocol/integer-mate` for I32/I128.

### Core functions
- `tick_to_sqrt_price(tick: i32) -> u128` — precomputed ratio binary exponentiation
- `sqrt_price_to_tick(sqrt_price: u128) -> i32` — log2 with BIT_PRECISION=14
- `compute_swap_step(sqrt_price_current, sqrt_price_target, liquidity, amount_remaining, fee_rate, a2b, by_amount_in) -> SwapStepResult` — innermost hot loop, must be sub-microsecond
- `simulate_swap(sqrt_price, liquidity, ticks: &[Tick], a2b, amount, fee_rate) -> SwapResult` — full multi-tick traversal
- Q64.64 helpers: `mul_div_u128`, `get_amount_a_delta`, `get_amount_b_delta`, `get_next_sqrt_price_from_amount`

### Constants
```rust
pub const FEE_RATE_DENOMINATOR: u64 = 1_000_000;
pub const MIN_SQRT_PRICE: u128 = 4_295_048_016;
pub const MAX_SQRT_PRICE: u128 = 79_226_673_515_401_279_992_447_579_055;
pub const MIN_TICK: i32 = -443_636;
pub const MAX_TICK: i32 = 443_636;
```

### Tick traversal
Once ticks are fetched (from Cetus SkipList or Turbos bitmap), normalize to sorted `Vec<Tick>`. Binary search for next initialized tick in direction of travel. O(log n) per step — sub-microsecond for typical pools (<1000 initialized ticks).

---

## Pool State Manager (`pool-manager`)

```rust
pub struct PoolManager {
    pools: DashMap<ObjectId, Arc<PoolState>>,
    tick_cache: DashMap<ObjectId, Arc<Vec<Tick>>>,
    token_to_pools: DashMap<CoinType, HashSet<ObjectId>>,
    pair_to_pools: DashMap<(CoinType, CoinType), HashSet<ObjectId>>,
}
```

### Discovery (startup)
1. Paginate Cetus `Pools` LinkedTable (`0xf699e7...`) via `getDynamicFields`
2. Paginate Turbos `PoolTableId` (`0x08984e...`) via `getDynamicFields`
3. Batch-fetch full pool objects with `multi_get_objects`
4. BCS-deserialize via `dex-cetus` / `dex-turbos`
5. Filter: only whitelisted tokens, not paused/locked
6. Fetch all initialized ticks per pool (paginated dynamic field reads)

### Three Update Paths (fastest → slowest)
1. **Shio feed mutations** (sub-300ms): `sideEffects.mutatedObjects` contain full post-tx pool state → parse and replace directly
2. **Swap events** (2-5s via polling/gRPC): Update `sqrt_price`, reserves from event fields. If `steps > 1`, mark pool for tick refresh (liquidity changed from tick crossings)
3. **Periodic full refresh** (10-15s): Re-fetch all pool objects + ticks for pools with pending liquidity changes

---

## Arbitrage Engine (`arb-engine`)

### Graph
```rust
pub struct ArbGraph {
    adjacency: HashMap<CoinType, Vec<PoolEdge>>,
}

pub struct PoolEdge {
    pub pool_id: ObjectId,
    pub dex: Dex,
    pub token_out: CoinType,
    pub a2b: bool,
}
```

### Cycle finding
BFS from each whitelisted start token (SUI, USDC), max 2-3 hops. **Constraint: first leg must be a Cetus pool** (flash loan source). Generates candidate paths.

### Optimal amount: Golden Section Search
Profit is unimodal (concave) for CLMM arbs — rises to peak, falls at deeper liquidity. Golden section converges in ~50 iterations, each calling `simulate_path` (microseconds with local math). Replaces fuzzland/sui-mev's 10-point grid search.

### Validation
`devInspect` used only for:
- Initial tuning (validate local math against on-chain)
- Turbos pools specifically (closed source — rounding may differ)
- Final pre-submission sanity check on high-value opportunities

---

## PTB Builder (`ptb-builder`)

### DEX trait
```rust
pub trait DexCommands {
    fn build_flash_swap(&self, ptb: &mut PTB, pool: &PoolState, a2b: bool, amount: u64) -> Result<FlashSwapResult>;
    fn build_repay_flash_swap(&self, ptb: &mut PTB, pool: &PoolState, balance_a: Arg, balance_b: Arg, receipt: Arg) -> Result<()>;
    fn build_swap(&self, ptb: &mut PTB, pool: &PoolState, a2b: bool, coin_in: Arg, amount: u64) -> Result<Arg>;
}
```

### Multi-hop flash swap flow (e.g., Cetus → Turbos → Cetus)
```
Cmd 0: Cetus flash_swap(pool_1, a2b=true, amount)
        → NestedResult(0,0)=Balance<A>, NestedResult(0,1)=Balance<B>, NestedResult(0,2)=Receipt
Cmd 1: coin::from_balance<B>(NestedResult(0,1))    → Coin<B> for Turbos
Cmd 2: Turbos swap_a_b_with_return_(pool_2, MakeMoveVec([Result(1)]), ...)
        → NestedResult(2,0)=Coin<Out>, NestedResult(2,1)=Coin<Remainder>
Cmd 3: Cetus flash_swap(pool_3, ...) or regular swap for final leg
Cmd 4: Repay flash swap receipts
Cmd 5: TransferObjects(profit, sender)
```

### Shio bid append
For backrun opportunities, append after profit commands:
```
Cmd N:   SplitCoins(GasCoin, [bid_amount])
Cmd N+1: coin::into_balance<SUI>(Result(N))
Cmd N+2: shio::auctioneer::submit_bid(random_global_state, bid_amount, Result(N+1))
```

---

## Execution Pipeline

### Event Sources → `mpsc::channel<ArbTrigger>`
1. **Shio feed** (`shio-client`): Primary. WebSocket to `wss://rpc.getshio.com/feed`. Parses `auctionStarted`, extracts mutated pools. Sub-300ms window.
2. **Event polling** (`sui-client`): `suix_queryEvents` with cursor. Filters Cetus + Turbos SwapEvents. 3-7s latency.
3. **Periodic timer**: Every 10s, full state refresh + evaluation.

### Flow per trigger
```
Event arrives
  → pool-manager.apply_update(event)
  → arb-engine.evaluate(affected_pools) → Vec<Opportunity>
  → if profitable: ptb-builder.build(best_opportunity) → TransactionData
  → submission:
      ShioBackrun → match gas price, append bid, submit via shio-client
      Direct      → gas-manager.acquire_gas_coin(), set gas = N * RGP, execute_transaction_block(WaitForEffects)
```

### Retry
- Contention (`ExecutionCancelledDueToSharedObjectCongestion`): Retry once with 2x gas multiplier + fresh state
- Shio bids: Never retry (deadline passed)
- Equivocation: No retry — investigate gas coin management

---

## Gas Manager (`gas-manager`)

```rust
pub struct GasManager {
    available_coins: Mutex<Vec<GasCoin>>,  // pre-split, ready to use
    in_flight: DashMap<ObjectId, Instant>,  // coins currently in use
}
```

- On startup: split SUI into N gas coins (e.g., 10 × 1 SUI each)
- `acquire()`: pop from available, mark in-flight
- `release(coin)`: return to available pool
- If a coin is equivocated (frozen): remove from pool, log warning
- SIP-45 gas pricing: `gas_price = config.rgp_multiplier * reference_gas_price`

---

## Configuration (`config/mainnet.toml`)

```toml
[network]
rpc_url = "https://fullnode.mainnet.sui.io:443"
grpc_url = "https://your-grpc-provider:443"

[cetus]
package_types = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb"
package_published_at = "VERIFY_ON_CHAIN_BEFORE_DEPLOY"   # changes with upgrades!
global_config = "0xdaa46292632c3c4d8f31f23ea0f9b36a28ff3677e9684980e4438403a67a3d8f"
pools_registry = "0xf699e7f2276f5c9a75944b37a0c5b5d9ddfd2471bf6242483b03ab2887d198d0"

[turbos]
package_types = "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1"
package_published_at = "VERIFY_ON_CHAIN_BEFORE_DEPLOY"
swap_router_package = "0xd02012c71c1a6a221e540c36c37c81e0224907fe1ee05bfe250025654ff17103"
versioned = "0xf1cf0e81048df168ebeb1b8030fad24b3e0b53ae827c25053fff0779c1445b6f"
pool_table_id = "0x08984ed8705f44b6403705dc248896e56ab7961447820ae29be935ce0d32198b"

[shio]
feed_url = "wss://rpc.getshio.com/feed"
rpc_url = "https://rpc.getshio.com"
auctioneer_package = "0x1889977f0fb56ae730e7bda8e8e32859ce78874458c74910d36121a81a615123"
bid_percentage = 90   # % of profit to bid

[gas]
budget = 50_000_000   # 0.05 SUI
rgp_multiplier_normal = 5
rgp_multiplier_high = 100
pre_split_count = 10
pre_split_amount = 1_000_000_000   # 1 SUI each

[strategy]
max_hops = 3
min_profit_mist = 1_000_000   # 0.001 SUI
gss_iterations = 50
poll_interval_ms = 3000
refresh_interval_ms = 10000
whitelisted_tokens = [
    "0x2::sui::SUI",
    "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC",
]
```

---

## Implementation Order

### Phase 1: Foundation (get data flowing)
1. `arb-types` — all shared types
2. `sui-client` — RPC wrapper (object fetch, dry run, submit)
3. `dex-cetus` — BCS deserialization of Cetus pools + ticks
4. `dex-turbos` — BCS deserialization of Turbos pools + ticks
5. `pool-manager` — discovery + initial state loading
6. **Verify**: Fetch a real SUI/USDC pool from mainnet, deserialize, print state

### Phase 2: Math (simulate locally)
7. `clmm-math` — port tick math + compute_swap_step from Cetus sources
8. **Verify**: Compare local `simulate_swap` output against `devInspectTransactionBlock` for same pool + amount

### Phase 3: Execution (build and submit)
9. `ptb-builder` — Cetus flash swap + Turbos swap commands
10. `gas-manager` — coin splitting + acquisition
11. **Verify**: Build a real 2-hop PTB, dry-run it on mainnet

### Phase 4: Strategy (find opportunities)
12. `arb-engine` — graph construction, cycle finding, golden section search
13. `bin/arb` — event loop wiring, periodic scan
14. **Verify**: Run in dry-run-only mode, log detected opportunities

### Phase 5: Shio (competitive execution)
15. `shio-client` — WebSocket feed + bid submission
16. Backrun integration in event loop
17. **Verify**: Connect to Shio feed, log auction events

### Phase 6: Production hardening
18. Metrics, logging, alerting
19. Reconnection logic for all WebSocket/gRPC streams
20. Config hot-reload for published_at addresses
