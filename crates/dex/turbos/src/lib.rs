mod events;
pub(crate) mod raw;
mod ticks;

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use dashmap::DashMap;
use tracing::{debug, info, warn};

use arb_types::error::ArbError;
use arb_types::event::SwapEstimate;
use arb_types::pool::{object_id_from_hex, CoinType, Dex};
use arb_types::tick::Tick;
use dex_common::{parse_type_params_with_fee, DexRegistry, Pool};
use sui_client::{ObjectDataOptions, SuiClient};

/// Turbos CLMM swap event type string.
pub const TURBOS_SWAP_EVENT_TYPE: &str =
    "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1::pool::SwapEvent";

const TURBOS_EVENT_TYPES: &[&str] = &[TURBOS_SWAP_EVENT_TYPE];

// ---------------------------------------------------------------------------
// TurbosPool — internal CLMM state
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub(crate) struct TurbosPoolState {
    pub sqrt_price: u128,
    pub tick_current: i32,
    pub liquidity: u128,
    pub fee_rate: u64,
    pub tick_spacing: u32,
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub is_active: bool,
    pub ticks_table_id: [u8; 32],
    pub fee_type: CoinType,
    pub initial_shared_version: u64,
    pub object_version: u64,
}

pub struct TurbosPool {
    id: [u8; 32],
    coin_a: CoinType,
    coin_b: CoinType,
    state: RwLock<TurbosPoolState>,
    ticks: RwLock<Vec<Tick>>,
}

#[async_trait::async_trait]
impl Pool for TurbosPool {
    fn id(&self) -> [u8; 32] {
        self.id
    }

    fn dex(&self) -> Dex {
        Dex::Turbos
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
        let new_ticks = ticks::fetch_turbos_ticks(client, &self.id, &ticks_table_id).await?;
        *self.ticks.write().unwrap() = new_ticks;
        Ok(())
    }

    fn apply_event(
        &self,
        event_type: &str,
        parsed_json: &serde_json::Value,
    ) -> Result<Option<bool>, ArbError> {
        if event_type != TURBOS_SWAP_EVENT_TYPE {
            return Ok(None);
        }

        let pool_id_str = match parsed_json["pool"].as_str() {
            Some(s) => s,
            None => return Ok(None),
        };
        let event_pool_id = object_id_from_hex(pool_id_str)?;
        if event_pool_id != self.id {
            return Ok(None);
        }

        let after_sqrt_price = events::parse_u128_field(parsed_json, "after_sqrt_price")?;
        let vault_a = events::parse_u64_field(parsed_json, "vault_a_amount")?;
        let vault_b = events::parse_u64_field(parsed_json, "vault_b_amount")?;
        let steps = events::parse_u64_field(parsed_json, "steps")?;

        {
            let mut state = self.state.write().unwrap();
            state.sqrt_price = after_sqrt_price;
            state.reserve_a = vault_a;
            state.reserve_b = vault_b;
        }

        Ok(Some(steps > 1))
    }

    fn estimate_swap(
        &self,
        _token_in: &CoinType,
        _amount_in: u64,
    ) -> Result<SwapEstimate, ArbError> {
        Err(ArbError::InvalidData(
            "swap estimation requires clmm-math (Phase 2)".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// TurbosRegistry
// ---------------------------------------------------------------------------

/// Get ticks table ID for a pool (for testing/verification).
pub fn get_ticks_table_id(registry: &TurbosRegistry, pool_id: &[u8; 32]) -> Option<[u8; 32]> {
    registry.pools.get(pool_id).map(|p| p.state.read().unwrap().ticks_table_id)
}

/// Fetch ticks for a pool (for testing/verification).
pub async fn fetch_ticks_for_pool(
    client: &SuiClient,
    registry: &TurbosRegistry,
    pool_id: &[u8; 32],
) -> Result<Vec<Tick>, ArbError> {
    let pool = registry.pools.get(pool_id).ok_or_else(|| {
        ArbError::PoolNotFound(arb_types::pool::object_id_to_hex(pool_id))
    })?;
    let ticks_table_id = pool.state.read().unwrap().ticks_table_id;
    ticks::fetch_turbos_ticks(client, pool_id, &ticks_table_id).await
}

/// Turbos pool creation event type — used for pool discovery.
pub const TURBOS_CREATE_POOL_EVENT_TYPE: &str =
    "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1::pool_factory::PoolCreatedEvent";

pub struct TurbosRegistry {
    package_types: String,
    pools: DashMap<[u8; 32], Arc<TurbosPool>>,
    token_index: DashMap<CoinType, HashSet<[u8; 32]>>,
}

impl TurbosRegistry {
    pub fn new(config: &arb_types::config::TurbosConfig) -> Self {
        Self {
            package_types: config.package_types.clone(),
            pools: DashMap::new(),
            token_index: DashMap::new(),
        }
    }

    fn index_pool(&self, pool_id: [u8; 32], coin_a: &CoinType, coin_b: &CoinType) {
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
impl DexRegistry for TurbosRegistry {
    fn dex(&self) -> Dex {
        Dex::Turbos
    }

    fn event_types(&self) -> &[&str] {
        TURBOS_EVENT_TYPES
    }

    fn matches_pool_type(&self, type_string: &str) -> bool {
        type_string.contains(&format!("{}::pool::Pool", self.package_types))
    }

    async fn discover_pools(
        &self,
        client: &SuiClient,
        whitelisted_tokens: &HashSet<String>,
    ) -> Result<Vec<([u8; 32], CoinType, CoinType)>, ArbError> {
        // Query all PoolCreatedEvent to collect pool IDs
        let mut pool_obj_ids = Vec::new();
        let mut cursor = None;

        loop {
            let events = client
                .query_events(
                    sui_client::EventFilter::MoveEventType(
                        TURBOS_CREATE_POOL_EVENT_TYPE.to_string(),
                    ),
                    cursor,
                    Some(200),
                    false,
                )
                .await
                .map_err(|e| ArbError::Rpc(format!("query Turbos PoolCreatedEvent: {}", e)))?;

            for event in &events.data {
                if let Some(json) = &event.parsed_json {
                    // Turbos event might use "pool" or "pool_id"
                    let pool_id = json["pool"]
                        .as_str()
                        .or_else(|| json["pool_id"].as_str());
                    if let Some(id) = pool_id {
                        pool_obj_ids.push(id.to_string());
                    }
                }
            }

            if !events.has_next_page {
                break;
            }
            cursor = events.next_cursor;
        }

        debug!(count = pool_obj_ids.len(), "found Turbos pools via PoolCreatedEvent");

        // Batch-fetch pool objects with BCS
        let mut results = Vec::new();
        for chunk in pool_obj_ids.chunks(50) {
            let objects = client
                .multi_get_objects(chunk, ObjectDataOptions::bcs())
                .await
                .map_err(|e| ArbError::Rpc(format!("batch fetch Turbos pools: {}", e)))?;

            for obj_resp in &objects {
                let Some(data) = &obj_resp.data else { continue };

                let type_str = match data.bcs_type() {
                    Some(t) if self.matches_pool_type(t) => t,
                    _ => continue,
                };

                let (coin_params, fee_type) = parse_type_params_with_fee(type_str);
                if coin_params.len() < 2 {
                    continue;
                }

                if !whitelisted_tokens.is_empty()
                    && !whitelisted_tokens.contains(&coin_params[0])
                    && !whitelisted_tokens.contains(&coin_params[1])
                {
                    continue;
                }

                let bcs_bytes = match data.bcs_bytes() {
                    Ok(b) => b,
                    Err(e) => {
                        warn!("skip Turbos pool {}: {}", data.object_id, e);
                        continue;
                    }
                };

                let object_id = match object_id_from_hex(&data.object_id) {
                    Ok(id) => id,
                    Err(e) => {
                        warn!("skip Turbos pool {}: {}", data.object_id, e);
                        continue;
                    }
                };

                let mut type_params = coin_params;
                if let Some(ft) = fee_type {
                    type_params.push(ft);
                }

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
                        debug!("skip Turbos pool {} deser: {}", data.object_id, e);
                    }
                }
            }
        }

        info!(count = results.len(), "discovered Turbos pools");
        Ok(results)
    }

    fn ingest_pool_object(
        &self,
        object_id: [u8; 32],
        bcs_bytes: &[u8],
        type_params: &[String],
        object_version: u64,
        initial_shared_version: u64,
    ) -> Result<Option<([u8; 32], CoinType, CoinType)>, ArbError> {
        if type_params.len() < 2 {
            return Err(ArbError::InvalidData(format!(
                "Turbos pool requires at least 2 type params, got {}",
                type_params.len()
            )));
        }

        let raw = raw::parse_turbos_pool(bcs_bytes)?;

        if !raw.unlocked {
            return Ok(None);
        }

        let coin_a: CoinType = Arc::from(type_params[0].as_str());
        let coin_b: CoinType = Arc::from(type_params[1].as_str());
        let fee_type: CoinType = type_params
            .get(2)
            .map(|s| Arc::from(s.as_str()))
            .unwrap_or_else(|| Arc::from(""));

        let pool = Arc::new(TurbosPool {
            id: object_id,
            coin_a: coin_a.clone(),
            coin_b: coin_b.clone(),
            state: RwLock::new(TurbosPoolState {
                sqrt_price: raw.sqrt_price,
                tick_current: raw.tick_current_index,
                liquidity: raw.liquidity,
                fee_rate: raw.fee as u64,
                tick_spacing: raw.tick_spacing,
                reserve_a: raw.coin_a,
                reserve_b: raw.coin_b,
                is_active: raw.unlocked,
                ticks_table_id: raw.tick_map_id,
                fee_type,
                initial_shared_version,
                object_version,
            }),
            ticks: RwLock::new(Vec::new()),
        });

        self.index_pool(object_id, &coin_a, &coin_b);
        self.pools.insert(object_id, pool);

        Ok(Some((object_id, coin_a, coin_b)))
    }

    fn pool(&self, pool_id: &[u8; 32]) -> Option<Arc<dyn Pool>> {
        self.pools
            .get(pool_id)
            .map(|entry| entry.value().clone() as Arc<dyn Pool>)
    }

    fn pool_ids(&self) -> Vec<[u8; 32]> {
        self.pools.iter().map(|entry| *entry.key()).collect()
    }

    fn pools_for_token(&self, token: &CoinType) -> Vec<[u8; 32]> {
        self.token_index
            .get(token)
            .map(|ids| ids.iter().copied().collect())
            .unwrap_or_default()
    }

    fn pool_count(&self) -> usize {
        self.pools.len()
    }
}
