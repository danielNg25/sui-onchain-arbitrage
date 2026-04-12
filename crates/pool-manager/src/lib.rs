pub mod collector;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use tracing::info;

use arb_types::error::ArbError;
use arb_types::event::SwapEstimate;
use arb_types::pool::{pair_key, CoinType, ObjectId};
use dex_common::{DexRegistry, Pool};
use sui_client::SuiClient;

/// Routes operations across multiple DEX registries.
pub struct PoolManager {
    client: Arc<SuiClient>,
    registries: Vec<Arc<dyn DexRegistry>>,
    /// Maps pool_id → index into registries vec.
    pool_to_registry: DashMap<ObjectId, usize>,
    /// Global pair index across all DEXes.
    pair_to_pools: DashMap<(CoinType, CoinType), HashSet<ObjectId>>,
    /// Checkpoint at which pool state was snapshotted.
    snapshot_checkpoint: AtomicU64,
}

impl PoolManager {
    pub fn new(client: Arc<SuiClient>, registries: Vec<Arc<dyn DexRegistry>>) -> Self {
        Self {
            client,
            registries,
            pool_to_registry: DashMap::new(),
            pair_to_pools: DashMap::new(),
            snapshot_checkpoint: AtomicU64::new(0),
        }
    }

    /// Full discovery: record checkpoint, discover pools from all registries.
    /// Returns the snapshot checkpoint number for event sync.
    pub async fn discover_all_pools(
        &self,
        whitelisted_tokens: &HashSet<String>,
    ) -> anyhow::Result<u64> {
        let checkpoint = self.client.get_latest_checkpoint_sequence_number().await?;
        self.snapshot_checkpoint.store(checkpoint, Ordering::SeqCst);
        info!(checkpoint = checkpoint, "recorded snapshot checkpoint");

        for (idx, registry) in self.registries.iter().enumerate() {
            let pools = registry
                .discover_pools(&self.client, whitelisted_tokens)
                .await?;

            for (pool_id, coin_a, coin_b) in &pools {
                self.pool_to_registry.insert(*pool_id, idx);
                let key = pair_key(coin_a, coin_b);
                self.pair_to_pools
                    .entry(key)
                    .or_default()
                    .insert(*pool_id);
            }
        }

        let total: usize = self.registries.iter().map(|r| r.pool_count()).sum();
        info!(total = total, "total pools discovered");

        Ok(checkpoint)
    }

    /// Route an event to the appropriate pool across all registries.
    /// Returns the pool ID if a pool was updated.
    pub fn apply_event(
        &self,
        event_type: &str,
        parsed_json: &serde_json::Value,
    ) -> Result<Option<ObjectId>, ArbError> {
        for registry in &self.registries {
            if !registry.event_types().contains(&event_type) {
                continue;
            }
            // Route to all pools in this registry that might match
            for pool_id in registry.pool_ids() {
                if let Some(pool) = registry.pool(&pool_id) {
                    match pool.apply_event(event_type, parsed_json)? {
                        Some(needs_refresh) => {
                            if needs_refresh {
                                // Caller should re-fetch price data for this pool
                            }
                            return Ok(Some(pool_id));
                        }
                        None => continue,
                    }
                }
            }
        }
        Ok(None)
    }

    /// Get a pool handle by ID.
    pub fn pool(&self, pool_id: &ObjectId) -> Option<Arc<dyn Pool>> {
        let idx = self.pool_to_registry.get(pool_id)?;
        self.registries[*idx].pool(pool_id)
    }

    /// Estimate swap for a specific pool.
    pub fn estimate_swap(
        &self,
        pool_id: &ObjectId,
        token_in: &CoinType,
        amount_in: u64,
    ) -> Result<SwapEstimate, ArbError> {
        let pool = self.pool(pool_id).ok_or_else(|| {
            ArbError::PoolNotFound(arb_types::pool::object_id_to_hex(pool_id))
        })?;
        pool.estimate_swap(token_in, amount_in)
    }

    /// Get all pools for a token pair (in either order).
    pub fn get_pools_for_pair(&self, a: &CoinType, b: &CoinType) -> Vec<ObjectId> {
        let key = pair_key(a, b);
        self.pair_to_pools
            .get(&key)
            .map(|ids| ids.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Get all pools containing a specific token.
    pub fn get_pools_for_token(&self, token: &CoinType) -> Vec<ObjectId> {
        self.registries
            .iter()
            .flat_map(|r| r.pools_for_token(token))
            .collect()
    }

    /// Total number of pools across all registries.
    pub fn pool_count(&self) -> usize {
        self.registries.iter().map(|r| r.pool_count()).sum()
    }

    /// Checkpoint at which pool state was snapshotted.
    pub fn snapshot_checkpoint(&self) -> u64 {
        self.snapshot_checkpoint.load(Ordering::SeqCst)
    }

    /// Get all registries.
    pub fn registries(&self) -> &[Arc<dyn DexRegistry>] {
        &self.registries
    }
}
