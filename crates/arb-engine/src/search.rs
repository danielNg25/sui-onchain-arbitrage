use arb_types::config::SearchStrategy;

use crate::cycle::RotatedCycle;
use crate::simulator::{simulate_cycle, SimCache};

/// Configuration for the search algorithm.
#[derive(Debug, Clone)]
pub struct SearchConfig {
    pub max_iterations: u32,
    pub sample_count: u32,
    pub convergence_threshold: f64,
    pub min_range_width: u64,
}

impl SearchConfig {
    pub fn from_strategy(strategy: SearchStrategy) -> Self {
        match strategy {
            SearchStrategy::Fast => SearchConfig {
                max_iterations: 10,
                sample_count: 0,
                convergence_threshold: 0.01,
                min_range_width: 1000,
            },
            SearchStrategy::Normal => SearchConfig {
                max_iterations: 20,
                sample_count: 8,
                convergence_threshold: 0.001,
                min_range_width: 100,
            },
            SearchStrategy::Thorough => SearchConfig {
                max_iterations: 30,
                sample_count: 16,
                convergence_threshold: 0.0001,
                min_range_width: 10,
            },
        }
    }
}

/// Result of searching for optimal amount on a cycle.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub cycle_idx: usize,
    pub optimal_amount_in: u64,
    pub amount_out: u64,
    pub profit: i64,
    pub profit_usd: f64,
    pub iterations_used: u32,
}

/// Search for optimal input amount on a single cycle using two-phase approach:
/// 1. Strategic sampling to find profitable region
/// 2. Golden-section refinement on the best region
///
/// Returns None if no profitable amount is found.
pub fn search_optimal_amount(
    rotated_cycle: &RotatedCycle,
    max_amount: u64,
    pool_manager: &pool_manager::PoolManager,
    sim_cache: &SimCache,
    config: &SearchConfig,
) -> Option<SearchResult> {
    if max_amount <= 1 {
        return None;
    }

    let mut best_amount = 0u64;
    let mut best_profit = i64::MIN;
    let mut best_output = 0u64;
    let mut total_iters = 0u32;

    // Phase 1: Strategic sampling
    if config.sample_count > 0 {
        let mut sample_profits: Vec<(u64, i64)> = Vec::with_capacity(config.sample_count as usize);

        for i in 1..=config.sample_count {
            let amount = (max_amount as u128 * i as u128 / (config.sample_count as u128 + 1)) as u64;
            if amount == 0 {
                continue;
            }

            total_iters += 1;
            if let Some((output, profit)) =
                simulate_cycle(&rotated_cycle.cycle, amount, pool_manager, sim_cache)
            {
                sample_profits.push((amount, profit));
                if profit > best_profit {
                    best_profit = profit;
                    best_amount = amount;
                    best_output = output;
                }
            }
        }

        // If no sample was profitable, skip this cycle
        if best_profit <= 0 {
            return None;
        }

        // Find the region around the best sample for refinement
        // Use the adjacent samples as bounds
        let best_idx = sample_profits
            .iter()
            .position(|(a, _)| *a == best_amount)
            .unwrap_or(0);

        let lo = if best_idx > 0 {
            sample_profits[best_idx - 1].0
        } else {
            1
        };
        let hi = if best_idx + 1 < sample_profits.len() {
            sample_profits[best_idx + 1].0
        } else {
            max_amount
        };

        // Phase 2: Golden-section search in [lo, hi]
        golden_section_search(
            rotated_cycle,
            lo,
            hi,
            pool_manager,
            sim_cache,
            config,
            &mut best_amount,
            &mut best_profit,
            &mut best_output,
            &mut total_iters,
        );
    } else {
        // No sampling — pure golden-section search across full range
        golden_section_search(
            rotated_cycle,
            1,
            max_amount,
            pool_manager,
            sim_cache,
            config,
            &mut best_amount,
            &mut best_profit,
            &mut best_output,
            &mut total_iters,
        );
    }

    if best_profit <= 0 || best_amount == 0 {
        return None;
    }

    Some(SearchResult {
        cycle_idx: 0, // caller sets this
        optimal_amount_in: best_amount,
        amount_out: best_output,
        profit: best_profit,
        profit_usd: 0.0, // caller computes this
        iterations_used: total_iters,
    })
}

/// Golden-section search to find the amount that maximizes profit.
/// The profit function f(amount) is assumed unimodal: rises then falls.
#[allow(clippy::too_many_arguments)]
fn golden_section_search(
    rotated_cycle: &RotatedCycle,
    mut lo: u64,
    mut hi: u64,
    pool_manager: &pool_manager::PoolManager,
    sim_cache: &SimCache,
    config: &SearchConfig,
    best_amount: &mut u64,
    best_profit: &mut i64,
    best_output: &mut u64,
    total_iters: &mut u32,
) {
    // Golden ratio: (sqrt(5) - 1) / 2 ≈ 0.618
    // Represented as fraction: 6765 / 10946 (Fibonacci approximation)
    const GOLDEN_NUMER: u64 = 6765;
    const GOLDEN_DENOM: u64 = 10946;

    let mut stale_count = 0u32;
    let budget = config.max_iterations.saturating_sub(*total_iters);

    for _ in 0..budget {
        if hi.saturating_sub(lo) < config.min_range_width {
            break;
        }

        *total_iters += 1;

        let range = hi - lo;
        let x1 = lo + (range as u128 * (GOLDEN_DENOM - GOLDEN_NUMER) as u128 / GOLDEN_DENOM as u128) as u64;
        let x2 = lo + (range as u128 * GOLDEN_NUMER as u128 / GOLDEN_DENOM as u128) as u64;

        if x1 == x2 {
            break;
        }

        let (out1, p1) = simulate_cycle(&rotated_cycle.cycle, x1, pool_manager, sim_cache)
            .unwrap_or((0, i64::MIN));
        let (out2, p2) = simulate_cycle(&rotated_cycle.cycle, x2, pool_manager, sim_cache)
            .unwrap_or((0, i64::MIN));

        // Track best
        if p1 > *best_profit {
            *best_profit = p1;
            *best_amount = x1;
            *best_output = out1;
        }
        if p2 > *best_profit {
            *best_profit = p2;
            *best_amount = x2;
            *best_output = out2;
        }

        // Narrow bracket
        if p1 > p2 {
            hi = x2;
        } else {
            lo = x1;
        }

        // Convergence check
        if *best_profit > 0 {
            let improvement = (p1.max(p2) - *best_profit).unsigned_abs();
            let threshold = (*best_profit as f64 * config.convergence_threshold) as u64;
            if improvement <= threshold {
                stale_count += 1;
                if stale_count >= 3 {
                    break;
                }
            } else {
                stale_count = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_config_from_strategy() {
        let fast = SearchConfig::from_strategy(SearchStrategy::Fast);
        assert_eq!(fast.max_iterations, 10);
        assert_eq!(fast.sample_count, 0);

        let normal = SearchConfig::from_strategy(SearchStrategy::Normal);
        assert_eq!(normal.max_iterations, 20);
        assert_eq!(normal.sample_count, 8);

        let thorough = SearchConfig::from_strategy(SearchStrategy::Thorough);
        assert_eq!(thorough.max_iterations, 30);
        assert_eq!(thorough.sample_count, 16);
    }
}
