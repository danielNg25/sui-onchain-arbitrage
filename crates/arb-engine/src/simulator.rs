use arb_types::event::SwapEstimate;
use arb_types::pool::{CoinType, ObjectId};
use dashmap::DashMap;

use crate::cycle::Cycle;

/// Cache key for memoizing simulation results within one event batch.
#[derive(Hash, Eq, PartialEq, Clone)]
struct SimCacheKey {
    pool_id: ObjectId,
    token_in: CoinType,
    amount_in: u64,
}

/// Event-scoped simulation cache. Created per event, shared across all
/// cycle simulations triggered by that event, then dropped.
pub struct SimCache {
    cache: DashMap<SimCacheKey, SwapEstimate>,
}

impl Default for SimCache {
    fn default() -> Self {
        Self::new()
    }
}

impl SimCache {
    pub fn new() -> Self {
        Self {
            cache: DashMap::new(),
        }
    }

    pub fn get(
        &self,
        pool_id: &ObjectId,
        token_in: &CoinType,
        amount_in: u64,
    ) -> Option<SwapEstimate> {
        let key = SimCacheKey {
            pool_id: *pool_id,
            token_in: token_in.clone(),
            amount_in,
        };
        self.cache.get(&key).map(|v| v.clone())
    }

    pub fn insert(
        &self,
        pool_id: &ObjectId,
        token_in: &CoinType,
        amount_in: u64,
        result: SwapEstimate,
    ) {
        let key = SimCacheKey {
            pool_id: *pool_id,
            token_in: token_in.clone(),
            amount_in,
        };
        self.cache.insert(key, result);
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

/// Simulate a full cycle for a given input amount.
/// Returns (output_amount, profit) where profit = output - input.
/// Returns None if any leg produces zero output (dead path).
pub fn simulate_cycle(
    cycle: &Cycle,
    amount_in: u64,
    pool_manager: &pool_manager::PoolManager,
    sim_cache: &SimCache,
) -> Option<(u64, i64)> {
    if amount_in == 0 {
        return None;
    }

    let mut current_amount = amount_in;

    for leg in &cycle.legs {
        // Check cache first
        let estimate = match sim_cache.get(&leg.pool_id, &leg.token_in, current_amount) {
            Some(cached) => cached,
            None => {
                let est = pool_manager
                    .estimate_swap(&leg.pool_id, &leg.token_in, current_amount)
                    .ok()?;
                sim_cache.insert(&leg.pool_id, &leg.token_in, current_amount, est.clone());
                est
            }
        };

        if estimate.amount_out == 0 {
            return None;
        }

        current_amount = estimate.amount_out;
    }

    let profit = current_amount as i64 - amount_in as i64;
    Some((current_amount, profit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_sim_cache_hit_miss() {
        let cache = SimCache::new();
        let pool_id = [0u8; 32];
        let token: CoinType = Arc::from("SUI");

        assert!(cache.get(&pool_id, &token, 1000).is_none());

        let estimate = SwapEstimate {
            token_in: token.clone(),
            token_out: Arc::from("USDC"),
            amount_in: 1000,
            amount_out: 990,
            fee_amount: 10,
        };
        cache.insert(&pool_id, &token, 1000, estimate.clone());

        let cached = cache.get(&pool_id, &token, 1000).unwrap();
        assert_eq!(cached.amount_out, 990);

        // Different amount = miss
        assert!(cache.get(&pool_id, &token, 2000).is_none());
    }

    #[test]
    fn test_sim_cache_different_pools() {
        let cache = SimCache::new();
        let pool1 = [1u8; 32];
        let pool2 = [2u8; 32];
        let token: CoinType = Arc::from("SUI");

        let est1 = SwapEstimate {
            token_in: token.clone(),
            token_out: Arc::from("USDC"),
            amount_in: 1000,
            amount_out: 990,
            fee_amount: 10,
        };
        let est2 = SwapEstimate {
            token_in: token.clone(),
            token_out: Arc::from("USDC"),
            amount_in: 1000,
            amount_out: 985,
            fee_amount: 15,
        };

        cache.insert(&pool1, &token, 1000, est1);
        cache.insert(&pool2, &token, 1000, est2);

        assert_eq!(cache.get(&pool1, &token, 1000).unwrap().amount_out, 990);
        assert_eq!(cache.get(&pool2, &token, 1000).unwrap().amount_out, 985);
        assert_eq!(cache.len(), 2);
    }
}
