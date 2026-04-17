use std::collections::HashSet;
use std::sync::Arc;

use arb_types::config::AppConfig;
use arb_types::pool::object_id_to_hex;
use dex_common::DexRegistry;

fn make_client() -> sui_client::SuiClient {
    let config = AppConfig::load("../../config/mainnet.toml").unwrap();
    sui_client::SuiClient::new(&config.network.rpc_url)
}

fn load_config() -> AppConfig {
    AppConfig::load("../../config/mainnet.toml").unwrap()
}

/// Build ArbGraph from mainnet pools and verify structure.
#[tokio::test]
#[ignore]
async fn test_graph_from_mainnet_pools() {
    let config = load_config();
    let client = Arc::new(make_client());

    let cetus = Arc::new(dex_cetus::CetusRegistry::new(&config.cetus)) as Arc<dyn DexRegistry>;
    let turbos = Arc::new(dex_turbos::TurbosRegistry::new(&config.turbos)) as Arc<dyn DexRegistry>;
    let manager = pool_manager::PoolManager::new(client.clone(), vec![cetus, turbos]);

    let whitelisted: HashSet<String> = config
        .strategy
        .whitelisted_tokens
        .iter()
        .cloned()
        .collect();

    let _checkpoint = manager.discover_all_pools(&whitelisted).await.unwrap();

    let graph = arb_engine::graph::ArbGraph::build(&manager);

    println!("Token count: {}", graph.token_count());
    println!("Edge count: {}", graph.edge_count());

    assert!(graph.token_count() > 0, "should discover at least one token");
    assert!(graph.edge_count() > 0, "should discover at least one edge");
}

/// Find arbitrage cycles from mainnet pools.
#[tokio::test]
#[ignore]
async fn test_find_cycles_from_mainnet() {
    let config = load_config();
    let client = Arc::new(make_client());

    let cetus = Arc::new(dex_cetus::CetusRegistry::new(&config.cetus)) as Arc<dyn DexRegistry>;
    let turbos = Arc::new(dex_turbos::TurbosRegistry::new(&config.turbos)) as Arc<dyn DexRegistry>;
    let manager = pool_manager::PoolManager::new(client.clone(), vec![cetus, turbos]);

    let whitelisted: HashSet<String> = config
        .strategy
        .whitelisted_tokens
        .iter()
        .cloned()
        .collect();

    let _checkpoint = manager.discover_all_pools(&whitelisted).await.unwrap();

    let graph = arb_engine::graph::ArbGraph::build(&manager);

    let profit_tokens = arb_engine::profit_token::ProfitTokenRegistry::from_config(
        &config.strategy.profit_tokens,
    );
    let pt_types = profit_tokens.ordered_profit_tokens();

    let cycle_index =
        arb_engine::cycle::find_all_cycles(&graph, config.strategy.max_hops, &pt_types);

    println!("Total cycles found: {}", cycle_index.len());

    // Log breakdown by hop count
    let mut by_length = std::collections::HashMap::new();
    for rc in cycle_index.iter() {
        *by_length.entry(rc.cycle.len()).or_insert(0usize) += 1;
    }
    for (len, count) in &by_length {
        println!("  {}-hop cycles: {}", len, count);
    }

    // Log a few example cycles
    for (i, rc) in cycle_index.iter().enumerate().take(5) {
        let tokens: Vec<String> = rc
            .cycle
            .legs
            .iter()
            .map(|l| l.token_in.to_string())
            .collect();
        let pools: Vec<String> = rc
            .cycle
            .legs
            .iter()
            .map(|l| object_id_to_hex(&l.pool_id))
            .collect();
        println!("Cycle {}: tokens={:?}, pools={:?}", i, tokens, pools);
    }

    assert!(cycle_index.len() > 0, "should find at least one arbitrage cycle");
}

/// Build full ArbEngine and verify it initializes.
#[tokio::test]
#[ignore]
async fn test_arb_engine_build_from_mainnet() {
    let config = load_config();
    let client = Arc::new(make_client());

    let cetus = Arc::new(dex_cetus::CetusRegistry::new(&config.cetus)) as Arc<dyn DexRegistry>;
    let turbos = Arc::new(dex_turbos::TurbosRegistry::new(&config.turbos)) as Arc<dyn DexRegistry>;
    let manager = pool_manager::PoolManager::new(client.clone(), vec![cetus, turbos]);

    let whitelisted: HashSet<String> = config
        .strategy
        .whitelisted_tokens
        .iter()
        .cloned()
        .collect();

    let _checkpoint = manager.discover_all_pools(&whitelisted).await.unwrap();

    let profit_registry = Arc::new(
        arb_engine::profit_token::ProfitTokenRegistry::from_config(&config.strategy.profit_tokens),
    );

    let manager = Arc::new(manager);
    let engine =
        arb_engine::ArbEngine::build(manager, profit_registry, &config.strategy).unwrap();

    println!("Engine cycle count: {}", engine.cycle_count());
    assert!(engine.cycle_count() > 0, "engine should find cycles");
}

/// Simulate a cycle with a known amount (requires tick data).
#[tokio::test]
#[ignore]
async fn test_simulate_cycle_from_mainnet() {
    let config = load_config();
    let client = Arc::new(make_client());

    let cetus = Arc::new(dex_cetus::CetusRegistry::new(&config.cetus)) as Arc<dyn DexRegistry>;
    let turbos = Arc::new(dex_turbos::TurbosRegistry::new(&config.turbos)) as Arc<dyn DexRegistry>;
    let manager = pool_manager::PoolManager::new(client.clone(), vec![cetus, turbos]);

    let whitelisted: HashSet<String> = config
        .strategy
        .whitelisted_tokens
        .iter()
        .cloned()
        .collect();

    let _checkpoint = manager.discover_all_pools(&whitelisted).await.unwrap();

    // Fetch ticks for all pools
    for registry in manager.registries() {
        for pool_id in registry.pool_ids() {
            if let Some(pool) = registry.pool(&pool_id) {
                let _ = pool.fetch_price_data(&client).await;
            }
        }
    }

    let profit_registry = Arc::new(
        arb_engine::profit_token::ProfitTokenRegistry::from_config(&config.strategy.profit_tokens),
    );
    let pt_types = profit_registry.ordered_profit_tokens();

    let graph = arb_engine::graph::ArbGraph::build(&manager);
    let cycle_index =
        arb_engine::cycle::find_all_cycles(&graph, config.strategy.max_hops, &pt_types);

    if cycle_index.is_empty() {
        println!("No cycles found, skipping simulation test");
        return;
    }

    // Simulate first cycle with 1 SUI (1_000_000_000 mist)
    let sim_cache = arb_engine::simulator::SimCache::new();
    let rc = cycle_index.get(0);

    let result =
        arb_engine::simulator::simulate_cycle(&rc.cycle, 1_000_000_000, &manager, &sim_cache);

    match result {
        Some((output, profit)) => {
            println!(
                "Cycle 0: input=1_000_000_000, output={}, profit={}",
                output, profit
            );
        }
        None => {
            println!("Cycle 0: simulation returned None (dead path or zero output)");
        }
    }
}
