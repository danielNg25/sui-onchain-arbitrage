use std::collections::{HashMap, HashSet};

use arb_types::pool::{CoinType, ObjectId};

/// An edge in the token graph: a pool connecting two tokens.
#[derive(Debug, Clone)]
pub struct PoolEdge {
    pub pool_id: ObjectId,
    pub token_in: CoinType,
    pub token_out: CoinType,
    pub fee_rate: u64,
}

/// Token adjacency graph built from all discovered pools.
/// Each node is a CoinType, each edge is a pool connecting two tokens.
pub struct ArbGraph {
    /// token -> list of outgoing edges
    adjacency: HashMap<CoinType, Vec<PoolEdge>>,
    /// All unique tokens
    tokens: HashSet<CoinType>,
}

impl ArbGraph {
    /// Build graph from the pool manager's discovered pools.
    /// Iterates all registries, gets each pool's coins and fee_rate,
    /// and inserts bidirectional edges.
    pub fn build(pool_manager: &pool_manager::PoolManager) -> Self {
        let mut adjacency: HashMap<CoinType, Vec<PoolEdge>> = HashMap::new();
        let mut tokens = HashSet::new();

        for registry in pool_manager.registries() {
            for pool_id in registry.pool_ids() {
                let pool = match registry.pool(&pool_id) {
                    Some(p) => p,
                    None => continue,
                };

                if !pool.is_active() {
                    continue;
                }

                let coins = pool.coins();
                if coins.len() < 2 {
                    continue;
                }

                let token_a = &coins[0];
                let token_b = &coins[1];
                let fee_rate = pool.fee_rate();

                tokens.insert(token_a.clone());
                tokens.insert(token_b.clone());

                // A -> B
                adjacency.entry(token_a.clone()).or_default().push(PoolEdge {
                    pool_id,
                    token_in: token_a.clone(),
                    token_out: token_b.clone(),
                    fee_rate,
                });

                // B -> A
                adjacency.entry(token_b.clone()).or_default().push(PoolEdge {
                    pool_id,
                    token_in: token_b.clone(),
                    token_out: token_a.clone(),
                    fee_rate,
                });
            }
        }

        Self { adjacency, tokens }
    }

    /// Build graph from explicit edges (for testing).
    pub fn from_edges(edges: Vec<PoolEdge>) -> Self {
        let mut adjacency: HashMap<CoinType, Vec<PoolEdge>> = HashMap::new();
        let mut tokens = HashSet::new();

        for edge in edges {
            tokens.insert(edge.token_in.clone());
            tokens.insert(edge.token_out.clone());
            adjacency
                .entry(edge.token_in.clone())
                .or_default()
                .push(edge);
        }

        Self { adjacency, tokens }
    }

    /// Get all outgoing edges from a token.
    pub fn neighbors(&self, token: &CoinType) -> &[PoolEdge] {
        static EMPTY: Vec<PoolEdge> = Vec::new();
        self.adjacency.get(token).map_or(&EMPTY, |v| v.as_slice())
    }

    /// All unique tokens in the graph.
    pub fn tokens(&self) -> &HashSet<CoinType> {
        &self.tokens
    }

    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    /// Total number of directed edges.
    pub fn edge_count(&self) -> usize {
        self.adjacency.values().map(|v| v.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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

    #[test]
    fn test_graph_from_edges() {
        let edges = vec![
            make_edge(1, "SUI", "USDC"),
            make_edge(1, "USDC", "SUI"),
            make_edge(2, "USDC", "USDT"),
            make_edge(2, "USDT", "USDC"),
            make_edge(3, "USDT", "SUI"),
            make_edge(3, "SUI", "USDT"),
        ];
        let graph = ArbGraph::from_edges(edges);

        assert_eq!(graph.token_count(), 3);
        assert_eq!(graph.edge_count(), 6);
        assert_eq!(graph.neighbors(&Arc::from("SUI")).len(), 2);
        assert_eq!(graph.neighbors(&Arc::from("USDC")).len(), 2);
    }

    #[test]
    fn test_empty_graph() {
        let graph = ArbGraph::from_edges(vec![]);
        assert_eq!(graph.token_count(), 0);
        assert_eq!(graph.edge_count(), 0);
        assert!(graph.neighbors(&Arc::from("SUI")).is_empty());
    }
}
