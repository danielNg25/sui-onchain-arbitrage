pub mod events;
pub mod raw;
mod ticks;

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use dashmap::DashMap;
use tracing::{debug, info, warn};

use arb_types::error::ArbError;
use arb_types::event::SwapEstimate;
use arb_types::pool::{object_id_from_hex, CoinType, Dex, ObjectId};
use arb_types::tick::Tick;
use dex_common::{parse_type_params, DexRegistry, Pool};
use sui_client::{ObjectDataOptions, SuiClient};

/// Cetus CLMM swap event type string.
pub const CETUS_SWAP_EVENT_TYPE: &str =
    "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::pool::SwapEvent";

/// Cetus pool creation event type string.
pub const CETUS_CREATE_POOL_EVENT_TYPE: &str =
    "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::factory::CreatePoolEvent";

pub const CETUS_ADD_LIQUIDITY_EVENT_TYPE: &str =
    "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::pool::AddLiquidityEvent";

pub const CETUS_REMOVE_LIQUIDITY_EVENT_TYPE: &str =
    "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::pool::RemoveLiquidityEvent";

const CETUS_EVENT_TYPES: &[&str] = &[
    CETUS_SWAP_EVENT_TYPE,
    CETUS_ADD_LIQUIDITY_EVENT_TYPE,
    CETUS_REMOVE_LIQUIDITY_EVENT_TYPE,
];

// ---------------------------------------------------------------------------
// CetusPool — internal CLMM state, implements Pool trait
// ---------------------------------------------------------------------------

/// Internal CLMM state for a Cetus pool.
#[allow(dead_code)]
pub(crate) struct CetusPoolState {
    pub sqrt_price: u128,
    pub tick_current: i32,
    pub liquidity: u128,
    pub fee_rate: u64,
    pub tick_spacing: u32,
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub is_active: bool,
    pub ticks_table_id: ObjectId,
    pub initial_shared_version: u64,
    pub object_version: u64,
}

pub struct CetusPool {
    id: ObjectId,
    coin_a: CoinType,
    coin_b: CoinType,
    state: RwLock<CetusPoolState>,
    ticks: RwLock<Vec<Tick>>,
}

#[async_trait::async_trait]
impl Pool for CetusPool {
    fn id(&self) -> ObjectId {
        self.id
    }

    fn dex(&self) -> Dex {
        Dex::Cetus
    }

    fn coins(&self) -> Vec<CoinType> {
        vec![self.coin_a.clone(), self.coin_b.clone()]
    }

    fn is_active(&self) -> bool {
        self.state.read().unwrap().is_active
    }

    fn fee_rate(&self) -> u64 {
        self.state.read().unwrap().fee_rate
    }

    async fn fetch_price_data(&self, client: &SuiClient) -> Result<(), ArbError> {
        let ticks_table_id = self.state.read().unwrap().ticks_table_id;
        let new_ticks = ticks::fetch_cetus_ticks(client, &ticks_table_id, &self.id).await?;
        *self.ticks.write().unwrap() = new_ticks;
        Ok(())
    }

    fn apply_event(
        &self,
        event_type: &str,
        parsed_json: &serde_json::Value,
    ) -> Result<Option<bool>, ArbError> {
        // Check if event is for our pool
        let pool_id_str = match parsed_json["pool"].as_str() {
            Some(s) => s,
            None => return Ok(None),
        };
        let event_pool_id = object_id_from_hex(pool_id_str)?;
        if event_pool_id != self.id {
            return Ok(None);
        }

        match event_type {
            CETUS_SWAP_EVENT_TYPE => self.apply_swap_event(parsed_json),
            CETUS_ADD_LIQUIDITY_EVENT_TYPE => self.apply_liquidity_event(parsed_json, true),
            CETUS_REMOVE_LIQUIDITY_EVENT_TYPE => self.apply_liquidity_event(parsed_json, false),
            _ => Ok(None),
        }
    }

    fn estimate_swap(
        &self,
        token_in: &CoinType,
        amount_in: u64,
    ) -> Result<SwapEstimate, ArbError> {
        let state = self.state.read().unwrap();
        let ticks = self.ticks.read().unwrap();
        let a_to_b = token_in == &self.coin_a;

        let result = clmm_math::simulate_swap(
            state.sqrt_price,
            state.tick_current,
            state.liquidity,
            state.fee_rate,
            state.tick_spacing,
            &ticks,
            a_to_b,
            amount_in,
        );

        Ok(SwapEstimate {
            token_in: token_in.clone(),
            token_out: if a_to_b {
                self.coin_b.clone()
            } else {
                self.coin_a.clone()
            },
            amount_in: result.amount_in,
            amount_out: result.amount_out,
            fee_amount: result.fee_total,
        })
    }
}

impl CetusPool {
    fn apply_swap_event(
        &self,
        json: &serde_json::Value,
    ) -> Result<Option<bool>, ArbError> {
        let after_sqrt_price = events::parse_u128_field(json, "after_sqrt_price")?;
        let before_sqrt_price = events::parse_u128_field(json, "before_sqrt_price")?;
        let vault_a = events::parse_u64_field(json, "vault_a_amount")?;
        let vault_b = events::parse_u64_field(json, "vault_b_amount")?;
        let steps = events::parse_u64_field(json, "steps")?;
        let a_to_b = json["atob"].as_bool().unwrap_or(true);

        let mut state = self.state.write().unwrap();
        state.sqrt_price = after_sqrt_price;
        state.reserve_a = vault_a;
        state.reserve_b = vault_b;

        // Update tick_current and liquidity by walking crossed ticks
        if steps > 1 {
            let ticks = self.ticks.read().unwrap();
            let (new_tick, new_liquidity) = walk_crossed_ticks(
                &ticks,
                state.liquidity,
                before_sqrt_price,
                after_sqrt_price,
                a_to_b,
            );
            state.tick_current = new_tick;
            state.liquidity = new_liquidity;
        } else {
            // Single step — find tick for after_sqrt_price
            let ticks = self.ticks.read().unwrap();
            state.tick_current = find_tick_for_sqrt_price(&ticks, after_sqrt_price);
        }

        Ok(Some(steps > 1))
    }

    fn apply_liquidity_event(
        &self,
        json: &serde_json::Value,
        is_add: bool,
    ) -> Result<Option<bool>, ArbError> {
        let tick_lower = events::parse_i32_field(json, "tick_lower")?;
        let tick_upper = events::parse_i32_field(json, "tick_upper")?;
        let liquidity_delta = events::parse_u128_field(json, "liquidity")?;
        let after_liquidity = events::parse_u128_field(json, "after_liquidity")?;
        let amount_a = events::parse_u64_field(json, "amount_a")?;
        let amount_b = events::parse_u64_field(json, "amount_b")?;

        let mut state = self.state.write().unwrap();
        state.liquidity = after_liquidity;

        // Update reserves
        if is_add {
            state.reserve_a += amount_a;
            state.reserve_b += amount_b;
        } else {
            state.reserve_a = state.reserve_a.saturating_sub(amount_a);
            state.reserve_b = state.reserve_b.saturating_sub(amount_b);
        }

        let mut ticks = self.ticks.write().unwrap();
        let signed_delta = if is_add {
            liquidity_delta as i128
        } else {
            -(liquidity_delta as i128)
        };
        apply_liquidity_to_ticks(&mut ticks, tick_lower, tick_upper, signed_delta);

        Ok(Some(false))
    }
}

// ---------------------------------------------------------------------------
// CetusRegistry — DEX-level pool discovery and management
// ---------------------------------------------------------------------------

pub struct CetusRegistry {
    package_types: String,
    pools: DashMap<ObjectId, Arc<CetusPool>>,
    token_index: DashMap<CoinType, HashSet<ObjectId>>,
}

impl CetusRegistry {
    pub fn new(config: &arb_types::config::CetusConfig) -> Self {
        Self {
            package_types: config.package_types.clone(),
            pools: DashMap::new(),
            token_index: DashMap::new(),
        }
    }

    fn index_pool(&self, pool_id: ObjectId, coin_a: &CoinType, coin_b: &CoinType) {
        self.token_index
            .entry(coin_a.clone())
            .or_default()
            .insert(pool_id);
        self.token_index
            .entry(coin_b.clone())
            .or_default()
            .insert(pool_id);
    }
}

#[async_trait::async_trait]
impl DexRegistry for CetusRegistry {
    fn dex(&self) -> Dex {
        Dex::Cetus
    }

    fn event_types(&self) -> &[&str] {
        CETUS_EVENT_TYPES
    }

    fn matches_pool_type(&self, type_string: &str) -> bool {
        type_string.contains(&format!("{}::pool::Pool", self.package_types))
    }

    async fn discover_pools(
        &self,
        client: &SuiClient,
        whitelisted_tokens: &HashSet<String>,
    ) -> Result<Vec<(ObjectId, CoinType, CoinType)>, ArbError> {
        // Step 1: Query all CreatePoolEvent to collect pool IDs
        let mut pool_obj_ids = Vec::new();
        let mut cursor = None;

        loop {
            let events = client
                .query_events(
                    sui_client::EventFilter::MoveEventType(
                        CETUS_CREATE_POOL_EVENT_TYPE.to_string(),
                    ),
                    cursor,
                    Some(200),
                    false,
                )
                .await
                .map_err(|e| ArbError::Rpc(format!("query Cetus CreatePoolEvent: {}", e)))?;

            for event in &events.data {
                if let Some(json) = &event.parsed_json {
                    if let Some(pool_id) = json["pool_id"].as_str() {
                        pool_obj_ids.push(pool_id.to_string());
                    }
                }
            }

            if !events.has_next_page {
                break;
            }
            cursor = events.next_cursor;
        }

        debug!(count = pool_obj_ids.len(), "found Cetus pools via CreatePoolEvent");

        // Step 2: Batch-fetch pool objects with BCS, filter by whitelist
        let mut results = Vec::new();
        for chunk in pool_obj_ids.chunks(50) {
            let objects = client
                .multi_get_objects(chunk, ObjectDataOptions::bcs())
                .await
                .map_err(|e| ArbError::Rpc(format!("batch fetch Cetus pools: {}", e)))?;

            for obj_resp in &objects {
                let Some(data) = &obj_resp.data else { continue };

                let type_str = match data.bcs_type() {
                    Some(t) if self.matches_pool_type(t) => t,
                    _ => continue,
                };

                let type_params = parse_type_params(type_str);
                if type_params.len() < 2 {
                    continue;
                }

                if !whitelisted_tokens.is_empty()
                    && !whitelisted_tokens.contains(&type_params[0])
                    && !whitelisted_tokens.contains(&type_params[1])
                {
                    continue;
                }

                let bcs_bytes = match data.bcs_bytes() {
                    Ok(b) => b,
                    Err(e) => {
                        warn!("skip Cetus pool {}: {}", data.object_id, e);
                        continue;
                    }
                };

                let object_id = match object_id_from_hex(&data.object_id) {
                    Ok(id) => id,
                    Err(e) => {
                        warn!("skip Cetus pool {}: {}", data.object_id, e);
                        continue;
                    }
                };

                match self.ingest_pool_object(
                    object_id,
                    &bcs_bytes,
                    &type_params,
                    data.version_number(),
                    data.initial_shared_version().unwrap_or(0),
                ) {
                    Ok(Some((id, coin_a, coin_b))) => {
                        results.push((id, coin_a, coin_b));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        debug!("skip Cetus pool {} deser: {}", data.object_id, e);
                    }
                }
            }
        }

        info!(count = results.len(), "discovered Cetus pools");
        Ok(results)
    }

    fn ingest_pool_object(
        &self,
        object_id: ObjectId,
        bcs_bytes: &[u8],
        type_params: &[String],
        object_version: u64,
        initial_shared_version: u64,
    ) -> Result<Option<(ObjectId, CoinType, CoinType)>, ArbError> {
        if type_params.len() < 2 {
            return Err(ArbError::InvalidData(format!(
                "Cetus pool requires 2 type params, got {}",
                type_params.len()
            )));
        }

        let raw = raw::parse_cetus_pool(bcs_bytes)?;

        if raw.is_pause {
            return Ok(None);
        }

        let coin_a: CoinType = Arc::from(type_params[0].as_str());
        let coin_b: CoinType = Arc::from(type_params[1].as_str());

        let pool = Arc::new(CetusPool {
            id: object_id,
            coin_a: coin_a.clone(),
            coin_b: coin_b.clone(),
            state: RwLock::new(CetusPoolState {
                sqrt_price: raw.current_sqrt_price,
                tick_current: raw.current_tick_index,
                liquidity: raw.liquidity,
                fee_rate: raw.fee_rate,
                tick_spacing: raw.tick_spacing,
                reserve_a: raw.coin_a,
                reserve_b: raw.coin_b,
                is_active: !raw.is_pause,
                ticks_table_id: raw.ticks_table_id,
                initial_shared_version,
                object_version,
            }),
            ticks: RwLock::new(Vec::new()),
        });

        self.index_pool(object_id, &coin_a, &coin_b);
        self.pools.insert(object_id, pool);

        Ok(Some((object_id, coin_a, coin_b)))
    }

    fn pool(&self, pool_id: &ObjectId) -> Option<Arc<dyn Pool>> {
        self.pools
            .get(pool_id)
            .map(|entry| entry.value().clone() as Arc<dyn Pool>)
    }

    fn pool_ids(&self) -> Vec<ObjectId> {
        self.pools.iter().map(|entry| *entry.key()).collect()
    }

    fn pools_for_token(&self, token: &CoinType) -> Vec<ObjectId> {
        self.token_index
            .get(token)
            .map(|ids| ids.iter().copied().collect())
            .unwrap_or_default()
    }

    fn pool_count(&self) -> usize {
        self.pools.len()
    }
}

/// Get sqrt_price for a pool (for testing/verification).
pub fn get_pool_sqrt_price(registry: &CetusRegistry, pool_id: &ObjectId) -> Option<u128> {
    registry
        .pools
        .get(pool_id)
        .map(|p| p.state.read().unwrap().sqrt_price)
}

/// Get the internal ticks snapshot for a pool (for testing/verification).
pub fn get_pool_ticks(registry: &CetusRegistry, pool_id: &ObjectId) -> Option<Vec<Tick>> {
    registry
        .pools
        .get(pool_id)
        .map(|p| p.ticks.read().unwrap().clone())
}

/// Get reserve_a and reserve_b for a pool (for testing/verification).
pub fn get_pool_reserves(registry: &CetusRegistry, pool_id: &ObjectId) -> Option<(u64, u64)> {
    registry.pools.get(pool_id).map(|p| {
        let s = p.state.read().unwrap();
        (s.reserve_a, s.reserve_b)
    })
}

/// Fetch ticks for a pool (for testing/verification).
pub async fn fetch_ticks_for_pool(
    client: &SuiClient,
    registry: &CetusRegistry,
    pool_id: &ObjectId,
) -> Result<Vec<Tick>, ArbError> {
    let pool = registry.pools.get(pool_id).ok_or_else(|| {
        ArbError::PoolNotFound(arb_types::pool::object_id_to_hex(pool_id))
    })?;
    let ticks_table_id = pool.state.read().unwrap().ticks_table_id;
    ticks::fetch_cetus_ticks(client, &ticks_table_id, pool_id).await
}

/// Check if a Cetus pool is paused from JSON content.
pub fn is_pool_paused(content: &serde_json::Value) -> bool {
    content
        .get("fields")
        .and_then(|f| f.get("is_pause"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tick helpers for event application
// ---------------------------------------------------------------------------

/// Find the tick index for a given sqrt_price by binary searching the ticks array.
/// Returns the largest tick index whose sqrt_price <= the target.
fn find_tick_for_sqrt_price(ticks: &[Tick], sqrt_price: u128) -> i32 {
    if ticks.is_empty() {
        return 0;
    }
    match ticks.binary_search_by_key(&sqrt_price, |t| t.sqrt_price) {
        Ok(i) => ticks[i].index,
        Err(0) => ticks[0].index,
        Err(i) if i >= ticks.len() => ticks[ticks.len() - 1].index,
        Err(i) => ticks[i - 1].index,
    }
}

/// Walk through crossed ticks between before and after sqrt_price,
/// updating liquidity as each tick is crossed.
/// Returns (new_tick_current, new_liquidity).
fn walk_crossed_ticks(
    ticks: &[Tick],
    mut liquidity: u128,
    before_sqrt_price: u128,
    after_sqrt_price: u128,
    a_to_b: bool,
) -> (i32, u128) {
    if ticks.is_empty() {
        return (0, liquidity);
    }

    let (price_lo, price_hi) = if a_to_b {
        (after_sqrt_price, before_sqrt_price)
    } else {
        (before_sqrt_price, after_sqrt_price)
    };

    // Find ticks in the crossed range
    for tick in ticks {
        if tick.sqrt_price == 0 {
            continue; // Turbos ticks may not have sqrt_price yet
        }
        if tick.sqrt_price > price_lo && tick.sqrt_price <= price_hi {
            // This tick was crossed
            if a_to_b {
                // Crossing downward: subtract liquidity_net
                liquidity = (liquidity as i128 - tick.liquidity_net) as u128;
            } else {
                // Crossing upward: add liquidity_net
                liquidity = (liquidity as i128 + tick.liquidity_net) as u128;
            }
        }
    }

    let new_tick = find_tick_for_sqrt_price(ticks, after_sqrt_price);
    (new_tick, liquidity)
}

/// Apply a liquidity delta to the ticks array.
/// Updates liquidity_net and liquidity_gross at tick_lower and tick_upper.
/// Inserts new ticks if they don't exist, removes if liquidity_gross reaches 0.
/// Apply liquidity change to ticks.
/// `signed_delta`: positive for add, negative for remove.
/// Lower tick gets +delta to liquidity_net, upper gets -delta.
/// Both get +abs(delta) to liquidity_gross (add) or -abs(delta) (remove).
fn apply_liquidity_to_ticks(
    ticks: &mut Vec<Tick>,
    tick_lower: i32,
    tick_upper: i32,
    signed_delta: i128,
) {
    // gross_delta: positive when adding liquidity, negative when removing
    let gross_delta = signed_delta;

    // Lower tick: liquidity_net increases by delta
    apply_delta_to_tick(ticks, tick_lower, signed_delta, gross_delta);
    // Upper tick: liquidity_net decreases by delta (opposite direction)
    apply_delta_to_tick(ticks, tick_upper, -signed_delta, gross_delta);

    // Remove ticks with zero liquidity_gross
    ticks.retain(|t| t.liquidity_gross > 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_ticks() -> Vec<Tick> {
        vec![
            Tick { index: -100, liquidity_net: 1000, liquidity_gross: 1000, sqrt_price: 100 },
            Tick { index: 0, liquidity_net: -500, liquidity_gross: 500, sqrt_price: 200 },
            Tick { index: 100, liquidity_net: -500, liquidity_gross: 500, sqrt_price: 300 },
        ]
    }

    #[test]
    fn test_find_tick_for_sqrt_price() {
        let ticks = make_test_ticks();
        assert_eq!(find_tick_for_sqrt_price(&ticks, 50), -100);
        assert_eq!(find_tick_for_sqrt_price(&ticks, 150), -100);
        assert_eq!(find_tick_for_sqrt_price(&ticks, 200), 0);
        assert_eq!(find_tick_for_sqrt_price(&ticks, 250), 0);
        assert_eq!(find_tick_for_sqrt_price(&ticks, 350), 100);
    }

    #[test]
    fn test_apply_liquidity_add() {
        let mut ticks = make_test_ticks();
        // Add liquidity in range [-100, 0]
        apply_liquidity_to_ticks(&mut ticks, -100, 0, 500);

        // tick -100: liquidity_net += 500 → 1500, liquidity_gross += 500 → 1500
        let t_neg100 = ticks.iter().find(|t| t.index == -100).unwrap();
        assert_eq!(t_neg100.liquidity_net, 1500);
        assert_eq!(t_neg100.liquidity_gross, 1500);

        // tick 0: liquidity_net -= 500 → -1000, liquidity_gross += 500 → 1000
        let t0 = ticks.iter().find(|t| t.index == 0).unwrap();
        assert_eq!(t0.liquidity_net, -1000);
        assert_eq!(t0.liquidity_gross, 1000);
    }

    #[test]
    fn test_apply_liquidity_remove_deletes_tick() {
        let mut ticks = make_test_ticks();
        // Remove liquidity with delta=-500 in range [0, 100]
        // Both tick 0 (gross=500) and tick 100 (gross=500) get gross_delta=-500 → 0 → removed
        apply_liquidity_to_ticks(&mut ticks, 0, 100, -500);

        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].index, -100); // only tick -100 survives
    }

    #[test]
    fn test_apply_liquidity_creates_new_tick() {
        let mut ticks = make_test_ticks();
        apply_liquidity_to_ticks(&mut ticks, 50, 150, 200);

        // Should have inserted tick at index 50 and 150
        assert_eq!(ticks.len(), 5);
        let t50 = ticks.iter().find(|t| t.index == 50).unwrap();
        assert_eq!(t50.liquidity_net, 200);
        assert_eq!(t50.liquidity_gross, 200);

        let t150 = ticks.iter().find(|t| t.index == 150).unwrap();
        assert_eq!(t150.liquidity_net, -200);
        assert_eq!(t150.liquidity_gross, 200);
    }

    #[test]
    fn test_walk_crossed_ticks_a_to_b() {
        let ticks = make_test_ticks();
        // Price going from 250 down to 50 (a_to_b = true)
        // Crosses tick at sqrt_price=200 (index 0) and sqrt_price=100 (index -100)
        let (new_tick, new_liq) = walk_crossed_ticks(
            &ticks, 1000, 250, 50, true,
        );
        // Crossing tick 0 (liq_net=-500): 1000 - (-500) = 1500
        // Crossing tick -100 (liq_net=1000): 1500 - 1000 = 500
        assert_eq!(new_liq, 500);
        assert_eq!(new_tick, -100);
    }
}

/// Apply a liquidity_net delta to a single tick.
/// `net_delta` affects liquidity_net (can be positive or negative).
/// `gross_delta` is always the absolute liquidity change (positive for add, negative for remove).
fn apply_delta_to_tick(ticks: &mut Vec<Tick>, tick_index: i32, net_delta: i128, gross_delta: i128) {
    match ticks.binary_search_by_key(&tick_index, |t| t.index) {
        Ok(i) => {
            ticks[i].liquidity_net += net_delta;
            if gross_delta > 0 {
                ticks[i].liquidity_gross += gross_delta as u128;
            } else {
                ticks[i].liquidity_gross = ticks[i]
                    .liquidity_gross
                    .saturating_sub((-gross_delta) as u128);
            }
        }
        Err(i) => {
            ticks.insert(
                i,
                Tick {
                    index: tick_index,
                    liquidity_net: net_delta,
                    liquidity_gross: gross_delta.unsigned_abs(),
                    sqrt_price: 0,
                },
            );
        }
    }
}

