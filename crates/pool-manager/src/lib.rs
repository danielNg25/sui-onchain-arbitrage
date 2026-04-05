mod discovery;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use tracing::info;

use arb_types::config::AppConfig;
use arb_types::event::SwapEventData;
use arb_types::pool::{CoinType, ObjectId, PoolState};
use arb_types::tick::Tick;
use sui_client::SuiClient;

pub struct PoolManager {
    client: Arc<SuiClient>,
    config: Arc<AppConfig>,
    pools: DashMap<ObjectId, Arc<PoolState>>,
    tick_cache: DashMap<ObjectId, Arc<Vec<Tick>>>,
    token_to_pools: DashMap<CoinType, HashSet<ObjectId>>,
    pair_to_pools: DashMap<(CoinType, CoinType), HashSet<ObjectId>>,
    /// Checkpoint at which pool state was snapshotted.
    snapshot_checkpoint: AtomicU64,
}

impl PoolManager {
    pub fn new(client: Arc<SuiClient>, config: Arc<AppConfig>) -> Self {
        Self {
            client,
            config,
            pools: DashMap::new(),
            tick_cache: DashMap::new(),
            token_to_pools: DashMap::new(),
            pair_to_pools: DashMap::new(),
            snapshot_checkpoint: AtomicU64::new(0),
        }
    }

    /// Full discovery: record checkpoint, enumerate registries, fetch pools + ticks, build indexes.
    /// Returns the snapshot checkpoint number for event sync.
    pub async fn discover_all_pools(&self) -> anyhow::Result<u64> {
        // Record checkpoint before fetching — event sync starts from here.
        let checkpoint = self.client.get_latest_checkpoint_sequence_number().await?;
        self.snapshot_checkpoint.store(checkpoint, Ordering::SeqCst);
        info!(checkpoint = checkpoint, "recorded snapshot checkpoint");

        let whitelisted: HashSet<&str> = self
            .config
            .strategy
            .whitelisted_tokens
            .iter()
            .map(|s| s.as_str())
            .collect();

        // Discover Cetus pools
        let cetus_count = self.discover_cetus_pools(&whitelisted).await?;
        info!(count = cetus_count, "discovered Cetus pools");

        // Discover Turbos pools
        let turbos_count = self.discover_turbos_pools(&whitelisted).await?;
        info!(count = turbos_count, "discovered Turbos pools");

        info!(
            total = self.pools.len(),
            "total pools discovered"
        );

        Ok(checkpoint)
    }

    /// Fetch and cache ticks for a pool.
    pub async fn fetch_and_cache_ticks(
        &self,
        pool_id: &ObjectId,
    ) -> Result<Arc<Vec<Tick>>, arb_types::error::ArbError> {
        let pool = self.get_pool(pool_id).ok_or_else(|| {
            arb_types::error::ArbError::PoolNotFound(arb_types::pool::object_id_to_hex(pool_id))
        })?;

        let ticks = match pool.dex {
            arb_types::pool::Dex::Cetus => {
                <dex_cetus::CetusTickFetcher as dex_common::TickFetcher>::fetch_ticks(
                    &self.client, &pool,
                )
                .await?
            }
            arb_types::pool::Dex::Turbos => {
                <dex_turbos::TurbosTickFetcher as dex_common::TickFetcher>::fetch_ticks(
                    &self.client, &pool,
                )
                .await?
            }
        };

        let ticks = Arc::new(ticks);
        self.tick_cache.insert(*pool_id, ticks.clone());
        Ok(ticks)
    }

    /// Get cached ticks for a pool.
    pub fn get_ticks(&self, pool_id: &ObjectId) -> Option<Arc<Vec<Tick>>> {
        self.tick_cache.get(pool_id).map(|v| v.value().clone())
    }

    /// Get pool by ID.
    pub fn get_pool(&self, id: &ObjectId) -> Option<Arc<PoolState>> {
        self.pools.get(id).map(|v| v.value().clone())
    }

    /// Get all pools for a token pair (in either order).
    pub fn get_pools_for_pair(&self, a: &CoinType, b: &CoinType) -> Vec<Arc<PoolState>> {
        let key = if a <= b {
            (a.clone(), b.clone())
        } else {
            (b.clone(), a.clone())
        };
        self.pair_to_pools
            .get(&key)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.pools.get(id).map(|v| v.value().clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all pool IDs containing a token.
    pub fn get_pools_for_token(&self, token: &CoinType) -> Vec<ObjectId> {
        self.token_to_pools
            .get(token)
            .map(|ids| ids.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Number of cached pools.
    pub fn pool_count(&self) -> usize {
        self.pools.len()
    }

    /// Checkpoint at which pool state was snapshotted.
    pub fn snapshot_checkpoint(&self) -> u64 {
        self.snapshot_checkpoint.load(Ordering::SeqCst)
    }

    /// Update a pool's state from a swap event.
    pub fn update_from_event(&self, event: &SwapEventData) {
        if let Some(mut entry) = self.pools.get_mut(&event.pool_id) {
            let old = entry.value();
            let mut updated = (**old).clone();
            updated.sqrt_price = event.after_sqrt_price;
            updated.reserve_a = event.vault_a_amount;
            updated.reserve_b = event.vault_b_amount;
            *entry = Arc::new(updated);
        }

        // If swap crossed multiple ticks, mark for tick refresh
        if event.steps > 1 {
            self.tick_cache.remove(&event.pool_id);
        }
    }

    /// Insert a pool and update indexes.
    fn insert_pool(&self, pool: PoolState) {
        let pool = Arc::new(pool);
        let id = pool.id;

        // Update token indexes
        self.token_to_pools
            .entry(pool.coin_a.clone())
            .or_default()
            .insert(id);
        self.token_to_pools
            .entry(pool.coin_b.clone())
            .or_default()
            .insert(id);

        // Update pair index
        let pair_key = pool.pair_key();
        self.pair_to_pools
            .entry(pair_key)
            .or_default()
            .insert(id);

        self.pools.insert(id, pool);
    }
}
