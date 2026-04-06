mod events;
pub(crate) mod raw;
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

const CETUS_EVENT_TYPES: &[&str] = &[CETUS_SWAP_EVENT_TYPE];

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
        if event_type != CETUS_SWAP_EVENT_TYPE {
            return Ok(None);
        }

        // Check if this event is for our pool
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

        // If multiple ticks crossed, price data needs refresh
        Ok(Some(steps > 1))
    }

    fn estimate_swap(
        &self,
        _token_in: &CoinType,
        _amount_in: u64,
    ) -> Result<SwapEstimate, ArbError> {
        // Placeholder — requires clmm-math (Phase 2)
        Err(ArbError::InvalidData(
            "swap estimation requires clmm-math (Phase 2)".into(),
        ))
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

/// Check if a Cetus pool is paused from JSON content.
pub fn is_pool_paused(content: &serde_json::Value) -> bool {
    content
        .get("fields")
        .and_then(|f| f.get("is_pause"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

