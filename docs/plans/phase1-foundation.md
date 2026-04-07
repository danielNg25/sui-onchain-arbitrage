# Phase 1: Foundation — Implementation Plan

## Context

Build the data layer for the Sui DEX arbitrage bot. No code exists yet — only documentation. This phase gets data flowing: fetch real Cetus/Turbos pool objects from mainnet, BCS-deserialize them into unified types, cache state, and fetch ticks. Everything downstream (math, strategy, execution) depends on this.

## Workspace Setup

Create root `Cargo.toml` with workspace members and shared dependency versions. Create `config/mainnet.toml` with all addresses from research docs.

```
Cargo.toml                          # workspace root
config/mainnet.toml                 # all package IDs, shared objects
crates/arb-types/                   # Step 1
crates/sui-client/                  # Step 2
crates/dex/common/                  # Step 3
crates/dex/cetus/                   # Step 4
crates/dex/turbos/                  # Step 5
crates/pool-manager/                # Step 6
```

Workspace dependencies:
- `serde`, `serde_json`, `bcs = "0.1"`, `toml = "0.8"`
- `tokio` (full), `reqwest` (json), `dashmap = "6"`
- `anyhow = "1"`, `thiserror = "2"`, `tracing = "0.1"`
- `async-trait = "0.1"`, `hex = "0.4"`, `base64 = "0.22"`

## Step 1: `arb-types` (zero I/O deps)

**Dependencies:** `serde`, `thiserror`, `toml` only.

### Public API

```rust
// pool.rs
pub type CoinType = Arc<str>;     // "0x2::sui::SUI"
pub type ObjectId = [u8; 32];

pub enum Dex { Cetus, Turbos }

pub struct PoolState {
    pub id: ObjectId,
    pub dex: Dex,
    pub coin_a: CoinType,
    pub coin_b: CoinType,
    pub sqrt_price: u128,         // Q64.64
    pub tick_current: i32,
    pub liquidity: u128,
    pub fee_rate: u64,            // PPM, denominator 1_000_000
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

pub fn object_id_from_hex(hex: &str) -> Result<ObjectId>;
pub fn object_id_to_hex(id: &ObjectId) -> String;

// tick.rs
pub struct Tick {
    pub index: i32,
    pub liquidity_net: i128,
    pub liquidity_gross: u128,
    pub sqrt_price: u128,
}

// config.rs — mirrors config/mainnet.toml
pub struct AppConfig {
    pub network: NetworkConfig,   // rpc_url
    pub cetus: CetusConfig,       // package_types, package_published_at, global_config, pools_registry
    pub turbos: TurbosConfig,     // package_types, package_published_at, swap_router_package, versioned, pool_table_id
    pub shio: ShioConfig,
    pub gas: GasConfig,
    pub strategy: StrategyConfig, // whitelisted_tokens, max_hops, etc.
}
impl AppConfig { pub fn load(path: &str) -> Result<Self>; }

// event.rs — shared swap event data for pool state updates
pub struct SwapEventData {
    pub pool_id: ObjectId,
    pub dex: Dex,
    pub a_to_b: bool,
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_amount: u64,
    pub after_sqrt_price: u128,
    pub vault_a_amount: u64,      // pool reserve_a after swap
    pub vault_b_amount: u64,      // pool reserve_b after swap
    pub steps: u64,               // tick crossings — if > 1, need tick refresh
}

// error.rs
pub enum ArbError {
    BcsDeserialize(String),
    Rpc(String),
    PoolNotFound(String),
    Config(String),
}
```

## Step 2: `sui-client` (thin JSON-RPC wrapper)

**Dependencies:** `reqwest`, `serde_json`, `arb-types`, `tokio`, `tracing`, `anyhow`, `base64`

**Design:** Raw `reqwest` + JSON-RPC 2.0. No `sui-sdk` git dep (workspace resolution issues). We only need ~9 RPC methods.

### Public API

```rust
pub struct SuiClient {
    http: reqwest::Client,
    rpc_url: String,
    request_id: AtomicU64,
}

impl SuiClient {
    pub fn new(rpc_url: &str) -> Self;

    pub async fn get_object(&self, id: &str, options: ObjectDataOptions) -> Result<SuiObjectResponse>;
    pub async fn multi_get_objects(&self, ids: &[String], options: ObjectDataOptions) -> Result<Vec<SuiObjectResponse>>;
    pub async fn get_dynamic_fields(&self, parent_id: &str, cursor: Option<String>, limit: Option<u32>) -> Result<DynamicFieldPage>;
    pub async fn get_dynamic_field_object(&self, parent_id: &str, name: &DynamicFieldName) -> Result<SuiObjectResponse>;
    pub async fn query_events(&self, filter: EventFilter, cursor: Option<String>, limit: Option<u32>, descending: bool) -> Result<EventPage>;
    pub async fn dev_inspect(&self, sender: &str, tx_bytes: &str) -> Result<DevInspectResults>;
    pub async fn execute_tx(&self, tx_bytes: &str, signature: &str, options: TxResponseOptions) -> Result<SuiTxResponse>;
    pub async fn get_reference_gas_price(&self) -> Result<u64>;
    pub async fn get_latest_checkpoint_sequence_number(&self) -> Result<u64>;
}
```

### RPC Response Types (minimal)

```rust
pub struct SuiObjectResponse { pub data: Option<SuiObjectData>, pub error: Option<Value> }
pub struct SuiObjectData {
    pub object_id: String,
    pub version: String,
    pub digest: String,
    pub type_: Option<String>,
    pub bcs: Option<SuiRawData>,
    pub owner: Option<Value>,        // extract initial_shared_version from here
    pub content: Option<Value>,
}
pub enum SuiRawData { MoveObject { bcs_bytes: String, type_: String, ... } }
pub struct ObjectDataOptions { pub show_bcs: bool, pub show_type: bool, pub show_owner: bool, ... }
pub struct DynamicFieldPage { pub data: Vec<DynamicFieldInfo>, pub next_cursor: Option<String>, pub has_next_page: bool }
pub struct EventPage { pub data: Vec<SuiEvent>, pub next_cursor: Option<Value>, pub has_next_page: bool }
```

**Key detail:** BCS bytes come back as **base64** in `SuiRawData::MoveObject::bcs_bytes`. Decode before passing to `bcs::from_bytes`.

## Step 3: `dex-common` (traits + helpers)

**Dependencies:** `arb-types`, `sui-client`, `async-trait`, `anyhow`

```rust
/// Deserialize BCS bytes into normalized PoolState
pub trait PoolDeserializer {
    fn deserialize_pool(
        object_id: ObjectId,
        bcs_bytes: &[u8],
        type_params: &[String],
        object_version: u64,
        initial_shared_version: u64,
    ) -> Result<PoolState, ArbError>;
}

/// Fetch ticks from on-chain dynamic fields
#[async_trait]
pub trait TickFetcher {
    async fn fetch_ticks(client: &SuiClient, pool: &PoolState) -> Result<Vec<Tick>, ArbError>;
}

/// Parse "0x...::pool::Pool<TypeA, TypeB>" → vec of type params
pub fn parse_type_params(type_string: &str) -> Vec<String>;

/// Parse "0x...::pool::Pool<A, B, Fee>" → (coin types, fee type)
pub fn parse_type_params_with_fee(type_string: &str) -> (Vec<String>, Option<String>);
```

## Step 4: `dex-cetus` (BCS deserialization)

**Dependencies:** `arb-types`, `dex-common`, `sui-client`, `bcs`, `serde`, `async-trait`, `tracing`, `anyhow`

### BCS Raw Structs (field order MUST match Move)

```rust
// Pool<CoinA, CoinB> — 2 type params
struct CetusPoolRaw {
    id: [u8; 32],                 // UID
    coin_a: u64,                  // Balance<A>
    coin_b: u64,                  // Balance<B>
    tick_spacing: u32,
    fee_rate: u64,
    liquidity: u128,
    current_sqrt_price: u128,     // Q64.64
    current_tick_index: CetusI32, // { bits: u32 }
    fee_growth_global_a: u128,
    fee_growth_global_b: u128,
    fee_protocol_coin_a: u64,
    fee_protocol_coin_b: u64,
    tick_manager: CetusTickManagerRaw,
    rewarder_manager: CetusRewarderManagerRaw,
    position_manager: CetusPositionManagerRaw,
    is_pause: bool,
    index: u64,
    url: String,
}

// TickManager → contains SkipList with ticks_table_id
struct CetusTickManagerRaw {
    tick_spacing: u32,
    ticks: CetusSkipListRaw,      // SkipList<Tick>
}

// SkipList — first field is the UID we need (ticks_table_id)
// Remaining fields are SkipList internals (head, tail, level, etc.)
// These MUST be fully defined to parse past them to reach is_pause
struct CetusSkipListRaw {
    id: [u8; 32],
    head: Vec<CetusOptionU64>,    // vector<OptionU64>
    tail: Vec<CetusOptionU64>,
    level: u64,
    max_level: u64,
    list_p: u64,
    size: u64,
}
```

**Risk:** SkipList, RewarderManager, PositionManager internal layouts are complex. Must validate against real BCS snapshot before finalizing structs.

**Mitigation strategy:**
1. First fetch raw BCS bytes of a real Cetus pool using `sui-client`
2. Hexdump and manually trace field boundaries
3. Iterate on struct definitions until deserialization succeeds
4. Save working BCS snapshot as test fixture

### Tick Fetching

Cetus ticks are dynamic fields on the SkipList UID. Each field is a SkipList node:

```rust
struct CetusSkipListNode {
    score: u64,                   // tick_index + 443636
    nexts: Vec<CetusOptionU64>,
    prev: CetusOptionU64,
    value: CetusTickRaw,
}

struct CetusTickRaw {
    index: CetusI32,
    sqrt_price: u128,
    liquidity_net: CetusI128,     // { bits: u128 }, cast to i128
    liquidity_gross: u128,
    fee_growth_outside_a: u128,
    fee_growth_outside_b: u128,
    points_growth_outside: u128,
    rewards_growth_outside: Vec<u128>,
}
```

Pagination: `get_dynamic_fields` on `ticks_table_id`, batch of 50, fetch each with `get_dynamic_field_object` (BCS).

### Implements

- `CetusDeserializer: PoolDeserializer` — BCS → PoolState
- `CetusTickFetcher: TickFetcher` — paginated dynamic field reads → sorted Vec<Tick>
- Event parsing for `SwapEvent` (type: `0x1eabed72...::pool::SwapEvent`)

## Step 5: `dex-turbos` (BCS deserialization)

**Dependencies:** same as dex-cetus

### BCS Raw Structs

```rust
// Pool<CoinA, CoinB, FeeType> — 3 type params
struct TurbosPoolRaw {
    id: [u8; 32],
    coin_a: u64,
    coin_b: u64,
    protocol_fees_a: u64,
    protocol_fees_b: u64,
    sqrt_price: u128,
    tick_current_index: TurbosI32,
    tick_spacing: u32,
    max_liquidity_per_tick: u128,
    fee: u32,
    fee_protocol: u32,
    unlocked: bool,
    fee_growth_global_a: u128,
    fee_growth_global_b: u128,
    liquidity: u128,
    tick_map: [u8; 32],           // Table UID
    deploy_time_ms: u64,
    reward_infos: Vec<TurbosRewardInfoRaw>,
    reward_last_updated_time_ms: u64,
}
```

**Key differences from Cetus:**
- 3rd type param = fee type (extract from type string, filter `fee` substring)
- `fee: u32` field — verify PPM normalization against known fee tiers
- `unlocked: bool` → `is_active = raw.unlocked` (inverse of Cetus `is_pause`)
- Turbos is closed source — struct layout inferred, must validate with real BCS

### Tick Fetching

Turbos ticks are individual dynamic fields on the Pool UID (not a nested SkipList). Tick map is a Table<I32, u256> bitmap — individual ticks fetched via dynamic fields.

### Implements

- `TurbosDeserializer: PoolDeserializer`
- `TurbosTickFetcher: TickFetcher`
- Event parsing for Turbos `SwapEvent` (identical struct to Cetus)

## Step 6: `pool-manager` (discovery + cache)

**Dependencies:** all above crates, `dashmap`, `tokio`

### Atomic Snapshot Design

All pool data must be fetched relative to a known checkpoint so event-based sync has no gaps:

1. **Before discovery:** call `get_latest_checkpoint_sequence_number()` → record as `snapshot_checkpoint`
2. **Fetch all pools** as fast as possible (concurrent batches)
3. **Fetch all ticks** for discovered pools
4. **Store `snapshot_checkpoint`** — downstream event polling starts from this checkpoint
5. Any swap events from `snapshot_checkpoint` onward will update pool state, covering any changes that occurred during the fetch window

This ensures: `pool_state_at(snapshot) + events_from(snapshot..now) = current_state`

### Public API

```rust
pub struct PoolManager {
    client: Arc<SuiClient>,
    config: Arc<AppConfig>,
    pools: DashMap<ObjectId, Arc<PoolState>>,
    tick_cache: DashMap<ObjectId, Arc<Vec<Tick>>>,
    token_to_pools: DashMap<CoinType, HashSet<ObjectId>>,
    pair_to_pools: DashMap<(CoinType, CoinType), HashSet<ObjectId>>,
    /// Checkpoint at which pool state was snapshotted.
    /// Event consumers should start polling from this checkpoint.
    snapshot_checkpoint: AtomicU64,
}

impl PoolManager {
    pub fn new(client: Arc<SuiClient>, config: Arc<AppConfig>) -> Self;

    /// Full discovery: record checkpoint, enumerate registries, fetch pools + ticks, build indexes.
    /// Returns the snapshot checkpoint number for event sync.
    pub async fn discover_all_pools(&self) -> Result<u64>;

    pub async fn fetch_ticks(&self, pool_id: &ObjectId) -> Result<Arc<Vec<Tick>>>;
    pub fn get_pool(&self, id: &ObjectId) -> Option<Arc<PoolState>>;
    pub fn get_pools_for_pair(&self, a: &CoinType, b: &CoinType) -> Vec<Arc<PoolState>>;
    pub fn get_pools_for_token(&self, token: &CoinType) -> Vec<ObjectId>;
    pub fn pool_count(&self) -> usize;
    pub fn snapshot_checkpoint(&self) -> u64;

    /// Update a pool's state from a swap event (sqrt_price, reserves, tick).
    /// If event crossed ticks (steps > 1), marks pool for tick refresh.
    pub fn update_from_event(&self, pool_id: &ObjectId, event: &SwapEventData);
}
```

### Discovery Flow

1. **Record checkpoint:** `snapshot_checkpoint = client.get_latest_checkpoint_sequence_number()`
2. **Cetus:** Paginate `getDynamicFields` on pools_registry (`0xf699e7...`) → collect pool IDs → `multi_get_objects` (batches of 50, BCS) → `CetusDeserializer::deserialize_pool` → filter active + whitelisted tokens
3. **Turbos:** Same via pool_table_id (`0x08984e...`) → `TurbosDeserializer::deserialize_pool`
4. Build indexes: `token_to_pools`, `pair_to_pools`
5. Fetch ticks for each pool (paginated, can be parallelized with `tokio::join!`)
6. Return `snapshot_checkpoint` for event sync

## Step 7: Verification

Integration test in `pool-manager` (marked `#[ignore]`):

```rust
#[tokio::test]
#[ignore]
async fn fetch_and_print_sui_usdc_pool() {
    // 1. Load config from config/mainnet.toml
    // 2. Create SuiClient
    // 3. Fetch known Cetus SUI/USDC pool by ID
    // 4. BCS deserialize → PoolState
    // 5. Print: dex, coins, sqrt_price, tick, liquidity, fee_rate, reserves, is_active
    // 6. Fetch ticks → print count + first/last tick
    // 7. Assert: is_active, liquidity > 0, sqrt_price > 0
}
```

Also test full discovery:
```rust
#[tokio::test]
#[ignore]
async fn discover_all_pools_and_print_summary() {
    // Run discover_all_pools, print total count by DEX
    // Verify at least some pools found for each DEX
}
```

## Implementation Order

| # | Crate | Depends On | Key Risk |
|---|-------|-----------|----------|
| 0 | Workspace + config | — | — |
| 1 | `arb-types` | — | None |
| 2 | `sui-client` | `arb-types` | RPC response format edge cases |
| 3 | `dex/common` | `arb-types`, `sui-client` | None |
| 4 | `dex/cetus` | `arb-types`, `dex-common`, `sui-client`, `bcs` | **HIGH: BCS struct layout for nested SkipList/RewarderManager/PositionManager** |
| 5 | `dex/turbos` | same | **HIGH: closed source, inferred struct layout** |
| 6 | `pool-manager` | all above | Registry enumeration format |
| 7 | Verification | all above | — |

**BCS validation strategy for steps 4-5:**
1. Build `sui-client` first
2. Write a small test that fetches raw BCS bytes of a known pool
3. Hexdump and manually trace field boundaries
4. Define raw structs to match
5. Save validated BCS bytes as `tests/fixtures/` for offline unit tests

## Test Plan

### Unit Tests (offline, fast)
- `arb-types`: hex conversion roundtrips, config loading from test TOML string
- `dex-common`: `parse_type_params` for Cetus 2-param and Turbos 3-param type strings
- `dex-cetus`: BCS deserialization from fixture file → verify all fields
- `dex-turbos`: BCS deserialization from fixture file → verify all fields
- `pool-manager`: index insert/query operations

### Integration Tests (`#[ignore]`, need network)
- Fetch known Cetus SUI/USDC pool, deserialize, verify reasonable values
- Fetch known Turbos SUI/USDC pool, deserialize, verify
- Fetch ticks for a Cetus pool, verify sorted + non-empty
- Fetch ticks for a Turbos pool, verify sorted + non-empty
- Full pool discovery (both DEXes), verify counts > 0

### Fixture Generation
After first successful integration test, save BCS bytes to `tests/fixtures/`:
- `cetus_sui_usdc_pool.bcs`
- `turbos_sui_usdc_pool.bcs`
- `cetus_tick_node.bcs`
- `turbos_tick.bcs`

## Files to Create

```
Cargo.toml
config/mainnet.toml
crates/arb-types/Cargo.toml
crates/arb-types/src/lib.rs
crates/arb-types/src/pool.rs
crates/arb-types/src/tick.rs
crates/arb-types/src/config.rs
crates/arb-types/src/error.rs
crates/sui-client/Cargo.toml
crates/sui-client/src/lib.rs
crates/sui-client/src/client.rs
crates/sui-client/src/types.rs
crates/sui-client/src/error.rs
crates/dex/common/Cargo.toml
crates/dex/common/src/lib.rs
crates/dex/cetus/Cargo.toml
crates/dex/cetus/src/lib.rs
crates/dex/cetus/src/raw.rs
crates/dex/cetus/src/ticks.rs
crates/dex/cetus/src/events.rs
crates/dex/turbos/Cargo.toml
crates/dex/turbos/src/lib.rs
crates/dex/turbos/src/raw.rs
crates/dex/turbos/src/ticks.rs
crates/dex/turbos/src/events.rs
crates/pool-manager/Cargo.toml
crates/pool-manager/src/lib.rs
crates/pool-manager/src/discovery.rs
tests/fixtures/                     # populated during integration tests
```
