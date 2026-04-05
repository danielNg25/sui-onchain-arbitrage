# Sui DEX Arbitrage Bot: Complete Technical Reference

**Cetus and Turbos are both Uniswap V3-style CLMMs on Sui, but their implementations diverge in critical ways that directly affect bot architecture.** Cetus exposes `flash_swap` with a hot-potato receipt pattern ideal for zero-capital atomic arbitrage; Turbos does not expose a public flash swap, forcing you to source initial capital from Cetus or DeepBook flash loans. Both use Q64.64 sqrt price representation and share identical tick range limits (±443,636), but Turbos encodes fee tiers as phantom type parameters (3 generic args per pool) while Cetus stores fees as numeric fields (2 generic args). The `fuzzland/sui-mev` repository provides strong architectural patterns but should be used as reference only — it's a single-commit code dump with broken BCS deserialization, zero documentation, and a hard dependency on a custom Sui validator fork.

---

## 1. fuzzland/sui-mev: architecture worth studying, code not worth forking

### Project structure

The repository uses a Rust workspace with **10 crates** organized into binaries and libraries:

```
bin/arb           — Main arbitrage bot (entry: cargo run -r --bin arb start-bot -- --private-key {})
bin/relay         — gRPC relay server for mempool transaction forwarding from validator
crates/arb-common — Path finding, profit calculation, strategy logic
crates/dex-indexer — Pool discovery, state fetching, BCS deserialization
crates/simulator  — Off-chain swap math for all supported DEXes (includes DBSimulator)
crates/object-pool — Sui object version tracking and staleness management
crates/shio       — Shio MEV auction/bundle submission
crates/logger     — Structured logging (published as mev_logger)
crates/utils      — General utilities
crates/version    — Version management
```

The bot supports **10 DEXes**: Cetus, Turbos, DeepBook, Aftermath, FlowX, BlueMove, Kriya, Abex, Navi, and Shio. Languages: Rust 98.4%, Python 1.3%, Shell 0.3%.

### Critical dependency: custom Sui validator fork

All Sui crates come from `suiflow/mevsui` (branch `relay-patch`) — a patched Sui fork that exposes mempool data. The relay binary receives pending transactions via gRPC (`tonic 0.12`) from this custom validator and forwards them to the arb bot through a Unix domain socket (`/tmp/sui_tx.sock`, confirmed by issue #1). A commented-out branch `monitor-pool-related-objects` suggests an alternative mode for tracking pool object mutations directly.

### Event-driven pipeline via burberry

The bot uses `burberry` (by `tonyke-bot`, pinned to rev `8bdb3ca`) — an event processing framework analogous to Paradigm's Artemis — which structures the pipeline as **collectors → strategies → executors**. Key runtime dependencies include `dashmap v6.0` for concurrent pool state caching, `tokio-tungstenite 0.24` for WebSocket event streaming, and `bcs 0.1.6` for on-chain object deserialization.

### End-to-end execution flow (inferred from architecture)

```
Sui Validator (mevsui fork) ──gRPC──▶ bin/relay ──unix socket──▶ bin/arb
                                                                    │
        ┌───────────────────────────────────────────────────────────┤
        │                    │                    │                  │
  dex-indexer          simulator           arb-common             shio
  (pool state)      (swap math)        (path finding)      (bundle submit)
        │                                       │
  object-pool                              PTB build
  (version mgmt)                          + sign + submit
```

The `simulator` crate implements **pure-Rust off-chain swap math** (not on-chain devInspect) for speed. The `DBSimulator` variant (issue #6) requires a local Sui node database for full transaction simulation — significantly more accurate but infrastructure-heavy.

### Repository health: abandoned after open-sourcing

**754 stars, 504 forks, exactly 1 commit** (single code dump around April 2025). Zero closed issues, zero PRs, 9 open issues all unanswered. The `scripts/restart_bot.py` restarts the bot every 3 hours — a workaround for fundamental stability issues rather than a fix. Issue #4 reveals **BCS deserialization already broken** by Sui protocol upgrades (`serialization error: integer 9, expected 0 <= i < 8`). Issue #3 reports a user's funds being stolen. No license specified.

### Verdict: reference only, do not fork

The workspace architecture is well-structured and the patterns (object-pool, dex-indexer separation, burberry pipeline) are worth studying. However, forking is inadvisable for five reasons: (1) the `suiflow/mevsui` dependency is fragile and tied to a specific Sui protocol version that's already outdated; (2) single-commit dump means no commit history to understand design reasoning; (3) BCS deserialization is already broken; (4) no tests, no docs, no error handling patterns visible; (5) no license means legal risk. **Study the architecture, build from scratch.**

---

## 2. Cetus Protocol: full source access makes it the easier integration target

### Mainnet package IDs and shared objects

| Component                                            | Address                                                              |
| ---------------------------------------------------- | -------------------------------------------------------------------- |
| **CLMM package (original, for type resolution)**     | `0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb` |
| **CLMM package (publishedAt, for MoveCall targets)** | `0x25ebb9a7c50eb17b3fa9c5a30fb8b5ad8f97caaf4928943acbcff7153dfee5e3` |
| **Integrate package**                                | `0x996c4d9480708fb8b92aa7acf819fb0497b5ec8e65ba06601cae2fb6db3312c3` |
| **GlobalConfig (shared object)**                     | `0xdaa46292632c3c4d8f31f23ea0f9b36a28ff3677e9684980e4438403a67a3d8f` |
| **Pools registry (shared object)**                   | `0xf699e7f2276f5c9a75944b37a0c5b5d9ddfd2471bf6242483b03ab2887d198d0` |
| **Clock**                                            | `0x6`                                                                |

The original package ID is used in type strings (e.g., `0x1eabed...::pool::Pool<A, B>`). The `publishedAt` address is where the latest bytecode lives — **use this for MoveCall targets**.

### Pool object structure

```move
struct Pool<phantom CoinTypeA, phantom CoinTypeB> has key, store {
    id: UID,
    coin_a: Balance<CoinTypeA>,       // Token A reserves
    coin_b: Balance<CoinTypeB>,       // Token B reserves
    tick_spacing: u32,                 // Determines fee tier
    fee_rate: u64,                     // Numerator; denominator is 1_000_000
    liquidity: u128,                   // Current active liquidity
    current_sqrt_price: u128,          // Q64.64 fixed-point sqrt(price)
    current_tick_index: I32,           // Signed tick index (bits: u32)
    fee_growth_global_a: u128,         // Q64.64
    fee_growth_global_b: u128,         // Q64.64
    fee_protocol_coin_a: u64,
    fee_protocol_coin_b: u64,
    tick_manager: TickManager,         // Contains Table of tick data (dynamic fields)
    rewarder_manager: RewarderManager,
    position_manager: PositionManager,
    is_pause: bool,
    index: u64,
    url: String,
}
```

Example pool: SUI/USDC at `0xcf994611fd4c48e277ce3ffd4d4364c914af2c3cbb05f7bf6facd371de688630`.

### Tick data storage

Ticks live inside `TickManager`, which wraps a Sui **Table** (dynamic field-based key-value store). Each initialized tick is a dynamic field keyed by `I32` (tick index). The `ticks_handle` table ID is extracted from the deserialized pool object, then queried via `sui_getDynamicFields` for enumeration.

The contract exposes `fetch_ticks` for bulk loading via `devInspectTransactionBlock`:

```move
public fun fetch_ticks<A, B>(pool: &Pool<A, B>, start: vector<u32>, limit: u64): vector<Tick>
```

For ongoing maintenance, subscribe to `AddLiquidityEvent` and `RemoveLiquidityEvent` to track tick mutations incrementally. Ticks are sparse — only initialized ticks exist as dynamic fields.

### Fee tiers

| tick_spacing | fee_rate | Effective fee   |
| ------------ | -------- | --------------- |
| 2            | 100      | 0.01% (1 bps)   |
| 10           | 500      | 0.05% (5 bps)   |
| 20           | 1,000    | 0.10% (10 bps)  |
| 60           | 2,500    | 0.25% (25 bps)  |
| 200          | 10,000   | 1.00% (100 bps) |
| 220          | 20,000   | 2.00% (200 bps) |

Fee is stored per-pool in `fee_rate`. Denominator is always **1,000,000**.

### Swap mechanics: flash_swap is the key primitive

**The function you want for arbitrage:**

```move
public fun flash_swap<CoinTypeA, CoinTypeB>(
    config: &GlobalConfig,                        // Shared, immutable borrow
    pool: &mut Pool<CoinTypeA, CoinTypeB>,       // Shared, mutable borrow
    a2b: bool,                                    // true = A→B, false = B→A
    by_amount_in: bool,                           // true = exact input, false = exact output
    amount: u64,                                  // Swap amount
    sqrt_price_limit: u128,                       // Q64.64 price boundary
    clock: &Clock,                                // 0x6
): (Balance<CoinTypeA>, Balance<CoinTypeB>, FlashSwapReceipt<CoinTypeA, CoinTypeB>)
```

**Must repay in the same PTB:**

```move
public fun repay_flash_swap<CoinTypeA, CoinTypeB>(
    config: &GlobalConfig,
    pool: &mut Pool<CoinTypeA, CoinTypeB>,
    coin_a: Balance<CoinTypeA>,
    coin_b: Balance<CoinTypeB>,
    receipt: FlashSwapReceipt<CoinTypeA, CoinTypeB>,
)
```

The `FlashSwapReceipt` is a **hot potato** — it cannot be copied, stored, or dropped. It must be consumed by `repay_flash_swap` within the same transaction, or the PTB aborts. This is ideal for atomic multi-hop arbitrage with zero upfront capital.

Direction is a **boolean flag** (`a2b`), not separate functions. Sqrt price limits: **min = `4295048016`** (for a2b), **max = `79226673515401279992447579055`** (for b2a). Pass the extreme value to allow maximum price movement.

The read-only `calculate_swap_result` can be called via `devInspectTransactionBlock` for off-chain quoting without executing a real transaction.

### Sqrt price: Q64.64 fixed-point

`current_sqrt_price` is a `u128` where the lower 64 bits represent the fractional part. To get actual price: `price = (sqrt_price / 2^64)^2`. Valid tick range: **-443,636 to +443,636**.

### Swap events

**Event type:** `0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::pool::SwapEvent`

Fields: `atob` (bool), `pool` (address), `partner` (address), `amount_in` (u64), `amount_out` (u64), `ref_amount` (u64), `fee_amount` (u64), `vault_a_amount` (u64), `vault_b_amount` (u64), `before_sqrt_price` (u128), `after_sqrt_price` (u128), `steps` (u64).

The event provides `after_sqrt_price` to update local state, but **does not include `current_tick_index`** — you must derive it from the sqrt price. It also doesn't include `liquidity` changes from tick crossings. For precise local simulation, also track `AddLiquidityEvent` and `RemoveLiquidityEvent`.

### PTB MoveCall structure

```rust
// Step 1: flash_swap
MoveCall {
    package: "0x25ebb9a7c50eb17b3fa9c5a30fb8b5ad8f97caaf4928943acbcff7153dfee5e3",
    module: "pool",
    function: "flash_swap",
    type_arguments: [CoinTypeA, CoinTypeB],
    arguments: [
        SharedObject(GlobalConfig),  // 0xdaa462... (immutable borrow)
        SharedObject(Pool),          // pool ID (mutable borrow)
        Pure(a2b: bool),
        Pure(by_amount_in: bool),
        Pure(amount: u64),
        Pure(sqrt_price_limit: u128),
        SharedObject(Clock),         // 0x6 (immutable borrow)
    ],
}
// Returns: [Balance<A>, Balance<B>, FlashSwapReceipt]
// Use NestedResult(cmd_idx, 0), NestedResult(cmd_idx, 1), NestedResult(cmd_idx, 2)

// Step 2: repay_flash_swap
MoveCall {
    package: same,
    module: "pool",
    function: "repay_flash_swap",
    type_arguments: [CoinTypeA, CoinTypeB],
    arguments: [GlobalConfig, Pool, balance_a, balance_b, receipt],
}
```

### Source code access

Full Move source available at `CetusProtocol/cetus-contracts` (packages/cetus_clmm/sources/). Math modules at `math/clmm_math.move` and `math/tick_math.move`. Signed integer library: `CetusProtocol/integer-mate`.

---

## 3. Turbos Finance: closed source with critical differences from Cetus

### Mainnet package IDs

| Component                               | Address                                                              |
| --------------------------------------- | -------------------------------------------------------------------- |
| **CLMM core (pool types, fee modules)** | `0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1` |
| **Swap router (separate package!)**     | `0xd02012c71c1a6a221e540c36c37c81e0224907fe1ee05bfe250025654ff17103` |

**Critical: Turbos splits the CLMM into two packages.** The swap functions live in `swap_router` at a different address than the `Pool` type definition. Cetus similarly has an `integrate` package, but the primary `flash_swap` lives in the core package.

### Three type parameters per pool

```
Pool<CoinTypeA, CoinTypeB, FeeType>
```

Unlike Cetus's 2-parameter pools, Turbos encodes the fee tier as a **phantom type parameter**. Fee types are separate modules:

| Fee (bps) | Type String                               | Tick Spacing |
| --------- | ----------------------------------------- | ------------ |
| 10,000    | `0x91bf...::fee10000bps::FEE10000BPS`     | 2            |
| 3,000     | `0x91bf...::fee3000bps::FEE3000BPS`       | —            |
| Others    | `0x91bf...::fee{N}bps::FEE{N}BPS` pattern | varies       |

When parsing pool types from on-chain data, filter out the fee type parameter:

```rust
let types: Vec<&str> = pool_type_string.split('<')[1].split('>')[0].split(", ").collect();
let coin_types: Vec<&str> = types.iter().filter(|t| !t.contains("fee")).collect();
```

### Swap function signatures: separate functions per direction

```
swap_router::swap_a_b_with_return_<CoinA, CoinB, Fee>  // A→B
swap_router::swap_b_a_with_return_<CoinA, CoinB, Fee>  // B→A
```

Note the trailing underscore. Three type arguments required — `[CoinTypeA, CoinTypeB, FeeType]`. Arguments include the pool object (shared, mutable), input coins (as vector), amount, and likely sqrt_price_limit, deadline, clock, and a Versioned config object.

**Key difference from Cetus:** Turbos uses **two separate functions** per direction instead of a single function with a boolean flag.

### No public flash swap — the most important difference

**Turbos does not expose a public `flash_swap` function.** This is confirmed by the absence of flash swap documentation in their developer docs, the SDK exposing only `swap` and `computeSwapResult`, and the interface-only source code showing no flash swap stubs.

For cross-DEX atomic arbitrage involving Turbos, you must source initial capital from elsewhere:

1. Use **Cetus `flash_swap`** to borrow the starting token
2. Swap on Turbos using the borrowed tokens (standard swap, requires providing coins)
3. Swap on another pool if multi-hop
4. Repay the Cetus flash_swap receipt

All within one PTB — still fully atomic.

### Swap events

**Event type:** `0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1::pool::SwapEvent`

Same field structure as Cetus (atob, pool, amount_in, amount_out, fee_amount, before/after_sqrt_price, steps) but **Turbos includes `vault_a_amount` and `vault_b_amount`** — direct pool reserve balances post-swap. This is particularly useful for maintaining local state without re-fetching the full object.

### CLMM math: identical foundations, different packaging

Both protocols are Uniswap V3 ports using **Q64.64 sqrt price representation** (u128), base-1.0001 tick spacing, and identical valid tick range (±443,636). The core math (tick↔sqrt_price conversions, swap step computation, tick crossing) should be nearly identical.

However, **Turbos contracts are closed source** — only interface stubs are published at `turbos-finance/turbos-sui-move-interface`. You cannot inspect their tick crossing logic, fee handling, or rounding behavior directly. For simulation, you'll need to either reverse-engineer by testing against real pools or port the Cetus implementation (which is open source) with adjustments for Turbos-specific quirks.

### Contract source and SDK

Move interface repo: `turbos-finance/turbos-sui-move-interface` (interface definitions only, latest tag `mainnet-v0.1.8`). TypeScript SDK: `turbos-clmm-sdk` v3.6.4. Config values fetched dynamically via `sdk.contract.getConfig()` — no published static config.

---

## 4. Sui transaction mechanics that determine your competitive edge

### Shared object ordering is a gas price auction

DEX pools are shared objects requiring Mysticeti consensus. Within a consensus commit, **transactions modifying the same shared object are ordered by gas price** — this is Sui's Priority Gas Auction (PGA). Higher gas = earlier execution. Transactions touching different shared objects execute in parallel across multiple cores.

Under congestion, validators **defer** excess transactions to future consensus commits. Excessively deferred transactions are **actively canceled** — the execution engine releases locked objects and returns a cancellation error.

**Gas price strategy tiers:**

- **5× Reference Gas Price (RGP):** triggers consensus submission amplification — multiple validators submit your tx to consensus
- **100× RGP:** unlocks next-round leader submission with high probability
- Current mainnet RGP: typically **750–1,000 MIST** (query via `sui_getReferenceGasPrice`)

Error codes for contention: `Failed to sign transaction by a quorum of validators because of locked objects` (owned object locked), `ObjectVersionUnavailableForConsensus` (stale shared object version). For owned objects, equivocation (two txs referencing same object version) **freezes the object until epoch end** (~24 hours).

### PTB construction limits

**Maximum 1,024 commands** per PTB. Gas budget specified in MIST (1 SUI = 10^9 MIST), withdrawn upfront and refunded unused. Maximum **255 gas payment objects** per transaction (gas smashing auto-merges them). Protocol limits queryable via `sui_getProtocolConfig`.

`Argument::Result(idx)` is shorthand for `NestedResult(idx, 0)` — valid only when command at index `idx` returns exactly one result. For multi-return functions like `flash_swap` (which returns 3 values), use `NestedResult(cmd_idx, 0)` for Balance<A>, `NestedResult(cmd_idx, 1)` for Balance<B>, `NestedResult(cmd_idx, 2)` for the receipt.

### Concrete 3-hop PTB: Cetus → Turbos → Cetus

```rust
let mut ptb = ProgrammableTransactionBuilder::new();

// --- Inputs ---
let global_config = ptb.obj(ObjectArg::SharedObject {
    id: parse("0xdaa46292632c3c4d8f31f23ea0f9b36a28ff3677e9684980e4438403a67a3d8f"),
    initial_shared_version: v_config, mutable: false,
});
let cetus_pool_1 = ptb.obj(ObjectArg::SharedObject {
    id: cetus_sui_usdc_id, initial_shared_version: v1, mutable: true,
});
let turbos_pool = ptb.obj(ObjectArg::SharedObject {
    id: turbos_usdc_weth_id, initial_shared_version: v2, mutable: true,
});
let cetus_pool_2 = ptb.obj(ObjectArg::SharedObject {
    id: cetus_weth_sui_id, initial_shared_version: v3, mutable: true,
});
let clock = ptb.obj(ObjectArg::SharedObject {
    id: parse("0x6"), initial_shared_version: 1, mutable: false,
});

// --- Cmd 0: Flash swap SUI→USDC on Cetus (borrows SUI, receives USDC) ---
let [bal_a_0, bal_b_0, receipt_0] = ptb.move_call(
    cetus_published_at, "pool", "flash_swap",
    vec![sui_type, usdc_type],
    vec![global_config, cetus_pool_1, pure(true), pure(true),
         pure(amount_in_u64), pure(MIN_SQRT_PRICE), clock],
);
// bal_b_0 = USDC output; bal_a_0 = zero Balance<SUI>

// --- Cmd 1: Convert Balance<USDC> to Coin<USDC> for Turbos ---
// (Turbos swap_router expects Coin, not Balance)
let usdc_coin = ptb.move_call(
    "0x2", "coin", "from_balance",
    vec![usdc_type],
    vec![bal_b_0],
);

// --- Cmd 2: Swap USDC→WETH on Turbos ---
let weth_coin = ptb.move_call(
    turbos_swap_router, "swap_router", "swap_a_b_with_return_",
    vec![usdc_type, weth_type, fee_type],
    vec![turbos_pool, usdc_coin, pure(amount), /* sqrt_price_limit, clock, versioned */],
);

// --- Cmd 3: Swap WETH→SUI on Cetus pool 2 via flash_swap ---
let [bal_a_2, bal_b_2, receipt_2] = ptb.move_call(
    cetus_published_at, "pool", "flash_swap",
    vec![weth_type, sui_type],
    vec![global_config, cetus_pool_2, pure(true), pure(true),
         pure(weth_amount), pure(MIN_SQRT_PRICE), clock],
);

// --- Cmd 4: Repay Cetus pool 2 (provide WETH, keep SUI profit) ---
let weth_balance = ptb.move_call("0x2", "coin", "into_balance", vec![weth_type], vec![weth_coin]);
ptb.move_call(
    cetus_published_at, "pool", "repay_flash_swap",
    vec![weth_type, sui_type],
    vec![global_config, cetus_pool_2, weth_balance, zero_balance_sui, receipt_2],
);

// --- Cmd 5: Repay Cetus pool 1 (provide SUI from profit, keep remaining) ---
// Split required SUI amount from bal_b_2 (our SUI output from pool 2)
ptb.move_call(
    cetus_published_at, "pool", "repay_flash_swap",
    vec![sui_type, usdc_type],
    vec![global_config, cetus_pool_1, sui_repay_balance, bal_a_0, receipt_0],
);

// --- Cmd 6: Transfer remaining profit to sender ---
ptb.transfer_objects(vec![profit_coin], sender);
```

The PTB is fully atomic. If any hop produces insufficient output to repay the flash swap receipts, the entire transaction aborts and only gas is consumed.

### Transaction execution latency

Sui achieves **sub-second finality at P90** via Mysticeti consensus (~500ms average commitment). Owned-object transactions (fast path) finalize in under 500ms. Shared-object transactions add consensus overhead but remain sub-second.

`WaitForEffectsCert` returns when 2/3+ validators have signed effects — **lower latency, use this for bots**. `WaitForLocalExecution` additionally waits for local fullnode execution — higher latency, only needed when subsequent queries to the same node must see updated state.

### Parallel transaction submission

Possible but requires **separate gas coins per transaction**. If two transactions share any owned object (including gas coin) at the same version, validators detect equivocation and **freeze the object until epoch end**. Strategy: pre-split SUI into N gas coins for N concurrent transaction slots. The Mysten Labs `ParallelTransactionExecutor` pattern and `Sui_Owned_Object_Pools` library implement this. **SIP-58 (Address Balances)** will eventually allow gas payment from address balance without specific gas coin objects, eliminating this complexity.

---

## 5. Event subscription: gRPC is the future, Shio Feed is the present

### WebSocket is deprecated

`suix_subscribeEvent` via JSON-RPC WebSocket is **deprecated as of testnet-v1.28.2** (July 2024). JSON-RPC scheduled for decommission by late July 2026. The replacement is **gRPC checkpoint streaming** via `SubscribeCheckpoints`:

```rust
// gRPC streaming (recommended)
let stream = client.subscribe_checkpoints(SubscribeCheckpointsRequest {
    read_mask: Some(FieldMask { paths: vec!["transactions.events".into()] }),
});
for checkpoint in stream {
    for tx in checkpoint.transactions {
        for event in tx.events {
            if event.type_ == CETUS_SWAP_EVENT_TYPE { process_swap(event); }
        }
    }
}
```

Checkpoints arrive **in order without gaps**. On disconnect, track last processed checkpoint sequence number and backfill via `GetCheckpoints` RPC before resubscribing.

### Latency reality check

gRPC checkpoint streaming delivers events at **checkpoint granularity (~2–3 seconds)**. This is too slow for competitive arbitrage. **Shio Feed provides sub-300ms opportunity windows** — for serious MEV, the Shio searcher WebSocket is the primary event source, with gRPC as a fallback for general state monitoring.

### Provider support

Sui Foundation fullnodes may kill gRPC streams after ~30 seconds (known issue #24096). Use third-party providers: **QuickNode** (port 9000, requires x-token auth), **Dwellir** (gRPC with API key), or run your own fullnode.

---

## 6. Shio MEV: the competitive edge layer

### Two operational modes

**Default Mode** (`https://rpc.getshio.com/boost`): Acts as RPC proxy. User transactions are auctioned for **100ms** to searchers. Only backruns — no frontrunning. Rate limit: 10 TPS per IP/sender.

**Fast Mode** (`https://rpc.getshio.com/fast`): Bypasses auction for minimum latency. Requires **tip ≥ 5% of gas_budget**. Rate limit: 20 TPS. Uses advanced routing and multi-region deployment.

### Soft bundles via SIP-19

Maximum **5 transactions** per bundle. All txs must have the same gas price and must write to a shared object. Owned objects must not conflict across bundle txs. Bundles are included in the **same consensus commit** with high probability.

### Searcher API for backrunning

Connect to Shio Feed via WebSocket. The feed streams `auctionStarted` events containing: `txDigest` (opportunity), `gasPrice`, `deadlineTimestampMs` (200–300ms window), and `sideEffects` (mutated objects with full content post-tx — pool states after the opportunity transaction). Only side effects are revealed, not transaction content.

**Bid submission:**

```json
{
    "oppTxDigest": "E72mG9...",
    "bidAmount": 42000000000,
    "txData": "base64EncodedPTB",
    "sig": "base64EncodedSignature"
}
```

Bid requirements: submitted before deadline, bid tx digest must be **lexicographically larger** than opportunity tx digest (binary comparison), bid PTB must include a `MoveCall` to `shio::auctioneer::submit_bid` paying exactly `bid_amount`.

**Shio auctioneer contract:**

```move
// Package: 0x1889977f0fb56ae730e7bda8e8e32859ce78874458c74910d36121a81a615123
module shio::auctioneer {
    public fun submit_bid(
        s: &mut GlobalState,   // 32 objects, pick randomly to reduce contention
        bid_amount: u64,
        fee: Balance<SUI>,
        _ctx: &mut TxContext
    ) {}
}
```

### Is Shio required?

SIP-19 (soft bundles) is a **protocol-level feature** — all validators support it. You don't need Shio for basic PGA-based arbitrage via direct `execute_transaction_block`. However, Shio provides critical advantages: the **opportunity feed** gives you visibility into pending transactions (200–300ms before they finalize), the **bundle atomicity** ensures your backrun executes immediately after the target, and their infrastructure is optimized for latency. From the Sui MEV blog: "With 15 commits/sec, a **70 millisecond** advantage in submission speed is a deal breaker." **Colocate in Frankfurt, Germany** — Shio's recommended searcher location.

---

## 7. Sui Rust SDK: two options, different tradeoffs

### Legacy SDK (sui-sdk) — battle-tested but deprecated transport

Add via git (NOT crates.io — the placeholder is empty):

```toml
[dependencies]
sui-sdk = { git = "https://github.com/mystenlabs/sui", package = "sui-sdk", tag = "mainnet-v1.XX.Y" }
sui-types = { git = "https://github.com/mystenlabs/sui", package = "sui-types", tag = "mainnet-v1.XX.Y" }
```

**Known compilation issue (#22336):** The git dependency expects workspace-local `move-core-types` at specific paths. Workaround: clone the full MystenLabs/sui repo and use `path` dependencies.

### New SDK (sui-rust-sdk) — modular, on crates.io

```toml
[dependencies]
sui-sdk-types = "0.1.1"           # Core types
sui-crypto = { version = "0.1", features = ["ed25519"] }
sui-transaction-builder = "0.1"   # PTB building
sui-rpc = "0.1"                   # gRPC client
```

Uses gRPC and GraphQL — no JSON-RPC. Lightweight and WASM-compatible. Still young with fewer examples.

### Client initialization

```rust
// Legacy
let client = SuiClientBuilder::default()
    .build("https://fullnode.mainnet.sui.io:443").await?;

// New SDK (gRPC)
let client = sui_rpc::Client::new("https://your-grpc-endpoint:443")?;
```

### Multi-object fetching

```rust
let objects = client.read_api().multi_get_objects(
    vec![pool1_id, pool2_id, pool3_id],
    Some(SuiObjectDataOptions::new().with_content().with_bcs()),
).await?;
```

### Dry run

```rust
let dry_run = client.read_api()
    .dry_run_transaction_block(bcs::to_bytes(&tx_data)?).await?;
// Returns effects, events, balance changes, object changes
// Use to estimate gas and validate before submitting
```

### Key gotcha: type annotation in pure arguments

When using `ptb.pure(N)`, **always annotate the type**: `ptb.pure(1000u64)`, not `ptb.pure(1000)`. Rust defaults to `i32`, causing `VMVerificationOrDeserializationError` on-chain.

---

## 8. Pool state deserialization: BCS for speed, JSON for resilience

### BCS deserialization of Cetus pools

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct I32 { pub bits: u32 }
impl I32 { pub fn to_i32(&self) -> i32 { self.bits as i32 } }

#[derive(Debug, Deserialize)]
pub struct Balance { pub value: u64 }

#[derive(Debug, Deserialize)]
pub struct TickManager {
    pub tick_spacing: u32,
    pub ticks: ObjectID,  // Table handle — dynamic fields stored here
}

#[derive(Debug, Deserialize)]
pub struct CetusPool {
    pub id: ObjectID,
    pub coin_a: Balance,
    pub coin_b: Balance,
    pub tick_spacing: u32,
    pub fee_rate: u64,
    pub liquidity: u128,
    pub current_sqrt_price: u128,
    pub current_tick_index: I32,
    pub fee_growth_global_a: u128,
    pub fee_growth_global_b: u128,
    pub fee_protocol_coin_a: u64,
    pub fee_protocol_coin_b: u64,
    pub tick_manager: TickManager,
    pub rewarder_manager: RewarderManager,
    pub position_manager: PositionManager,
    pub is_pause: bool,
    pub index: u64,
    pub url: String,
}

fn deserialize_cetus_pool(response: &SuiObjectResponse) -> Result<CetusPool> {
    let bcs_data = response.data.as_ref()?.bcs.as_ref()?;
    match bcs_data {
        SuiRawData::MoveObject(obj) => Ok(bcs::from_bytes(&obj.bcs_bytes)?),
        _ => Err(anyhow!("Not a Move object")),
    }
}
```

**BCS field order must exactly match the Move struct definition.** If Cetus upgrades and adds fields, your struct breaks. For resilience, consider JSON deserialization (`serde_json::from_value` on `obj.fields`) which is field-name-based but slower.

### Tick data fetching via dynamic fields

```rust
// 1. Get tick table handle from deserialized pool
let ticks_handle = pool.tick_manager.ticks;

// 2. Enumerate all tick keys (paginated)
let mut cursor = None;
let mut all_tick_ids = vec![];
loop {
    let page = client.read_api()
        .get_dynamic_fields(ticks_handle, cursor, Some(50)).await?;
    for field in &page.data {
        all_tick_ids.push(field.object_id);
    }
    cursor = page.next_cursor;
    if !page.has_next_page { break; }
}

// 3. Batch fetch all tick objects
let tick_objects = client.read_api()
    .multi_get_objects(all_tick_ids, Some(opts.with_bcs())).await?;

// 4. Deserialize each tick
#[derive(Debug, Deserialize)]
pub struct Tick {
    pub index: I32,
    pub sqrt_price: u128,
    pub liquidity_net: I128,
    pub liquidity_gross: u128,
    pub fee_growth_outside_a: u128,
    pub fee_growth_outside_b: u128,
    pub rewards_growth_outside: Vec<u128>,
}
```

### Turbos pool deserialization

Similar structure but with different field layout (pseudocode — verify against actual object):

```rust
#[derive(Debug, Deserialize)]
pub struct TurbosPool {
    pub id: ObjectID,
    pub coin_a: Balance,
    pub coin_b: Balance,
    pub fee: u32,
    pub tick_spacing: u32,
    pub liquidity: u128,
    pub sqrt_price: u128,          // Same Q64.64 as Cetus
    pub tick_current_index: I32,
    pub fee_growth_global_a: u128,
    pub fee_growth_global_b: u128,
    pub protocol_fees_a: u64,
    pub protocol_fees_b: u64,
    pub ticks: ObjectID,           // Dynamic field table
    pub reward_infos: Vec<RewardInfo>,
    pub reward_last_updated_time_ms: u64,
    pub is_locked: bool,
}
```

Because Turbos contracts are closed source, you must **inspect a real pool object** via `sui_getObject` with `with_content()` to confirm the exact field order and types before writing BCS deserialization.

---

## 9. Token type system on Sui mainnet

### Canonical type strings

| Token                            | Full Type String                                                                 | Decimals |
| -------------------------------- | -------------------------------------------------------------------------------- | -------- |
| **SUI**                          | `0x2::sui::SUI`                                                                  | 9        |
| **USDC** (Circle native)         | `0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC` | 6        |
| **USDT** (Wormhole)              | `0xc060006111016b8a020ad5b33834984a437aaa7d3c74c18e09a95d9c823fb8ab::coin::COIN` | 6        |
| **WETH** (Wormhole)              | `0xaf8cd5edc19c4512f4259f0bee101a40d41ebed738ade5874359610ef8eeced5::coin::COIN` | 8        |
| **wBTC** (Wormhole)              | `0x027792d9fed7f9844eb4839566001bb6f6cb4804f66aa2da6fe1ee242d896881::coin::COIN` | 8        |
| **wUSDC** (Wormhole, deprecated) | `0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN` | 6        |

### TypeTag parsing

```rust
use sui_types::TypeTag;
use std::str::FromStr;

let tag = TypeTag::from_str(
    "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC"
)?;
```

### Decimals resolution

```rust
let metadata = client.coin_read_api()
    .get_coin_metadata("0xdba346...::usdc::USDC".into()).await?;
let decimals = metadata.unwrap().decimals; // 6
```

### Token discovery from pool type

Pool types are parameterized: `Pool<CoinTypeA, CoinTypeB>` (Cetus) or `Pool<CoinTypeA, CoinTypeB, FeeType>` (Turbos). Extract from the object's `type_` field:

```rust
fn extract_coin_types(pool_type: &str) -> (String, String) {
    let inner = &pool_type[pool_type.find('<').unwrap()+1..pool_type.rfind('>').unwrap()];
    let parts: Vec<&str> = inner.split(", ")
        .filter(|p| !p.contains("fee"))  // Filter out Turbos fee type
        .collect();
    (parts[0].to_string(), parts[1].to_string())
}
```

---

## Conclusion: architecture decisions from the research

The critical architectural insight is that **Cetus `flash_swap` is your capital source** — use it to borrow tokens for zero-capital atomic arbitrage across both Cetus and Turbos pools in a single PTB. Turbos lacks flash swap, so all Turbos legs must receive coins from a prior step. Build a shared CLMM math library (Q64.64 sqrt price, tick traversal) that works for both protocols, but expect to tune rounding behavior for Turbos since its implementation is closed source.

For the event pipeline, skip WebSocket entirely (deprecated) and build dual-path: **Shio Feed WebSocket** for sub-300ms opportunity detection with backrun bidding, plus **gRPC checkpoint streaming** as fallback and for state synchronization. Pre-split gas coins into a pool for concurrent transaction slots, and set gas prices at 5–100× RGP depending on opportunity size.

The `fuzzland/sui-mev` codebase validates the workspace separation (simulator, dex-indexer, object-pool, shio as independent crates) and the burberry collector→strategy→executor pipeline as production-proven patterns. Its `object-pool` crate for version management solves a Sui-specific problem you'll need to replicate. Build from scratch using the new `sui-rust-sdk` crates on crates.io with gRPC transport, using the legacy SDK source as reference for PTB construction patterns until the new SDK matures.
