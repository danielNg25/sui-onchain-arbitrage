use std::sync::Arc;

use arb_types::config::AppConfig;
use arb_types::pool::{object_id_from_hex, object_id_to_hex, Dex};
use dex_common::{parse_type_params, PoolDeserializer};
use sui_client::{ObjectDataOptions, SuiClient};

/// Known Cetus SUI/USDC pool on mainnet.
const CETUS_SUI_USDC_POOL: &str =
    "0xcf994611fd4c48e277ce3ffd4d4364c914af2c3cbb05f7bf6facd371de688630";

/// Known Turbos SUI/USDC pool on mainnet.
#[allow(dead_code)]
const TURBOS_SUI_USDC_POOL: &str =
    "0x5eb2dfcdd1b15d2021c5f592ab3ae9de5ff1064fa2e08e9fb76245b05e48fcb7";

fn make_client() -> SuiClient {
    SuiClient::new("https://fullnode.mainnet.sui.io:443")
}

#[tokio::test]
#[ignore] // requires network
async fn fetch_and_deserialize_cetus_pool() {
    let client = make_client();

    let resp = client
        .get_object(CETUS_SUI_USDC_POOL, ObjectDataOptions::bcs())
        .await
        .expect("RPC call failed");

    let data = resp.data.expect("no object data");
    let bcs_bytes = data.bcs_bytes().expect("no BCS bytes");
    let type_str = data.bcs_type().expect("no type string");
    let type_params = parse_type_params(type_str);

    println!("Type: {}", type_str);
    println!("Type params: {:?}", type_params);
    println!("BCS bytes length: {}", bcs_bytes.len());
    println!("Version: {}", data.version);
    println!("Initial shared version: {:?}", data.initial_shared_version());

    let pool = dex_cetus::CetusDeserializer::deserialize_pool(
        object_id_from_hex(CETUS_SUI_USDC_POOL).unwrap(),
        &bcs_bytes,
        &type_params,
        data.version_number(),
        data.initial_shared_version().unwrap_or(0),
    )
    .expect("deserialization failed");

    println!("\n=== Cetus SUI/USDC Pool ===");
    println!("  ID:           {}", object_id_to_hex(&pool.id));
    println!("  DEX:          {:?}", pool.dex);
    println!("  coin_a:       {}", pool.coin_a);
    println!("  coin_b:       {}", pool.coin_b);
    println!("  sqrt_price:   {}", pool.sqrt_price);
    println!("  tick_current:  {}", pool.tick_current);
    println!("  liquidity:    {}", pool.liquidity);
    println!("  fee_rate:     {} PPM", pool.fee_rate);
    println!("  tick_spacing: {}", pool.tick_spacing);
    println!("  reserve_a:    {}", pool.reserve_a);
    println!("  reserve_b:    {}", pool.reserve_b);
    println!("  is_active:    {}", pool.is_active);
    println!("  ticks_table:  {}", object_id_to_hex(&pool.ticks_table_id));

    assert_eq!(pool.dex, Dex::Cetus);
    assert!(pool.is_active, "pool should be active");
    assert!(pool.liquidity > 0, "pool should have liquidity");
    assert!(pool.sqrt_price > 0, "pool should have sqrt_price");
    assert!(pool.reserve_a > 0 || pool.reserve_b > 0, "pool should have reserves");
}

#[tokio::test]
#[ignore] // requires network
async fn fetch_cetus_pool_ticks() {
    let client = make_client();

    let resp = client
        .get_object(CETUS_SUI_USDC_POOL, ObjectDataOptions::bcs())
        .await
        .expect("RPC call failed");

    let data = resp.data.expect("no object data");
    let bcs_bytes = data.bcs_bytes().expect("no BCS bytes");
    let type_params = parse_type_params(data.bcs_type().unwrap());

    let pool = dex_cetus::CetusDeserializer::deserialize_pool(
        object_id_from_hex(CETUS_SUI_USDC_POOL).unwrap(),
        &bcs_bytes,
        &type_params,
        data.version_number(),
        data.initial_shared_version().unwrap_or(0),
    )
    .unwrap();

    println!("Fetching ticks for ticks_table: {}", object_id_to_hex(&pool.ticks_table_id));

    let ticks =
        <dex_cetus::CetusTickFetcher as dex_common::TickFetcher>::fetch_ticks(&client, &pool)
            .await
            .expect("tick fetch failed");

    println!("\n=== Cetus Pool Ticks ===");
    println!("  Total initialized ticks: {}", ticks.len());
    if let Some(first) = ticks.first() {
        println!("  First tick: index={}, liq_net={}, liq_gross={}", first.index, first.liquidity_net, first.liquidity_gross);
    }
    if let Some(last) = ticks.last() {
        println!("  Last tick:  index={}, liq_net={}, liq_gross={}", last.index, last.liquidity_net, last.liquidity_gross);
    }

    assert!(!ticks.is_empty(), "pool should have ticks");
    // Verify sorted
    for w in ticks.windows(2) {
        assert!(w[0].index < w[1].index, "ticks should be sorted");
    }
}

#[tokio::test]
#[ignore] // requires network
async fn discover_all_pools() {
    let config = AppConfig::load("../../config/mainnet.toml").expect("config load failed");
    let client = Arc::new(SuiClient::new(&config.network.rpc_url));
    let manager = pool_manager::PoolManager::new(client, Arc::new(config));

    let checkpoint = manager
        .discover_all_pools()
        .await
        .expect("discovery failed");

    println!("\n=== Pool Discovery ===");
    println!("  Snapshot checkpoint: {}", checkpoint);
    println!("  Total pools:        {}", manager.pool_count());

    assert!(checkpoint > 0, "should have valid checkpoint");
    assert!(manager.pool_count() > 0, "should discover at least some pools");
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
