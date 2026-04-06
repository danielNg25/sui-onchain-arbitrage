use std::collections::HashSet;
use std::sync::Arc;

use arb_types::config::AppConfig;
use arb_types::pool::object_id_to_hex;
use dex_common::DexRegistry;
use sui_client::SuiClient;

fn make_client() -> SuiClient {
    SuiClient::new("https://fullnode.mainnet.sui.io:443")
}

#[tokio::test]
#[ignore] // requires network
async fn discover_cetus_pools_via_registry() {
    let config = AppConfig::load("../../config/mainnet.toml").expect("config load failed");
    let client = Arc::new(make_client());
    let registry = dex_cetus::CetusRegistry::new(&config.cetus);

    let whitelisted: HashSet<String> = config
        .strategy
        .whitelisted_tokens
        .iter()
        .cloned()
        .collect();

    let pools = registry
        .discover_pools(&client, &whitelisted)
        .await
        .expect("discovery failed");

    println!("\n=== Cetus Pool Discovery ===");
    println!("  Total pools: {}", pools.len());
    for (id, coin_a, coin_b) in &pools {
        println!("  {} | {} / {}", object_id_to_hex(id), coin_a, coin_b);
    }

    assert!(!pools.is_empty(), "should discover at least some Cetus pools");

    // Test pool handle
    let (first_id, _, _) = &pools[0];
    let pool = registry.pool(first_id).expect("pool handle should exist");
    println!("\n  First pool:");
    println!("    dex:      {:?}", pool.dex());
    println!("    coins:    {:?}", pool.coins());
    println!("    active:   {}", pool.is_active());
    println!("    fee_rate: {} PPM", pool.fee_rate());
    assert!(pool.is_active());
}

#[tokio::test]
#[ignore] // requires network
async fn fetch_cetus_pool_ticks_via_trait() {
    let config = AppConfig::load("../../config/mainnet.toml").expect("config load failed");
    let client = Arc::new(make_client());
    let registry = dex_cetus::CetusRegistry::new(&config.cetus);

    let whitelisted: HashSet<String> = config
        .strategy
        .whitelisted_tokens
        .iter()
        .cloned()
        .collect();

    let pools = registry
        .discover_pools(&client, &whitelisted)
        .await
        .expect("discovery failed");

    assert!(!pools.is_empty());

    let (first_id, _, _) = &pools[0];
    let pool = registry.pool(first_id).unwrap();

    pool.fetch_price_data(&client)
        .await
        .expect("tick fetch failed");

    println!("Ticks fetched successfully for pool {}", object_id_to_hex(first_id));
}

#[tokio::test]
#[ignore] // requires network
async fn discover_all_pools_via_manager() {
    let config = AppConfig::load("../../config/mainnet.toml").expect("config load failed");
    let client = Arc::new(make_client());

    let cetus = Arc::new(dex_cetus::CetusRegistry::new(&config.cetus)) as Arc<dyn DexRegistry>;
    let turbos = Arc::new(dex_turbos::TurbosRegistry::new(&config.turbos)) as Arc<dyn DexRegistry>;

    let manager = pool_manager::PoolManager::new(client, vec![cetus, turbos]);

    let whitelisted: HashSet<String> = config
        .strategy
        .whitelisted_tokens
        .iter()
        .cloned()
        .collect();

    let checkpoint = manager
        .discover_all_pools(&whitelisted)
        .await
        .expect("discovery failed");

    println!("\n=== Pool Discovery via Manager ===");
    println!("  Snapshot checkpoint: {}", checkpoint);
    println!("  Total pools:        {}", manager.pool_count());

    assert!(checkpoint > 0);
    assert!(manager.pool_count() > 0);
}

#[tokio::test]
#[ignore] // requires network
async fn ingest_and_query_cetus_pool() {
    let config = AppConfig::load("../../config/mainnet.toml").expect("config load failed");
    let client = make_client();
    let registry = dex_cetus::CetusRegistry::new(&config.cetus);

    // Fetch a known Cetus SUI/USDC pool
    let pool_id_str = "0xcf994611fd4c48e277ce3ffd4d4364c914af2c3cbb05f7bf6facd371de688630";
    let resp = client
        .get_object(pool_id_str, sui_client::ObjectDataOptions::bcs())
        .await
        .unwrap();

    let data = resp.data.unwrap();
    let bcs_bytes = data.bcs_bytes().unwrap();
    let type_params = dex_common::parse_type_params(data.bcs_type().unwrap());
    let object_id = arb_types::pool::object_id_from_hex(pool_id_str).unwrap();

    let result = registry
        .ingest_pool_object(
            object_id,
            &bcs_bytes,
            &type_params,
            data.version_number(),
            data.initial_shared_version().unwrap_or(0),
        )
        .unwrap();

    assert!(result.is_some());
    let (id, coin_a, coin_b) = result.unwrap();

    println!("\n=== Ingested Cetus Pool ===");
    println!("  id:     {}", arb_types::pool::object_id_to_hex(&id));
    println!("  coin_a: {}", coin_a);
    println!("  coin_b: {}", coin_b);

    // Query via Pool trait
    let pool = registry.pool(&id).expect("pool should exist");
    println!("  dex:    {:?}", pool.dex());
    println!("  coins:  {:?}", pool.coins());
    println!("  active: {}", pool.is_active());
    println!("  fee:    {} PPM", pool.fee_rate());

    assert!(pool.is_active());
    assert_eq!(pool.coins().len(), 2);

    // Verify registry indexes
    assert_eq!(registry.pool_count(), 1);
    assert!(!registry.pools_for_token(&coin_a).is_empty());
    assert!(!registry.pools_for_token(&coin_b).is_empty());
}

#[tokio::test]
#[ignore] // requires network
async fn fetch_checkpoint_number() {
    let client = make_client();
    let checkpoint = client
        .get_latest_checkpoint_sequence_number()
        .await
        .expect("failed to get checkpoint");
    println!("Latest checkpoint: {}", checkpoint);
    assert!(checkpoint > 0);
}
