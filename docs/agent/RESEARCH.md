# Sui DEX Arbitrage Bot — Technical Research Findings

> Research date: 2026-04-05
> Target DEXes: Cetus CLMM, Turbos Finance CLMM
> Reference repo: fuzzland/sui-mev

---

## Table of Contents

1. [fuzzland/sui-mev Analysis](#1-fuzzlandsui-mev-analysis)
2. [Cetus Protocol — Technical Deep Dive](#2-cetus-protocol--technical-deep-dive)
3. [Turbos Finance — Technical Deep Dive](#3-turbos-finance--technical-deep-dive)
4. [Cetus vs Turbos — Comparison](#4-cetus-vs-turbos--all-differences-that-matter)
5. [Sui Transaction Mechanics](#5-sui-transaction-mechanics)
6. [Shio MEV Protocol](#6-shio-mev-protocol)
7. [Sui Rust SDK](#7-sui-rust-sdk)
8. [Pool State Deserialization](#8-pool-state-deserialization)
9. [Token Types (Mainnet)](#9-token-types-mainnet)
10. [Architecture Implications](#10-architecture-implications)
11. [Open Questions](#11-open-questions)

---

## 1. fuzzland/sui-mev Analysis

**Repository**: https://github.com/fuzzland/sui-mev
**Stars**: 766 | **Forks**: 506 | **Created**: 2025-04-02 (single "release" commit)

### Project Structure
- 10-crate Rust workspace using `burberry` (Artemis-like Collector/Strategy/Executor pattern)
- **Binaries**: `bin/arb/` (main bot), `bin/relay/` (mempool relay)
- **Libraries**: `dex-indexer` (14 DEX adapters), `shio` (MEV auction), `simulator` (Http/DB/Replay), `object-pool`, `utils`
- DEX adapters: Cetus, Turbos, Aftermath, BlueMove, DeepbookV2, FlowxClmm, KriyaAmm, KriyaClmm, Navi (flash loans only)
- **Critical dependency**: Forks Sui SDK from `https://github.com/suiflow/mevsui` (branch `relay-patch`) — patched Sui node exposing mempool via Unix socket

### Critical Finding: No Local Swap Math
The bot does **zero local CLMM math**. Every price check builds a full PTB and executes it through a simulator (either RPC `devInspectTransactionBlock` or local RocksDB via patched Sui node). This is orders of magnitude slower than local tick math.

### Architecture
- **Event-driven**: Parses swap events from tx effects → extracts `(coin_type, pool_id)` → `ArbCache` (HashMap + BinaryHeap, 5s TTL)
- **Path discovery**: BFS/DFS up to 2 hops, max 10 pools/hop, min liquidity 1000
- **Grid search**: 10 parallel trials at logarithmic amounts (0.01 SUI to 10B SUI), each simulated via PTB
- **Golden section search**: Exists but disabled (`use_gss: false`)
- **8 worker threads**, 128MB stack each
- **Flash loan support**: Cetus flash_swap for capital-free arb

### Pool State Fetching
```rust
let pool_obj = simulator.get_object(&pool.pool).await?;
let parsed_pool = MoveStruct::simple_deserialize(move_obj.contents(), &layout)?;
let liquidity = extract_u128_from_move_struct(&parsed_pool, "liquidity")?;
let is_pause = extract_bool_from_move_struct(&parsed_pool, "is_pause")?;
```

Three concurrent lookup structures in `dex-indexer`:
```rust
pub type TokenPools = DashMap<String, HashSet<Pool>>;
pub type Token01Pools = DashMap<(String, String), HashSet<Pool>>;
// plus pool_map: DashMap<ObjectID, Pool>
```

### Shio Integration
- WebSocket collector at `wss://rpc.getshio.com/feed`
- Reconstructs post-opportunity-tx world state from Shio's BCS data using `MoveObject::new_from_execution(unsafe)`
- Bid = 90% of profit: `profit / 10 * 9`
- Gas price must match opportunity tx
- Bid format:
```rust
json!({
    "oppTxDigest": opp_tx_digest.base58_encode(),
    "bidAmount": bid_amount,
    "txData": tx_b64,
    "sig": sig,
})
```

### PTB Construction Pattern (from TradeCtx)
```rust
pub struct TradeCtx {
    pub ptb: ProgrammableTransactionBuilder,
    command_count: u16,
}

// Multi-hop flashloan PTB:
// 1. Flash borrow from first DEX (e.g., Cetus flash_swap_a2b)
// 2. Chain intermediate swaps via extend_trade_tx() for each hop
// 3. Repay flash loan via extend_repay_tx()
// 4. For Shio: append bid submission
// 5. Transfer profit to sender
```

### Cetus DEX Adapter (key objects)
```rust
const CETUS_DEX: &str = "0xeffc8ae61f439bb34c9b905ff8f29ec56873dcedf81c7123ff2f1f67c45ec302";
// Move call arguments: [config, pool, partner, coin_in, clock]
// Supports flash loans: flash_swap_a2b returns (Coin<CoinB>, FlashSwapReceipt<CoinA, CoinB>, u64)
```

### Turbos DEX Adapter
```rust
const VERSIONED: &str = "0xf1cf0e81048df168ebeb1b8030fad24b3e0b53ae827c25053fff0779c1445b6f";
// Move call arguments: [pool, coin_in, clock, versioned]
// No flash swap support in their adapter (but Turbos contract does support it)
```

### Key Weaknesses
1. Requires **patched Sui node** (`suiflow/mevsui` fork) — not portable
2. `OnceCell`-cached global objects (config/partner) never refreshed — upgrades break it
3. Hardcoded package IDs everywhere, no config-driven approach
4. Silent error swallowing in path finding (commented-out error logging)
5. Single "release" commit, suspicious star count (766 stars overnight)
6. No DeepBook V3 despite enum variant existing
7. Issue #3: User reports being "robbed" using the script
8. Issue #4: Serialization error (`invalid value: integer 9`) — fragile Move struct deserialization

### Verdict: **Reference only, do not fork**
Valuable for: PTB construction patterns per DEX, Shio bid format, pool indexing, Sui object handling patterns. Must build own local swap math.

---

## 2. Cetus Protocol — Technical Deep Dive

### Package IDs
| Contract | Address |
|----------|---------|
| CLMM (original, for types) | `0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb` |
| CLMM (published_at, for calls) | `0xc6faf3703b0e8ba9ed06b7851134bbbe7565eb35ff823fd78432baa4cbeaa12e` |
| Integrate | `0x996c4d9480708fb8b92aa7acf819fb0497b5ec8e65ba06601cae2fb6db3312c3` |
| Aggregator | `0x639b5e433da31739e800cd085f356e64cae222966d0f1b11bd9dc76b322ff58b` |

> **WARNING**: `published_at` changes with every package upgrade. Verify on-chain before use.

### Shared Objects
| Object | ID |
|--------|-----|
| GlobalConfig | `0xdaa46292632c3c4d8f31f23ea0f9b36a28ff3677e9684980e4438403a67a3d8f` |
| Pools (factory) | `0xf699e7f2276f5c9a75944b37a0c5b5d9ddfd2471bf6242483b03ab2887d198d0` |
| RewarderGlobalVault | `0xce7bceef26d3ad1f6d9b6f13a953f053e6ed3ca77907516481ce99ae8e588f2b` |
| Clock | `0x0000000000000000000000000000000000000000000000000000000000000006` |

### Pool Object Structure
```move
struct Pool<phantom CoinTypeA, phantom CoinTypeB> has key, store {
    id: UID,
    coin_a: Balance<CoinTypeA>,
    coin_b: Balance<CoinTypeB>,
    tick_spacing: u32,
    fee_rate: u64,                  // PPM (denominator = 1,000,000)
    liquidity: u128,                // active liquidity at current tick
    current_sqrt_price: u128,       // Q64.64 fixed-point
    current_tick_index: I32,        // signed 32-bit integer (custom type)
    fee_growth_global_a: u128,      // Q64.64
    fee_growth_global_b: u128,      // Q64.64
    fee_protocol_coin_a: u64,
    fee_protocol_coin_b: u64,
    tick_manager: TickManager,
    rewarder_manager: RewarderManager,
    position_manager: PositionManager,
    is_pause: bool,
    index: u64,
    url: String,
}
```

Pool type string format: `0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::pool::Pool<CoinTypeA, CoinTypeB>`

### Tick Storage
```move
struct TickManager has store {
    tick_spacing: u32,
    ticks: SkipList<Tick>,      // Custom data structure from move-stl, NOT Table/LinkedTable
}

struct Tick has copy, drop, store {
    index: I32,
    sqrt_price: u128,            // Q64.64
    liquidity_net: I128,         // signed — change in liquidity when crossing
    liquidity_gross: u128,       // total liquidity referencing this tick
    fee_growth_outside_a: u128,  // Q64.64
    fee_growth_outside_b: u128,  // Q64.64
    points_growth_outside: u128,
    rewards_growth_outside: vector<u128>,
}
```

**Key format**: Tick indices (range `[-443636, +443636]`) are converted to a `u64` score via `tick_score()`: the I32 tick index is offset by `TICK_BOUND` (443636) to produce a non-negative key. So tick -100 becomes score `443536`, tick 0 becomes `443636`, tick +100 becomes `443736`.

**Fetching ticks off-chain**: Use `sui_getDynamicFields` on the TickManager's UID to enumerate SkipList nodes. Or use the on-chain `fetch_ticks<A,B>(pool, start, limit)` function.

### Pool Enumeration
The Pools object (`0xf699e7...`) contains a `LinkedTable<ID, PoolSimpleInfo>`:
```move
struct Pools has key, store {
    id: UID,
    list: LinkedTable<ID, PoolSimpleInfo>,
    index: u64,
}

struct PoolSimpleInfo has copy, drop, store {
    pool_id: ID,
    pool_key: ID,
    coin_type_a: TypeName,
    coin_type_b: TypeName,
    tick_spacing: u32,
}
```

Methods:
1. On-chain: `fetch_pools(pools, start_id, limit)`
2. Off-chain: `sui_getDynamicFields` on Pools object ID
3. Events: Listen for `CreatePoolEvent`

Pool uniqueness: `(coin_type_a, coin_type_b, tick_spacing)`. Coin types must be lexicographically ordered (a < b).

### Swap Functions (Flash Swap Pattern — Two-Step)
```move
// Step 1: Initiate swap
public fun flash_swap<CoinTypeA, CoinTypeB>(
    config: &GlobalConfig,
    pool: &mut Pool<CoinTypeA, CoinTypeB>,
    a2b: bool,
    by_amount_in: bool,
    amount: u64,
    sqrt_price_limit: u128,
    clock: &Clock,
): (Balance<CoinTypeA>, Balance<CoinTypeB>, FlashSwapReceipt<CoinTypeA, CoinTypeB>)

// Step 2: Repay
public fun repay_flash_swap<CoinTypeA, CoinTypeB>(
    config: &GlobalConfig,
    pool: &mut Pool<CoinTypeA, CoinTypeB>,
    coin_a: Balance<CoinTypeA>,
    coin_b: Balance<CoinTypeB>,
    receipt: FlashSwapReceipt<CoinTypeA, CoinTypeB>,
)
```

Partner variants also exist: `flash_swap_with_partner` / `repay_flash_swap_with_partner`

Convenience wrappers (in Aggregator contract `0x639b5e...`):
```move
public fun swap_a2b<CoinA, CoinB>(config, pool, partner, coin_a: Coin<CoinA>, clock, ctx): Coin<CoinB>
public fun swap_b2a<CoinA, CoinB>(config, pool, partner, coin_b: Coin<CoinB>, clock, ctx): Coin<CoinA>
```

Pre-calculation (read-only):
```move
public fun calculate_swap_result<CoinTypeA, CoinTypeB>(
    pool: &Pool<CoinTypeA, CoinTypeB>,
    a2b: bool,
    by_amount_in: bool,
    amount: u64,
): CalculatedSwapResult
```

### Swap Direction
- **`a2b = true`**: Selling CoinTypeA, receiving CoinTypeB. Price moves DOWN. `sqrt_price_limit` = `MIN_SQRT_PRICE_X64 + 1` = `4295048017`
- **`a2b = false`**: Selling CoinTypeB, receiving CoinTypeA. Price moves UP. `sqrt_price_limit` = `MAX_SQRT_PRICE_X64 - 1` = `79226673515401279992447579054`
- **`by_amount_in = true`**: `amount` is exact input. Output is variable.
- **`by_amount_in = false`**: `amount` is desired output. Input is variable.

After `flash_swap` returns:
- If `a2b`: `Balance<CoinTypeB>` is non-zero (output), you repay with `Balance<CoinTypeA>` = `receipt.pay_amount`
- If `b2a`: `Balance<CoinTypeA>` is non-zero (output), you repay with `Balance<CoinTypeB>` = `receipt.pay_amount`

### Swap Math (compute_swap_step)

```
1. Start at current_sqrt_price, current_tick_index, with active liquidity
2. Find next initialized tick in direction of travel (SkipList traversal)
3. For each step, call compute_swap_step():
   a. If by_amount_in: deduct fee first
      amount_remain = amount * (1_000_000 - fee_rate) / 1_000_000
   b. Compute max_amount movable within [current_price, target_price] given liquidity
   c. If amount_remain >= max_amount:
      - next_sqrt_price = target_sqrt_price (tick fully crossed)
      - Compute actual amount_in and amount_out from price delta
      - Cross tick: liquidity += tick.liquidity_net (or -= if a2b)
   d. If amount_remain < max_amount:
      - Compute next_sqrt_price from partial fill
      - amount_in, amount_out from price delta
      - Swap complete
   e. Fee = amount_in * fee_rate / (1_000_000 - fee_rate)
4. Accumulate across steps into SwapResult
```

```move
compute_swap_step(
    current_sqrt_price: u128,
    target_sqrt_price: u128,
    liquidity: u128,
    amount: u64,
    fee_rate: u64,
    a2b: bool,
    by_amount_in: bool,
): (amount_in: u64, amount_out: u64, next_sqrt_price: u128, fee_amount: u64)
```

Return structs:
```move
struct CalculatedSwapResult has copy, drop, store {
    amount_in: u64,
    amount_out: u64,
    fee_amount: u64,
    fee_rate: u64,
    after_sqrt_price: u128,
    is_exceed: bool,          // true if pool liquidity exhausted before amount filled
    step_results: vector<SwapStepResult>,
}

struct SwapStepResult has copy, drop, store {
    current_sqrt_price: u128,
    target_sqrt_price: u128,
    current_liquidity: u128,
    amount_in: u64,
    amount_out: u64,
    fee_amount: u64,
    remainder_amount: u64,
}
```

### Fee Tiers

Fee denominator: **1,000,000** (PPM). Protocol fee: 20% of swap fees (DEFAULT_PROTOCOL_FEE_RATE = 2000/10000).

| tick_spacing | fee_rate (PPM) | % |
|---|---|---|
| 2 | 100 | 0.01% |
| 10 | 500 | 0.05% |
| 20 | 1,000 | 0.10% |
| 60 | 2,500 | 0.25% |
| 200 | 10,000 | 1.00% |
| 220 | 20,000 | 2.00% |

Constants:
- `MAX_FEE_RATE`: 200,000 (20%)
- `MAX_PROTOCOL_FEE_RATE`: 3,000 (30% of swap fee)
- `MAX_TICK_SPACING`: 1,000

### Sqrt Price Representation

**Q64.64 fixed-point** (X64 format). `current_sqrt_price` is `u128` where value = `sqrt(price) * 2^64`.

| Constant | Value |
|---|---|
| `MIN_SQRT_PRICE_X64` | `4295048016` |
| `MAX_SQRT_PRICE_X64` | `79226673515401279992447579055` |
| `TICK_BOUND` | `443636` |
| `MIN_TICK` / `MAX_TICK` | `-443636` / `+443636` |

To convert: `price = (sqrt_price / 2^64)^2`

### Swap Event
```
Type: 0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::pool::SwapEvent
```

```move
struct SwapEvent has copy, drop, store {
    atob: bool,
    pool: ID,
    partner: ID,
    amount_in: u64,
    amount_out: u64,
    ref_amount: u64,
    fee_amount: u64,
    vault_a_amount: u64,           // pool's coin_a balance after swap
    vault_b_amount: u64,           // pool's coin_b balance after swap
    before_sqrt_price: u128,
    after_sqrt_price: u128,
    steps: u64,                    // number of tick crossings
}
```

**State update from events**: Can update `current_sqrt_price` = `after_sqrt_price`, balances = `vault_a/b_amount`. Cannot reconstruct `liquidity` (ticks crossed not specified). Periodic full re-fetch needed.

### PTB MoveCall Structure

Target uses `published_at` address. Type arguments use original package ID.

```
Call 1: flash_swap
  target: "0xc6faf3703b0e8ba9ed06b7851134bbbe7565eb35ff823fd78432baa4cbeaa12e::pool::flash_swap"
  typeArguments: [CoinTypeA, CoinTypeB]
  arguments: [config, pool, a2b, by_amount_in, amount, sqrt_price_limit, clock]
  returns: (Balance<A>, Balance<B>, FlashSwapReceipt<A,B>)

Call 2: repay_flash_swap
  target: "0xc6faf3703b0e8ba9ed06b7851134bbbe7565eb35ff823fd78432baa4cbeaa12e::pool::repay_flash_swap"
  typeArguments: [CoinTypeA, CoinTypeB]
  arguments: [config, pool, balance_a, balance_b, receipt]
```

### Object Access Patterns
| Object | Access | Contention |
|--------|--------|------------|
| GlobalConfig | Shared, `&` (immutable) | Low (read-only) |
| Pool | Shared, `&mut` (mutable) | **HIGH — primary contention point** |
| Clock | Shared, `&` (immutable) | Minimal |
| Coin objects | Owned | None (sender-exclusive) |
| FlashSwapReceipt | Hot potato (no key/store) | Must consume in same tx |

---

## 3. Turbos Finance — Technical Deep Dive

### Package IDs
| Contract | Address |
|----------|---------|
| CLMM (original, for types) | `0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1` |
| CLMM (published_at, for calls) | `0xa5a0c25c79e428eba04fb98b3fb2a34db45ab26d4c8faf0d7e39d66a63891e64` |

> SDK fetches addresses dynamically from `https://s3.amazonaws.com/app.turbos.finance/sdk/contract.json`

### Shared Objects
| Object | ID |
|--------|-----|
| Versioned | `0xf1cf0e81048df168ebeb1b8030fad24b3e0b53ae827c25053fff0779c1445b6f` |
| PoolConfig | `0xc294552b2765353bcafa7c359cd28fd6bc237662e5db8f09877558d81669170c` |
| Positions | `0xf5762ae5ae19a2016bb233c72d9a4b2cba5a302237a82724af66292ae43ae52d` |
| PoolTableId | `0x08984ed8705f44b6403705dc248896e56ab7961447820ae29be935ce0d32198b` |
| AclConfig | `0x0302b15f040b008a1bc011cd85231605299ceaac2f9699e4e826ade0a61f3fbe` |

### Pool Object Structure — 3 Type Parameters
```move
struct Pool<phantom CoinTypeA, phantom CoinTypeB, phantom FeeType> has key, store {
    id: UID,
    coin_a: Balance<CoinTypeA>,
    coin_b: Balance<CoinTypeB>,
    protocol_fees_a: u64,
    protocol_fees_b: u64,
    sqrt_price: u128,               // Q64.64 (same as Cetus)
    tick_current_index: I32,        // two's complement in u32
    tick_spacing: u32,
    max_liquidity_per_tick: u128,
    fee: u32,                       // fee numerator, denominator = 1,000,000
    fee_protocol: u32,
    unlocked: bool,                 // (Cetus uses is_pause)
    fee_growth_global_a: u128,      // Q64.64
    fee_growth_global_b: u128,      // Q64.64
    liquidity: u128,
    tick_map: Table<I32, u256>,     // Bitmap for tick initialization (standard UniV3)
    deploy_time_ms: u64,
    reward_infos: vector<PoolRewardInfo>,
    reward_last_updated_time_ms: u64,
}
```

The `I32` struct:
```move
struct I32 has copy, drop, store {
    bits: u32   // two's complement, sign bit at bit 31
}
```

### Tick Storage — Standard UniV3 Bitmap
**Bitmap**: `tick_map: Table<I32, u256>` — each entry maps a word index to a 256-bit bitmap. Each bit represents whether a tick at that position is initialized.

**Individual ticks** as dynamic fields on Pool:
```move
struct Tick has key, store {
    id: UID,
    liquidity_gross: u128,
    liquidity_net: I128,
    fee_growth_outside_a: u128,
    fee_growth_outside_b: u128,
    reward_growths_outside: vector<u128>,
    initialized: bool,
}
```

Tick fetching via `devInspectTransactionBlock` with `fetch_ticks<A,B,Fee>()`, batch size 900, paginated with cursor.

### Pool Enumeration
Query dynamic fields on `PoolTableId` object. Each field contains `PoolSimpleInfo` with `pool_id`, `pool_key`, `coin_type_a`, `coin_type_b`, `fee_type`, `fee`, `tick_spacing`.

Or use `pool_factory::get_pool_id<CoinTypeA, CoinTypeB, FeeType>` for specific lookups.

### Swap Functions

**Entry functions (swap_router module):**
```move
public entry fun swap_a_b<CoinTypeA, CoinTypeB, FeeType>(
    pool: &mut Pool<CoinTypeA, CoinTypeB, FeeType>,
    coins_a: vector<Coin<CoinTypeA>>,
    amount: u64,
    amount_threshold: u64,
    sqrt_price_limit: u128,
    is_exact_in: bool,
    recipient: address,
    deadline: u64,
    clock: &Clock,
    versioned: &Versioned,
    ctx: &mut TxContext
)

public entry fun swap_b_a<CoinTypeA, CoinTypeB, FeeType>(...)  // mirror
```

**Composable (for PTBs):**
```move
public fun swap_a_b_with_return_<A, B, Fee>(...): (Coin<B>, Coin<A>)  // (output, remainder)
public fun swap_b_a_with_return_<A, B, Fee>(...): (Coin<A>, Coin<B>)
```

**Flash swap (pool module):**
```move
public fun flash_swap<CoinTypeA, CoinTypeB, FeeType>(
    pool: &mut Pool<CoinTypeA, CoinTypeB, FeeType>,
    recipient: address,
    a_to_b: bool,
    amount_specified: u128,         // NOTE: u128, not u64 like Cetus
    amount_specified_is_input: bool,
    sqrt_price_limit: u128,
    clock: &Clock,
    versioned: &Versioned,
    ctx: &mut TxContext
): (Coin<CoinTypeA>, Coin<CoinTypeB>, FlashSwapReceipt<CoinTypeA, CoinTypeB>)

public fun repay_flash_swap<CoinTypeA, CoinTypeB, FeeType>(
    pool: &mut Pool<CoinTypeA, CoinTypeB, FeeType>,
    coin_a: Coin<CoinTypeA>,
    coin_b: Coin<CoinTypeB>,
    receipt: FlashSwapReceipt<CoinTypeA, CoinTypeB>,
    versioned: &Versioned
)
```

**Multi-hop (2-pool, swap_router):**
```move
swap_a_b_b_c  // Pool1: A→B, Pool2: B→C
swap_a_b_c_b  // Pool1: A→B, Pool2: C→B
swap_b_a_b_c  // Pool1: B→A, Pool2: B→C
swap_b_a_c_b  // Pool1: B→A, Pool2: C→B
// Type args for 2-hop: [Pool1_CoinA, Pool1_FeeType, Pool1_CoinB, Pool2_FeeType, Pool2_OutputCoin]
```

### Swap Direction
- `a_to_b = true`: Input CoinA, output CoinB. Price decreases. `sqrt_price_limit` = `MIN_SQRT_PRICE + 1 = 4295048017`
- `a_to_b = false`: Input CoinB, output CoinA. Price increases. `sqrt_price_limit` = `MAX_SQRT_PRICE - 1`
- `_with_return_` variants return coins for PTB composition

### Fee Tiers

Fee denominator: **1,000,000**. Fee stored as `fee: u32` in pool + phantom `FeeType`.

Common tiers (from SDK):
| Label | Fee (PPM) | Actual % |
|---|---|---|
| 10bps | 1,000 | 0.1% |
| 100bps | 10,000 | 1.0% |
| 500bps | 50,000 | 5.0% |
| 2500bps | 250,000 | 25.0% |
| 3000bps | 300,000 | 30.0% |

> WARNING: The "bps" labels in SDK are misleading. Cross-reference with actual pool `fee` field values.

Fee formula: `fee_amount = amount_in * fee_rate / (1_000_000 - fee_rate)` (standard UniV3 approach)

### Tick Math Constants
```
MAX_TICK_INDEX = 443636        (same as Cetus)
MIN_SQRT_PRICE = 4295048016   (same as Cetus)
MAX_SQRT_PRICE = 79226673515401279992447579055  (same as Cetus)
BIT_PRECISION = 14             (for log2 computation)
```

Both use Q64.64 fixed-point. Same tick-to-sqrt-price precomputed ratio constants.

### Swap Event
```
Type: 0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1::pool::SwapEvent
```

```move
struct SwapEvent has copy, drop, store {  // Structurally identical to Cetus SwapEvent
    atob: bool, pool: ID, partner: ID,
    amount_in: u64, amount_out: u64, ref_amount: u64, fee_amount: u64,
    vault_a_amount: u64, vault_b_amount: u64,
    before_sqrt_price: u128, after_sqrt_price: u128, steps: u64,
}
```

### PTB MoveCall Structure

For `swap_a_b_with_return_`:
```
target: "0xa5a0c25c79e428eba04fb98b3fb2a34db45ab26d4c8faf0d7e39d66a63891e64::swap_router::swap_a_b_with_return_"

typeArguments: [
    "<CoinTypeA>",
    "<CoinTypeB>",
    "<FeeType>"     // e.g. "0x91bfbc...::fee3000bps::FEE3000BPS"
]

arguments: [
    pool_object_id,
    makeMoveVec([input_coin]),       // vector<Coin<CoinTypeA>>
    pure(amount_u64),
    pure(min_amount_out_u64),
    pure(sqrt_price_limit_u128),
    pure(true),                      // is_exact_in
    pure(recipient_address),
    pure(deadline_ms_u64),
    object(0x6),                     // Clock
    object(Versioned_id),
]

returns: (Coin<CoinTypeB>, Coin<CoinTypeA>)  // (output, remainder)
```

---

## 4. Cetus vs Turbos — All Differences That Matter

| Aspect | Cetus | Turbos |
|--------|-------|--------|
| **Pool type params** | `<CoinA, CoinB>` (2) | `<CoinA, CoinB, FeeType>` (3) |
| **Fee storage** | `fee_rate: u64` field | `fee: u32` field + phantom `FeeType` |
| **Fee denominator** | 1,000,000 | 1,000,000 (same) |
| **Tick storage** | SkipList (custom) | Table<I32, u256> bitmap (standard UniV3) |
| **Swap direction** | Single fn + `a2b: bool` | Separate `swap_a_b` / `swap_b_a` functions |
| **Flash swap amount type** | `u64` | `u128` |
| **Flash swap returns** | `Balance<A>, Balance<B>` | `Coin<A>, Coin<B>` |
| **Required shared obj** | GlobalConfig | Versioned |
| **Input coin format** | Balance (via flash_swap) | vector<Coin> (via swap_router) |
| **Pool lock field** | `is_pause: bool` | `unlocked: bool` |
| **sqrt_price precision** | Q64.64, same constants | Q64.64, same constants |
| **Tick bounds** | ±443636 | ±443636 (same) |
| **MIN/MAX sqrt_price** | 4295048016 / 79226673515401279992447579055 | Same values |
| **Swap event struct** | SwapEvent (see above) | Identical field structure |
| **Pool enumeration** | LinkedTable in Pools factory | Table in PoolConfig + PoolTableId |

**Key takeaway**: Tick-to-price math is **identical**. Fee math uses same denominator. The differences are **structural** (type params, storage mechanism, function signatures) not mathematical. A **single Rust CLMM math library** works for both — only PTB construction and state deserialization differ.

---

## 5. Sui Transaction Mechanics

### Shared Object Ordering & Contention
- **Mysticeti** DAG-based consensus, 3-round commit latency
- Transactions on **same shared object ordered by gas price** (Priority Gas Auction)
- Competing txs are **deferred** (not failed immediately) when per-object capacity exceeded → re-sorted next commit
- After multiple deferrals: `ExecutionCancelledDueToSharedObjectCongestion`
- **SIP-45 gas amplification**: `5x RGP` eliminates ordering jitter; `100x RGP` gets next-round leader submission
- Other error: `ObjectVersionUnavailableForConsumption` (version already consumed)

### PTB Limits
| Limit | Value |
|-------|-------|
| Max commands | 1,024 |
| Max objects created | 2,048 |
| Max tx size | 128 KB |
| Max gas payment coins | 256 (gas smashing) |
| Max gas budget | 50 SUI (50,000,000,000 MIST) |
| Min gas budget | 2,000 MIST |
| Max computation units | 5,000,000 |

### Coin Splitting/Merging in PTBs
```
SplitCoins(Argument::GasCoin, vec![amount])  → creates new coin
MergeCoins(Argument::GasCoin, vec![coin])    → merges back
```

Result chaining: `Argument::Result(u16)` = shorthand for `NestedResult(idx, 0)`. For multi-return: `NestedResult(cmd_idx, tuple_idx)`. Results usable in **any** subsequent command.

### 3-Hop Swap PTB Pattern
```
cmd 0: SplitCoins(GasCoin, [amount])           → input coin
cmd 1: MoveCall swap_A (Result(0))             → output coin 1
cmd 2: MoveCall swap_B (Result(1))             → output coin 2
cmd 3: MoveCall swap_C (Result(2))             → output coin 3 (profit)
cmd 4: MergeCoins(GasCoin, [Result(3)])        → collect profit
```
All atomic — any failure reverts everything.

### Execution Latency
| Scenario | Latency |
|----------|---------|
| Owned objects only | < 500ms (fast path, no consensus) |
| Shared objects (DEX pools) | 2-3 seconds (consensus required) |
| Mysticeti consensus itself | ~500ms average |

- Use **`WaitForEffects`** (not `WaitForLocalExecution`) — it IS finality, lower latency
- `WaitForLocalExecution` additionally waits for local full node execution

### Parallel Transaction Submission
- Same gas coin in 2 txs = **equivocation** = both may fail
- **Solutions**:
  1. Pre-split gas coins into a pool, use one per concurrent tx
  2. Multiple sender addresses, each with own gas coins
  3. Sponsored transactions via `sui-gas-pool` (Redis-backed coin reservation)
  4. TypeScript SDK's `ParallelTransactionExecutor` (manages coin pools automatically)

### Event Subscription
- **`suix_subscribeEvent` is DEPRECATED** since July 2024, fully removed July 2026
- **Polling `suix_queryEvents`**: cursor-based pagination, most reliable, 3-7s latency. Filters: `MoveEventType`, `MoveModule`, `Sender`, `Transaction`, `TimeRange`, or combine with `Any`/`All`
- **gRPC `SubscribeCheckpoint`**: streaming, GA Sep-Oct 2025, ~2-5s checkpoint granularity. Proto defs at `github.com/MystenLabs/sui-apis`
- **For competitive arb: run your own full node** — lowest latency, no rate limits
- Most providers (Shinami, Triton, public Mysten) support JSON-RPC; gRPC being rolled out

---

## 6. Shio MEV Protocol

### Bundle Mechanics
- Max **5 transactions** per bundle
- All txs must have **identical gas price**
- All must write to **shared objects**
- No owned object conflicts
- Total tip >= **5% of combined gas budgets**
- Transactions must not be pre-executed

### Endpoints
| Mode | URL | Rate Limit | Cost |
|------|-----|------------|------|
| Default (MEV protection) | `https://rpc.getshio.com/boost` | 10 TPS/IP | Free (MEV kickbacks) |
| Fast (speed priority) | `https://rpc.getshio.com/fast` | 20 TPS/IP | 5% gas budget |
| Searcher Feed | `wss://rpc.getshio.com/feed` | 1 conn/IP | Free |
| JSON-RPC (bid/simulate) | `https://rpc.getshio.com` | — | — |

### Searcher Flow
1. Connect WebSocket to `wss://rpc.getshio.com/feed` (must respond to Ping with Pong or disconnect; max 1 conn/IP)
2. Receive `auctionStarted` events: `{ txDigest, gasPrice, deadlineTimestampMs, sideEffects: { events, createdObjects, mutatedObjects, gasUsage } }`
3. Build bid tx including MoveCall to `submit_bid`
4. Submit bid via WebSocket or JSON-RPC

### Shio Contract
- Package: `0x1889977f0fb56ae730e7bda8e8e32859ce78874458c74910d36121a81a615123`
- Module: `shio::auctioneer`
- Function: `submit_bid(s: &mut GlobalState, bid_amount: u64, fee: Balance<SUI>, ctx: &mut TxContext)`
- **32 shared GlobalState objects** — randomly choose one per submission to minimize congestion

### Bid Construction in PTB
```
split-coins gas [bid_amount]
move-call sui::coin::into_balance<sui::sui::SUI> split_result
move-call 0x1889...::auctioneer::submit_bid <random_global_state> bid_amount balance
```

### Bid Submission Format

**WebSocket:**
```json
{
  "oppTxDigest": "E72mG9GCroPgaw9uoeGiKLzAfd9CZq82iGDjypKdzYG7",
  "bidAmount": 42000000000,
  "txData": "<base64 BCS-serialized TransactionData>",
  "sig": "<base64 ED25519 signature>"
}
```

**JSON-RPC:**
```json
{
  "jsonrpc": "2.0", "id": 1,
  "method": "shio_submitBid",
  "params": ["<oppTxDigest>", 42000000000, "<txDataBase64>", "<sigBase64>"]
}
```

Other methods: `shio_simulateBid` (dry-run), `shio_tipPercentage`, `shio_auctionEvents`

### Bid Validation Rules
- Bid digest must be **lexicographically larger** than opportunity tx digest (binary comparison, not base58)
- Gas price must **exactly match** the opportunity transaction
- Must not lock objects already locked by opportunity tx
- Bundle [opportunity tx, bid] must pass dry-run

### Validator Coverage
**SIP-19 is protocol-level** — all validators running current Sui software support soft bundles. This is NOT opt-in like Jito on Solana.

**SIP-45 gas amplification** is also protocol-wide: tx with `n * RGP` gas price gets amplified through `n` validators. Factor 5 eliminates jitter; `100 * RGP` unlocks next-round leader submission.

### Latency
- Sui consensus: ~15 commits/second = ~67ms competitive windows (non-congested)
- Congested objects: windows extend to ~670ms (10x)
- Shio Default Mode auction window: **100ms**
- Shio bid submission window: **200-300ms**
- Future: Consensus Block Streaming SIP will reduce propagation delay to ~160ms

### No Rust SDK
Must implement WebSocket + JSON-RPC client manually. Only published SDK is `shio-fast-sdk` (npm, TypeScript).

### **Recommended hosting: Frankfurt, Germany**

---

## 7. Sui Rust SDK

### Three Options

**Option A: Legacy SDK (recommended for now — most complete)**
```toml
[dependencies]
sui-sdk = { git = "https://github.com/mystenlabs/sui", package = "sui-sdk" }
```
Known issue: `move-core-types` workspace resolution. May need full monorepo clone with `path =` deps.

**Option B: New Modular SDK (crates.io, cleaner)**
```toml
[dependencies]
sui-sdk-types = { version = "0.1", features = ["serde", "hash"] }
sui-crypto = { version = "0.1", features = ["ed25519"] }
sui-transaction-builder = "0.1"
sui-rpc = { version = "0.1", features = ["faucet"] }
```
Requires Rust 1.82+. WASM-compatible. Uses gRPC.

**Option C: GraphQL client**
```toml
[dependencies]
sui-graphql-client = "..."
```
Higher latency than JSON-RPC. Better for indexing, not MEV.

### Key Code Patterns

**SuiClient initialization:**
```rust
let sui = SuiClientBuilder::default()
    .ws_url("wss://fullnode.mainnet.sui.io:443")
    .build("https://fullnode.mainnet.sui.io:443")
    .await?;
```

**Building a ProgrammableTransaction:**
```rust
let mut ptb = ProgrammableTransactionBuilder::new();
let amount = ptb.pure(1000u64)?;  // MUST specify u64!
ptb.command(Command::SplitCoins(Argument::GasCoin, vec![amount]));
ptb.programmable_move_call(package_id, module, function, type_args, args);
let pt = ptb.finish();
let tx_data = TransactionData::new_programmable(sender, vec![gas_coin.object_ref()], pt, gas_budget, gas_price);
```

**Signing and submitting:**
```rust
let signature = keystore.sign_secure(&sender, &tx_data, Intent::sui_transaction()).await?;
let response = sui.quorum_driver_api()
    .execute_transaction_block(
        Transaction::from_data(tx_data, vec![signature]),
        SuiTransactionBlockResponseOptions::full_content(),
        Some(ExecuteTransactionRequestType::WaitForEffects),
    ).await?;
```

**Dry run:**
```rust
let result = sui.read_api()
    .dev_inspect_transaction_block(sender, tx_data, None, None, None)
    .await?;
```

**Object fetching:**
```rust
let objects = sui.read_api()
    .multi_get_object_with_options(
        vec![obj_id_1, obj_id_2],
        SuiObjectDataOptions::full_content(),
    ).await?;
```

**Event polling:**
```rust
let events = sui.event_api()
    .query_events(filter, cursor, limit, descending)
    .await?;
// Save cursor for next page
```

### BCS Deserialization
```rust
use serde::Deserialize;

#[derive(Deserialize)]
struct MyPool {
    id: ObjectID,
    reserve_x: u64,
    reserve_y: u64,
    fee_rate: u64,
}

let pool: MyPool = bcs::from_bytes(&raw_bcs_bytes)?;
```

Rules:
- BCS is NOT self-describing — field order must match exactly
- Move `address` = 32 bytes, `vector<T>` = length-prefixed, `u256` = 32 bytes LE
- Get raw BCS from object content via `SuiObjectDataOptions` with `bcs` enabled
- `af-sui-pkg-sdk` crate provides macros for auto-generating Rust types from Move

### Critical Gotchas
1. **Numeric types**: Always `1000u64` not `1000` — `VMVerificationOrDeserializationError`
2. **move-core-types**: Workspace resolution fails outside monorepo
3. **Protobuf conflicts**: Monorepo deps may clash
4. **Compile times**: Legacy SDK pulls entire Sui dep tree
5. **Rust version**: New SDK needs 1.82+

---

## 8. Pool State Deserialization

### Cetus Pool — Rust Struct
```rust
#[derive(Deserialize)]
struct CetusPool {
    id: ObjectID,
    coin_a: u64,              // Balance<A> → stored as u64
    coin_b: u64,              // Balance<B> → stored as u64
    tick_spacing: u32,
    fee_rate: u64,
    liquidity: u128,
    current_sqrt_price: u128, // Q64.64
    current_tick_index: u32,  // I32 stored as u32 (two's complement)
    fee_growth_global_a: u128,
    fee_growth_global_b: u128,
    fee_protocol_coin_a: u64,
    fee_protocol_coin_b: u64,
    // tick_manager, rewarder_manager, position_manager are nested — need custom deser
    // is_pause: bool, index: u64, url: String — after nested structs
}
```

### Turbos Pool — Rust Struct
```rust
#[derive(Deserialize)]
struct TurbosPool {
    id: ObjectID,
    coin_a: u64,
    coin_b: u64,
    protocol_fees_a: u64,
    protocol_fees_b: u64,
    sqrt_price: u128,           // Q64.64
    tick_current_index: u32,    // I32 as u32
    tick_spacing: u32,
    max_liquidity_per_tick: u128,
    fee: u32,
    fee_protocol: u32,
    unlocked: bool,
    fee_growth_global_a: u128,
    fee_growth_global_b: u128,
    liquidity: u128,
    // tick_map: Table — stored as UID, actual data in dynamic fields
    // remaining fields...
}
```

### Tick Data Fetching
- **Cetus**: `sui_getDynamicFields` on SkipList UID → enumerate nodes → read tick data
- **Turbos**: `sui_getDynamicFields` on Pool UID for individual ticks; bitmap separately from `tick_map` Table

### BCS vs JSON
- **BCS**: Faster, more compact, exact field-order struct mapping. Preferred for hot path.
- **JSON**: More forgiving, self-describing. Useful for debugging and initial development.
- **Recommendation**: Use BCS for pool state in production, JSON for exploratory work.

---

## 9. Token Types (Mainnet)

| Token | Type String | Decimals |
|-------|------------|----------|
| SUI | `0x2::sui::SUI` | 9 |
| USDC (native, Circle) | `0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC` | 6 |
| USDC (Wormhole) | `0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN` | 6 |
| USDT (Wormhole) | `0xc060006111016b8a020ad5b33834984a437aaa7d3c74c18e09a95d9c::coin::COIN` | 6 |
| WETH (Wormhole) | `0xaf8cd5edc19c4512f4259f0bee101a40d41ebed738ade5874359610ef8eeced5::coin::COIN` | 8 |
| wBTC (Wormhole) | `0x027792d9fed7f9844eb4839566001bb6f6cb4804f66aa2da6fe1ee242d896881::coin::COIN` | 8 |

Wormhole-bridged tokens all use `::coin::COIN` suffix. Native tokens use their own module name.

**Parsing type strings:**
```rust
use move_core_types::language_storage::TypeTag;
let tag = TypeTag::from_str("0x2::sui::SUI")?;
```

**Getting decimals:**
```rust
let metadata = sui.coin_read_api().get_coin_metadata(type_string).await?;
let decimals = metadata.unwrap().decimals;  // Cache this — immutable
```

**Discovering pool tokens**: Parse the pool object's type string to extract type parameters:
```rust
let struct_tag = StructTag::from_str(pool_type_str)?;
// struct_tag.type_params → Vec<TypeTag> = [CoinA, CoinB] or [CoinA, CoinB, FeeType]
```

---

## 10. Architecture Implications

### Must-Build Components
1. **Local CLMM tick math in Rust** — Q64.64 sqrt price, tick traversal, compute_swap_step. Same math for both Cetus and Turbos (identical constants and precision).
2. **Pool state manager** — deserialize pool objects + tick data via BCS, maintain in-memory state, refresh on events + periodic full fetch.
3. **Shio client** — WebSocket feed + JSON-RPC bid submission. No Rust SDK exists.
4. **Gas coin pool** — pre-split coins for parallel tx submission. Consider `sui-gas-pool` pattern.

### Can Reuse from sui-mev (as reference)
1. PTB construction patterns per DEX (Move call arguments, type params, object args)
2. Pool indexing via creation events + DashMap structure
3. Collector/Strategy/Executor architecture pattern (burberry)
4. Object handling patterns (`shared_obj_arg`, `MoveStruct::simple_deserialize`, type param extraction)

### Key Design Decisions
| Decision | Recommendation | Rationale |
|----------|---------------|-----------|
| Event source | Own full node | Polling = 3-7s delay. WebSocket deprecated. |
| Simulation | Local tick math + `devInspect` validation | sui-mev's simulation-only approach is too slow |
| MEV strategy | Shio for back-running, SIP-45 for blind arb | Both are protocol-level, no validator opt-in needed |
| Flash loans | Cetus flash_swap | Capital-free arb. Turbos also supports it. |
| SDK | Legacy sui-sdk (git dep) | Most complete API. New modular SDK is cleaner but thinner. |
| Hosting | Frankfurt, Germany | Optimized for Shio bid window (200-300ms) |

### Rust Constants for Implementation
```rust
// Cetus
const CETUS_CLMM_PACKAGE: &str = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";
const CETUS_CLMM_PUBLISHED_AT: &str = "0xc6faf3703b0e8ba9ed06b7851134bbbe7565eb35ff823fd78432baa4cbeaa12e";
const CETUS_GLOBAL_CONFIG: &str = "0xdaa46292632c3c4d8f31f23ea0f9b36a28ff3677e9684980e4438403a67a3d8f";
const CETUS_POOLS: &str = "0xf699e7f2276f5c9a75944b37a0c5b5d9ddfd2471bf6242483b03ab2887d198d0";

// Turbos
const TURBOS_CLMM_PACKAGE: &str = "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1";
const TURBOS_CLMM_PUBLISHED_AT: &str = "0xa5a0c25c79e428eba04fb98b3fb2a34db45ab26d4c8faf0d7e39d66a63891e64";
const TURBOS_VERSIONED: &str = "0xf1cf0e81048df168ebeb1b8030fad24b3e0b53ae827c25053fff0779c1445b6f";
const TURBOS_POOL_TABLE: &str = "0x08984ed8705f44b6403705dc248896e56ab7961447820ae29be935ce0d32198b";

// Shio
const SHIO_PACKAGE: &str = "0x1889977f0fb56ae730e7bda8e8e32859ce78874458c74910d36121a81a615123";

// Shared
const CLOCK: &str = "0x0000000000000000000000000000000000000000000000000000000000000006";
const FEE_RATE_DENOMINATOR: u64 = 1_000_000;
const MIN_SQRT_PRICE_X64: u128 = 4_295_048_016;
const MAX_SQRT_PRICE_X64: u128 = 79_226_673_515_401_279_992_447_579_055;
const TICK_BOUND: i32 = 443_636;
```

---

## 11. Open Questions (Can Only Answer by Running Code)

1. What is the actual current `published_at` address for Cetus/Turbos? (upgrades change it)
2. How many active Cetus/Turbos pools exist on mainnet?
3. What's the real-world tick density for major pairs (SUI/USDC)?
4. What latency do we actually get from Shio feed in Frankfurt?
5. Does the new modular Sui SDK support PTB construction well enough?
6. What's the gas cost of a 3-hop flash swap PTB?
7. How fast can we deserialize a full pool + all ticks via BCS?
8. What are the 32 Shio GlobalState object IDs?
9. What is the actual Shio auction latency end-to-end?
10. Can we run `calculate_swap_result` via `devInspect` fast enough for validation?

---

## Sources

- [fuzzland/sui-mev](https://github.com/fuzzland/sui-mev)
- [CetusProtocol/cetus-contracts](https://github.com/CetusProtocol/cetus-contracts)
- [CetusProtocol/cetus-clmm-interface](https://github.com/CetusProtocol/cetus-clmm-interface)
- [CetusProtocol/cetus-clmm-sui-sdk](https://github.com/CetusProtocol/cetus-clmm-sui-sdk)
- [Cetus Developer Docs](https://cetus-1.gitbook.io/cetus-developer-docs)
- [Turbos Finance Move Interface](https://github.com/turbos-finance/turbos-sui-move-interface)
- [Turbos CLMM SDK](https://github.com/turbos-finance/turbos-clmm-sdk)
- [Turbos Developer Docs](https://turbos.gitbook.io/turbos/developer-docs)
- [Sui Documentation](https://docs.sui.io)
- [Shio Documentation](https://docs.getshio.com)
- [MystenLabs/sui](https://github.com/MystenLabs/sui)
- [sui-gas-pool](https://github.com/MystenLabs/sui-gas-pool)
