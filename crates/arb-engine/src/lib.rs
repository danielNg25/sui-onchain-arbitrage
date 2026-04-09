pub mod cycle;
pub mod error;
pub mod graph;
pub mod opportunity;
pub mod profit_token;
pub mod search;
pub mod simulator;

use std::sync::Arc;
use std::time::Instant;

use arb_types::config::StrategyConfig;
use arb_types::event::SwapEventData;
use arb_types::pool::{object_id_to_hex, CoinType};
use tracing::{debug, info};

use crate::cycle::CycleIndex;
use crate::error::EngineError;
use crate::opportunity::Opportunity;
use crate::profit_token::ProfitTokenRegistry;
use crate::search::SearchConfig;
use crate::simulator::SimCache;

pub struct ArbEngine {
    pool_manager: Arc<pool_manager::PoolManager>,
    cycle_index: CycleIndex,
    profit_registry: Arc<ProfitTokenRegistry>,
    search_config: SearchConfig,
    min_profit_usd: f64,
}

impl ArbEngine {
    /// Build the engine: construct graph, detect cycles, index them.
    /// Call after pool_manager.discover_all_pools() completes.
    pub fn build(
        pool_manager: Arc<pool_manager::PoolManager>,
        profit_registry: Arc<ProfitTokenRegistry>,
        strategy_config: &StrategyConfig,
    ) -> Result<Self, EngineError> {
        let graph = graph::ArbGraph::build(&pool_manager);
        info!(
            tokens = graph.token_count(),
            edges = graph.edge_count(),
            "built token graph"
        );

        let profit_tokens: Vec<CoinType> = profit_registry.profit_token_types();
        let cycle_index = cycle::find_all_cycles(
            &graph,
            strategy_config.max_hops,
            &profit_tokens,
        );
        info!(
            cycles = cycle_index.len(),
            "precomputed arbitrage cycles"
        );

        let search_config = SearchConfig::from_strategy(strategy_config.search_strategy);

        Ok(Self {
            pool_manager,
            cycle_index,
            profit_registry,
            search_config,
            min_profit_usd: strategy_config.min_profit_usd,
        })
    }

    /// Process a swap event: find all cycles containing the affected pool,
    /// search for optimal amounts, return profitable opportunities.
    pub async fn process_event(&self, event: &SwapEventData) -> Vec<Opportunity> {
        let start = Instant::now();
        let sim_cache = SimCache::new();

        let cycle_indices = self.cycle_index.cycles_for_pool(&event.pool_id);
        if cycle_indices.is_empty() {
            return Vec::new();
        }

        let max_amount = event.amount_in;
        if max_amount == 0 {
            return Vec::new();
        }

        let mut opportunities = Vec::new();

        for &cycle_idx in cycle_indices {
            let rotated_cycle = self.cycle_index.get(cycle_idx);

            let result = search::search_optimal_amount(
                rotated_cycle,
                max_amount,
                &self.pool_manager,
                &sim_cache,
                &self.search_config,
            );

            if let Some(search_result) = result {
                if search_result.profit <= 0 {
                    continue;
                }

                let profit_token = rotated_cycle.cycle.profit_token().clone();
                let profit_usd = self
                    .profit_registry
                    .get_usd_value(&profit_token, search_result.profit as u64)
                    .await
                    .unwrap_or(0.0);

                if profit_usd < self.min_profit_usd {
                    continue;
                }

                opportunities.push(Opportunity {
                    cycle: rotated_cycle.as_ref().clone(),
                    amount_in: search_result.optimal_amount_in,
                    amount_out: search_result.amount_out,
                    profit: search_result.profit as u64,
                    profit_usd,
                    profit_token,
                    trigger_pool_id: event.pool_id,
                    detected_at_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64,
                });
            }
        }

        opportunities.sort_by(|a, b| {
            b.profit_usd
                .partial_cmp(&a.profit_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let elapsed = start.elapsed();
        if !opportunities.is_empty() {
            info!(
                pool = %object_id_to_hex(&event.pool_id),
                cycles_checked = cycle_indices.len(),
                opportunities = opportunities.len(),
                best_profit_usd = format!("{:.4}", opportunities[0].profit_usd),
                elapsed_us = elapsed.as_micros(),
                "found opportunities"
            );
        } else {
            debug!(
                pool = %object_id_to_hex(&event.pool_id),
                cycles_checked = cycle_indices.len(),
                elapsed_us = elapsed.as_micros(),
                "no profitable cycles"
            );
        }

        opportunities
    }

    pub fn cycle_count(&self) -> usize {
        self.cycle_index.len()
    }

    pub fn cycle_index(&self) -> &CycleIndex {
        &self.cycle_index
    }
}
