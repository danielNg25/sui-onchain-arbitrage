use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arb_types::pool::{CoinType, ObjectId};

use crate::graph::ArbGraph;

/// One leg of an arbitrage cycle.
#[derive(Debug, Clone)]
pub struct CycleLeg {
    pub pool_id: ObjectId,
    pub token_in: CoinType,
    pub token_out: CoinType,
}

/// An arbitrage cycle: a closed path through pools.
/// The last leg's token_out equals the first leg's token_in.
#[derive(Debug, Clone)]
pub struct Cycle {
    pub legs: Vec<CycleLeg>,
}

impl Cycle {
    /// The token we start and end with (profit/loss token).
    pub fn profit_token(&self) -> &CoinType {
        &self.legs[0].token_in
    }

    /// Number of hops (legs) in the cycle.
    pub fn len(&self) -> usize {
        self.legs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.legs.is_empty()
    }

    /// All pool IDs in this cycle.
    pub fn pool_ids(&self) -> Vec<ObjectId> {
        self.legs.iter().map(|l| l.pool_id).collect()
    }

    /// All tokens in this cycle (input tokens of each leg).
    pub fn tokens(&self) -> Vec<CoinType> {
        self.legs.iter().map(|l| l.token_in.clone()).collect()
    }

    /// Rotate the cycle so that the given token is the start/end (profit) token.
    /// Returns None if the token is not in the cycle.
    pub fn rotate_to_profit_token(&self, profit_token: &CoinType) -> Option<Cycle> {
        let pos = self.legs.iter().position(|l| &l.token_in == profit_token)?;
        if pos == 0 {
            return Some(self.clone());
        }
        let mut rotated = Vec::with_capacity(self.legs.len());
        rotated.extend_from_slice(&self.legs[pos..]);
        rotated.extend_from_slice(&self.legs[..pos]);
        Some(Cycle { legs: rotated })
    }
}

/// A cycle prepared for execution: rotated so the output is a profit token.
#[derive(Debug, Clone)]
pub struct RotatedCycle {
    /// The cycle with legs rotated so profit_token is legs[0].token_in.
    pub cycle: Cycle,
    /// Index of the profit token in the profit token registry (usize::MAX if fallback).
    pub profit_token_idx: usize,
    /// Original (un-rotated) cycle for debugging/logging.
    pub original_cycle: Cycle,
}

/// Index: pool_id -> vec of RotatedCycle references.
/// Used for O(1) lookup when a swap event arrives.
pub struct CycleIndex {
    /// All discovered rotated cycles.
    cycles: Vec<Arc<RotatedCycle>>,
    /// pool_id -> indices into `cycles` vec.
    pool_to_cycles: HashMap<ObjectId, Vec<usize>>,
}

impl CycleIndex {
    /// Get all cycle indices containing a given pool.
    pub fn cycles_for_pool(&self, pool_id: &ObjectId) -> &[usize] {
        static EMPTY: Vec<usize> = Vec::new();
        self.pool_to_cycles
            .get(pool_id)
            .map_or(&EMPTY, |v| v.as_slice())
    }

    /// Get a cycle by index.
    pub fn get(&self, idx: usize) -> &Arc<RotatedCycle> {
        &self.cycles[idx]
    }

    /// Total number of cycles.
    pub fn len(&self) -> usize {
        self.cycles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cycles.is_empty()
    }

    /// Iterator over all cycles.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<RotatedCycle>> {
        self.cycles.iter()
    }
}

/// Canonical key for deduplication: sorted list of (pool_id, token_in) tuples.
/// Two cycles are the same if they traverse the same pools in the same directions,
/// regardless of starting token.
fn canonical_key(legs: &[CycleLeg]) -> Vec<(ObjectId, CoinType)> {
    let key: Vec<(ObjectId, CoinType)> = legs
        .iter()
        .map(|l| (l.pool_id, l.token_in.clone()))
        .collect();
    // Find the lexicographically smallest rotation
    let n = key.len();
    let mut best_start = 0;
    for i in 1..n {
        let mut is_smaller = false;
        for j in 0..n {
            let a = &key[(best_start + j) % n];
            let b = &key[(i + j) % n];
            match a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)) {
                std::cmp::Ordering::Less => break,
                std::cmp::Ordering::Greater => {
                    is_smaller = true;
                    break;
                }
                std::cmp::Ordering::Equal => continue,
            }
        }
        if is_smaller {
            best_start = i;
        }
    }
    let mut canonical = Vec::with_capacity(n);
    for j in 0..n {
        canonical.push(key[(best_start + j) % n].clone());
    }
    canonical
}

/// Find all arbitrage cycles up to max_hops length using DFS.
pub fn find_all_cycles(
    graph: &ArbGraph,
    max_hops: u32,
    profit_tokens: &[CoinType],
) -> CycleIndex {
    let mut seen_keys: HashSet<Vec<(ObjectId, CoinType)>> = HashSet::new();
    let mut raw_cycles: Vec<Cycle> = Vec::new();

    let tokens: Vec<CoinType> = graph.tokens().iter().cloned().collect();

    for start_token in &tokens {
        let mut path: Vec<CycleLeg> = Vec::new();
        let mut used_pools: HashSet<ObjectId> = HashSet::new();
        dfs(
            graph,
            start_token,
            start_token,
            &mut path,
            &mut used_pools,
            max_hops,
            &mut seen_keys,
            &mut raw_cycles,
        );
    }

    // Rotate each cycle to best profit token and build index
    let mut cycles = Vec::with_capacity(raw_cycles.len());
    let mut pool_to_cycles: HashMap<ObjectId, Vec<usize>> = HashMap::new();

    for raw_cycle in &raw_cycles {
        let cycle_tokens = raw_cycle.tokens();

        // Find best profit token in this cycle (by priority order)
        let (rotated, profit_idx) = find_best_rotation(raw_cycle, &cycle_tokens, profit_tokens);

        let idx = cycles.len();
        let rotated_cycle = Arc::new(RotatedCycle {
            cycle: rotated,
            profit_token_idx: profit_idx,
            original_cycle: raw_cycle.clone(),
        });

        // Index by every pool in the cycle
        for leg in &rotated_cycle.cycle.legs {
            pool_to_cycles
                .entry(leg.pool_id)
                .or_default()
                .push(idx);
        }

        cycles.push(rotated_cycle);
    }

    CycleIndex {
        cycles,
        pool_to_cycles,
    }
}

fn find_best_rotation(
    cycle: &Cycle,
    cycle_tokens: &[CoinType],
    profit_tokens: &[CoinType],
) -> (Cycle, usize) {
    // Find highest-priority profit token in this cycle
    for (idx, pt) in profit_tokens.iter().enumerate() {
        if cycle_tokens.contains(pt) {
            if let Some(rotated) = cycle.rotate_to_profit_token(pt) {
                return (rotated, idx);
            }
        }
    }
    // Fallback: use the first token as-is
    (cycle.clone(), usize::MAX)
}

#[allow(clippy::too_many_arguments)]
fn dfs(
    graph: &ArbGraph,
    start_token: &CoinType,
    current_token: &CoinType,
    path: &mut Vec<CycleLeg>,
    used_pools: &mut HashSet<ObjectId>,
    max_hops: u32,
    seen_keys: &mut HashSet<Vec<(ObjectId, CoinType)>>,
    results: &mut Vec<Cycle>,
) {
    if path.len() >= max_hops as usize {
        return;
    }

    for edge in graph.neighbors(current_token) {
        if used_pools.contains(&edge.pool_id) {
            continue;
        }

        let leg = CycleLeg {
            pool_id: edge.pool_id,
            token_in: edge.token_in.clone(),
            token_out: edge.token_out.clone(),
        };

        // Check if this closes the cycle
        if &edge.token_out == start_token && !path.is_empty() {
            // Found a cycle!
            path.push(leg);
            let key = canonical_key(path);
            if seen_keys.insert(key) {
                results.push(Cycle { legs: path.clone() });
            }
            path.pop();
            continue;
        }

        // Don't revisit tokens (except start, handled above)
        let visited = path.iter().any(|l| l.token_in == edge.token_out);
        if visited {
            continue;
        }

        // Recurse
        path.push(leg);
        used_pools.insert(edge.pool_id);
        dfs(
            graph,
            start_token,
            &edge.token_out,
            path,
            used_pools,
            max_hops,
            seen_keys,
            results,
        );
        used_pools.remove(&edge.pool_id);
        path.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::PoolEdge;

    fn make_edge(pool_id: u8, token_in: &str, token_out: &str) -> PoolEdge {
        let mut id = [0u8; 32];
        id[31] = pool_id;
        PoolEdge {
            pool_id: id,
            token_in: Arc::from(token_in),
            token_out: Arc::from(token_out),
            fee_rate: 2500,
        }
    }

    fn pool_id(n: u8) -> ObjectId {
        let mut id = [0u8; 32];
        id[31] = n;
        id
    }

    #[test]
    fn test_triangle_cycle_detection() {
        // SUI <-> USDC, USDC <-> USDT, USDT <-> SUI = triangle
        let edges = vec![
            make_edge(1, "SUI", "USDC"),
            make_edge(1, "USDC", "SUI"),
            make_edge(2, "USDC", "USDT"),
            make_edge(2, "USDT", "USDC"),
            make_edge(3, "USDT", "SUI"),
            make_edge(3, "SUI", "USDT"),
        ];
        let graph = ArbGraph::from_edges(edges);
        let profit_tokens: Vec<CoinType> = vec![Arc::from("SUI")];

        let index = find_all_cycles(&graph, 3, &profit_tokens);

        // Should find 2 cycles: clockwise and counterclockwise
        assert_eq!(index.len(), 2);

        // Both should be rotated to start with SUI
        for i in 0..index.len() {
            let rc = index.get(i);
            assert_eq!(rc.cycle.profit_token().as_ref(), "SUI");
            assert_eq!(rc.cycle.len(), 3);
        }

        // Each pool should be in 2 cycles
        assert_eq!(index.cycles_for_pool(&pool_id(1)).len(), 2);
        assert_eq!(index.cycles_for_pool(&pool_id(2)).len(), 2);
        assert_eq!(index.cycles_for_pool(&pool_id(3)).len(), 2);
    }

    #[test]
    fn test_no_cycles_with_two_pools() {
        // Only 2 pools connecting 2 tokens — no cycle of length >= 2 through distinct pools
        let edges = vec![
            make_edge(1, "SUI", "USDC"),
            make_edge(1, "USDC", "SUI"),
            make_edge(2, "SUI", "USDC"),
            make_edge(2, "USDC", "SUI"),
        ];
        let graph = ArbGraph::from_edges(edges);
        let profit_tokens: Vec<CoinType> = vec![Arc::from("SUI")];

        let index = find_all_cycles(&graph, 3, &profit_tokens);

        // 2-pool cycles between same pair should be found (SUI->USDC via pool1, USDC->SUI via pool2)
        assert_eq!(index.len(), 2);
    }

    #[test]
    fn test_max_hops_limit() {
        // 4-pool square: SUI-USDC-USDT-WETH-SUI
        let edges = vec![
            make_edge(1, "SUI", "USDC"),
            make_edge(1, "USDC", "SUI"),
            make_edge(2, "USDC", "USDT"),
            make_edge(2, "USDT", "USDC"),
            make_edge(3, "USDT", "WETH"),
            make_edge(3, "WETH", "USDT"),
            make_edge(4, "WETH", "SUI"),
            make_edge(4, "SUI", "WETH"),
        ];
        let graph = ArbGraph::from_edges(edges);
        let profit_tokens: Vec<CoinType> = vec![Arc::from("SUI")];

        // max_hops=3 should not find the 4-hop cycle
        let index3 = find_all_cycles(&graph, 3, &profit_tokens);
        assert_eq!(index3.len(), 0);

        // max_hops=4 should find it
        let index4 = find_all_cycles(&graph, 4, &profit_tokens);
        assert!(index4.len() >= 2); // clockwise + counterclockwise
    }

    #[test]
    fn test_profit_token_rotation() {
        let edges = vec![
            make_edge(1, "SUI", "USDC"),
            make_edge(1, "USDC", "SUI"),
            make_edge(2, "USDC", "USDT"),
            make_edge(2, "USDT", "USDC"),
            make_edge(3, "USDT", "SUI"),
            make_edge(3, "SUI", "USDT"),
        ];
        let graph = ArbGraph::from_edges(edges);

        // USDC is highest priority profit token
        let profit_tokens: Vec<CoinType> = vec![Arc::from("USDC"), Arc::from("SUI")];
        let index = find_all_cycles(&graph, 3, &profit_tokens);

        for i in 0..index.len() {
            let rc = index.get(i);
            assert_eq!(
                rc.cycle.profit_token().as_ref(),
                "USDC",
                "should rotate to highest priority profit token"
            );
            assert_eq!(rc.profit_token_idx, 0);
        }
    }

    #[test]
    fn test_fallback_profit_token() {
        let edges = vec![
            make_edge(1, "WETH", "WBTC"),
            make_edge(1, "WBTC", "WETH"),
            make_edge(2, "WBTC", "DAI"),
            make_edge(2, "DAI", "WBTC"),
            make_edge(3, "DAI", "WETH"),
            make_edge(3, "WETH", "DAI"),
        ];
        let graph = ArbGraph::from_edges(edges);

        // None of the cycle tokens are profit tokens
        let profit_tokens: Vec<CoinType> = vec![Arc::from("SUI"), Arc::from("USDC")];
        let index = find_all_cycles(&graph, 3, &profit_tokens);

        assert!(index.len() > 0);
        for i in 0..index.len() {
            let rc = index.get(i);
            assert_eq!(
                rc.profit_token_idx,
                usize::MAX,
                "should use fallback"
            );
        }
    }

    #[test]
    fn test_cycle_index_lookup() {
        let edges = vec![
            make_edge(1, "SUI", "USDC"),
            make_edge(1, "USDC", "SUI"),
            make_edge(2, "USDC", "USDT"),
            make_edge(2, "USDT", "USDC"),
            make_edge(3, "USDT", "SUI"),
            make_edge(3, "SUI", "USDT"),
        ];
        let graph = ArbGraph::from_edges(edges);
        let profit_tokens: Vec<CoinType> = vec![Arc::from("SUI")];
        let index = find_all_cycles(&graph, 3, &profit_tokens);

        // Pool not in any cycle
        let unknown = pool_id(99);
        assert!(index.cycles_for_pool(&unknown).is_empty());

        // Pool 1 should be in cycles
        assert!(!index.cycles_for_pool(&pool_id(1)).is_empty());
    }

    #[test]
    fn test_cycle_rotate() {
        let cycle = Cycle {
            legs: vec![
                CycleLeg {
                    pool_id: pool_id(1),
                    token_in: Arc::from("A"),
                    token_out: Arc::from("B"),
                },
                CycleLeg {
                    pool_id: pool_id(2),
                    token_in: Arc::from("B"),
                    token_out: Arc::from("C"),
                },
                CycleLeg {
                    pool_id: pool_id(3),
                    token_in: Arc::from("C"),
                    token_out: Arc::from("A"),
                },
            ],
        };

        let rotated = cycle.rotate_to_profit_token(&Arc::from("B")).unwrap();
        assert_eq!(rotated.profit_token().as_ref(), "B");
        assert_eq!(rotated.legs[0].token_in.as_ref(), "B");
        assert_eq!(rotated.legs[0].token_out.as_ref(), "C");
        assert_eq!(rotated.legs[2].token_out.as_ref(), "B");

        // Token not in cycle
        assert!(cycle.rotate_to_profit_token(&Arc::from("X")).is_none());
    }
}
