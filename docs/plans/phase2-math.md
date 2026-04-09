# Phase 2: CLMM Math — Detailed Plan

## Goal

Implement a pure Rust CLMM math library (`clmm-math`) that simulates swaps locally in microseconds. This is the core competitive advantage over fuzzland/sui-mev which uses on-chain simulation for every price check. Once complete, the `estimate_swap()` placeholder in both DEX crates becomes functional, enabling Phase 3 (strategy) to evaluate arbitrage opportunities.

## What Phase 1 Provides

- `Tick { index: i32, liquidity_net: i128, liquidity_gross: u128, sqrt_price: u128 }` — normalized tick data, sorted by index ascending
- Ticks fetched from mainnet for both Cetus (SkipList) and Turbos (dynamic fields)
- `CetusPoolState` / `TurbosPoolState` with `sqrt_price: u128`, `tick_current: i32`, `liquidity: u128`, `fee_rate: u64`, `tick_spacing: u32`
- `SwapEstimate { token_in, token_out, amount_in, amount_out, fee_amount }` — return type for simulations
- Turbos ticks have `sqrt_price: 0` — must be computed by this crate

## Crate: `clmm-math`

**Location**: `crates/clmm-math/`
**Dependencies**: None (pure math, no async, no I/O, `#[no_std]`-compatible)
**Depended on by**: `dex-cetus`, `dex-turbos` (for `estimate_swap` implementation)

### Public API

```rust
// --- Constants ---
pub const FEE_RATE_DENOMINATOR: u64 = 1_000_000;
pub const MIN_SQRT_PRICE: u128 = 4_295_048_016;
pub const MAX_SQRT_PRICE: u128 = 79_226_673_515_401_279_992_447_579_055;
pub const MIN_TICK: i32 = -443_636;
pub const MAX_TICK: i32 = 443_636;

// --- Tick ↔ Sqrt Price Conversion ---
/// Convert tick index to Q64.64 sqrt price. Panics if tick out of bounds.
pub fn tick_to_sqrt_price(tick: i32) -> u128;

/// Convert Q64.64 sqrt price to tick index (floor).
pub fn sqrt_price_to_tick(sqrt_price: u128) -> i32;

// --- Single Swap Step (innermost hot loop) ---
pub struct SwapStepResult {
    pub sqrt_price_next: u128,
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_amount: u64,
}

/// Compute one swap step within a single tick range.
/// This is the performance-critical function — must be sub-microsecond.
pub fn compute_swap_step(
    sqrt_price_current: u128,
    sqrt_price_target: u128,
    liquidity: u128,
    amount_remaining: u64,
    fee_rate: u64,
) -> SwapStepResult;

// --- Full Multi-Tick Swap Simulation ---
pub struct SwapResult {
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_total: u64,
    pub sqrt_price_after: u128,
    pub tick_after: i32,
    pub liquidity_after: u128,
    pub steps: u32,           // number of tick crossings
    pub is_exceed: bool,      // true if pool liquidity exhausted
}

/// Simulate a full swap across multiple ticks.
/// `ticks` must be sorted by index ascending.
/// `a_to_b`: true = price decreases (sell token A), false = price increases (sell token B).
/// `amount`: input amount to swap.
pub fn simulate_swap(
    sqrt_price: u128,
    tick_current: i32,
    liquidity: u128,
    fee_rate: u64,
    tick_spacing: u32,
    ticks: &[Tick],           // from arb-types
    a_to_b: bool,
    amount: u64,
) -> SwapResult;

// --- Q64.64 Helpers (internal, but pub for testing) ---
pub fn get_amount_a_delta(
    sqrt_price_lower: u128,
    sqrt_price_upper: u128,
    liquidity: u128,
    round_up: bool,
) -> u64;

pub fn get_amount_b_delta(
    sqrt_price_lower: u128,
    sqrt_price_upper: u128,
    liquidity: u128,
    round_up: bool,
) -> u64;

pub fn get_next_sqrt_price_from_input(
    sqrt_price: u128,
    liquidity: u128,
    amount: u64,
    a_to_b: bool,
) -> u128;

pub fn get_next_sqrt_price_from_output(
    sqrt_price: u128,
    liquidity: u128,
    amount: u64,
    a_to_b: bool,
) -> u128;
```

### Implementation Source

Port from open-source Cetus Move contracts:
- `CetusProtocol/cetus-contracts` → `packages/cetus_clmm/sources/math/clmm_math.move`
- `CetusProtocol/cetus-contracts` → `packages/cetus_clmm/sources/math/tick_math.move`
- `CetusProtocol/integer-mate` → I32/I128 signed integer helpers

Key math details:
- **tick_to_sqrt_price**: Binary exponentiation with 18 precomputed ratio constants (same approach as Uniswap V3)
- **sqrt_price_to_tick**: log2 approximation with `BIT_PRECISION = 14`
- **compute_swap_step**: Fee deducted first (`amount * fee_rate / (1_000_000 - fee_rate)`), then compute price movement within tick range
- **simulate_swap**: Loop calling `compute_swap_step`, crossing ticks and updating liquidity via `liquidity_net`
- All intermediate math uses **u256** (via `ethnum::U256` or manual u128×u128→u256) for `mul_div` to avoid overflow

### Module Structure

```
crates/clmm-math/src/
├── lib.rs          — re-exports, constants
├── tick_math.rs    — tick_to_sqrt_price, sqrt_price_to_tick
├── swap_math.rs    — compute_swap_step, get_amount_a/b_delta, get_next_sqrt_price
├── simulate.rs     — simulate_swap (multi-tick loop)
└── math_u256.rs    — mul_div_round_up, mul_div_floor, checked_shl/shr
```

## Integration with DEX Crates

After `clmm-math` is built, wire it into the existing `estimate_swap()` stubs:

### `dex/cetus/src/lib.rs`
Replace the Phase 2 placeholder:
```rust
fn estimate_swap(&self, token_in: &CoinType, amount_in: u64) -> Result<SwapEstimate, ArbError> {
    let state = self.state.read().unwrap();
    let ticks = self.ticks.read().unwrap();
    let a_to_b = token_in == &self.coin_a;

    let result = clmm_math::simulate_swap(
        state.sqrt_price, state.tick_current, state.liquidity,
        state.fee_rate, state.tick_spacing, &ticks, a_to_b, amount_in,
    );

    Ok(SwapEstimate {
        token_in: token_in.clone(),
        token_out: if a_to_b { self.coin_b.clone() } else { self.coin_a.clone() },
        amount_in: result.amount_in,
        amount_out: result.amount_out,
        fee_amount: result.fee_total,
    })
}
```

### `dex/turbos/src/lib.rs`
Identical pattern — same `simulate_swap` call. The math is the same for both DEXes.

### `dex/turbos/src/ticks.rs`
Fill in `sqrt_price: 0` → `sqrt_price: clmm_math::tick_to_sqrt_price(tick_index)` during tick deserialization.

## Implementation Steps

1. **Create `clmm-math` crate** — Cargo.toml, lib.rs with constants and module declarations
2. **`math_u256.rs`** — `mul_div_floor`, `mul_div_round_up` using u128×u128→u256. Add `ethnum` dep or implement manually.
3. **`tick_math.rs`** — Port `tick_to_sqrt_price` and `sqrt_price_to_tick` from Cetus Move source. Include the 18 precomputed ratio constants.
4. **`swap_math.rs`** — Port `compute_swap_step`, `get_amount_a_delta`, `get_amount_b_delta`, `get_next_sqrt_price_from_input/output`
5. **`simulate.rs`** — `simulate_swap` loop: find next tick, call `compute_swap_step`, cross tick if needed, accumulate results
6. **Wire into `dex-cetus`** — Replace `estimate_swap` stub, add `clmm-math` dependency
7. **Wire into `dex-turbos`** — Replace `estimate_swap` stub, fix `sqrt_price: 0` in tick deser
8. **Unit tests** — Tick conversion round-trips, compute_swap_step edge cases, known swap vectors
9. **Mainnet verification** — Compare `simulate_swap` output against `devInspectTransactionBlock` for real pools (both Cetus AND Turbos)

## Test Plan

### Unit Tests (`clmm-math` crate)

**tick_math tests:**
- `tick_to_sqrt_price(0)` = 2^64 (price = 1.0)
- `tick_to_sqrt_price(MIN_TICK)` = `MIN_SQRT_PRICE`
- `tick_to_sqrt_price(MAX_TICK)` = `MAX_SQRT_PRICE`
- Round-trip: `sqrt_price_to_tick(tick_to_sqrt_price(t)) == t` for various t
- Negative ticks, boundary ticks, tick_spacing multiples

**swap_math tests:**
- `compute_swap_step` with exact amount that fills the range
- `compute_swap_step` with partial fill (amount < range capacity)
- `compute_swap_step` with zero liquidity → zero output
- `compute_swap_step` fee calculation matches `amount * fee / (1M - fee)`
- `get_amount_a_delta` / `get_amount_b_delta` consistency: delta_a at price range should match inverse

**simulate_swap tests:**
- Single-tick swap (no crossing)
- Multi-tick swap (2-3 crossings, verify liquidity changes)
- Swap that exhausts all liquidity (`is_exceed = true`)
- a_to_b vs b_to_a symmetry checks

### Mainnet Verification (`#[ignore]` integration tests)

**For BOTH Cetus AND Turbos** (per feedback — test every DEX):

1. Fetch a real pool + ticks from mainnet
2. Run local `simulate_swap` with a known amount
3. Run same swap via `devInspectTransactionBlock` (calling `calculate_swap_result` on-chain)
4. Compare: `amount_in`, `amount_out`, `fee_amount`, `after_sqrt_price` must match within rounding tolerance (±1)
5. Test multiple amounts: small (0.01 SUI), medium (10 SUI), large (1000 SUI)
6. Test both directions (a_to_b and b_to_a)

### Test Fixtures
- Save BCS snapshots of real pools + ticks to `tests/fixtures/` for deterministic unit tests
- Include pool state + tick array + expected swap results

## Verification Criteria

Phase 2 is complete when:
1. `cargo test -p clmm-math` passes all unit tests
2. `estimate_swap()` works on both Cetus and Turbos pools (no more "Phase 2" error)
3. Local simulation matches `devInspectTransactionBlock` within ±1 for both DEXes
4. `simulate_swap` benchmarks < 10 microseconds for typical pools
5. All existing Phase 1 tests still pass (`cargo test --workspace`)
