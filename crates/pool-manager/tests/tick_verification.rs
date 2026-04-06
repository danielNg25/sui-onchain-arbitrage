use std::sync::Arc;

use arb_types::config::AppConfig;
use arb_types::pool::object_id_from_hex;
use dex_common::DexRegistry;
use sui_client::{EventFilter, ObjectDataOptions, SuiClient};

fn make_client() -> Arc<SuiClient> {
    Arc::new(SuiClient::new("https://fullnode.mainnet.sui.io:443"))
}

/// Test Cetus tick fetching with full verification
#[tokio::test]
#[ignore]
async fn verify_cetus_ticks() {
    let config = AppConfig::load("../../config/mainnet.toml").unwrap();
    let client = make_client();
    let registry = dex_cetus::CetusRegistry::new(&config.cetus);

    let pool_id_str = "0xcf994611fd4c48e277ce3ffd4d4364c914af2c3cbb05f7bf6facd371de688630";
    ingest_pool(&client, &registry, pool_id_str).await;

    let object_id = object_id_from_hex(pool_id_str).unwrap();
    let pool = registry.pool(&object_id).unwrap();

    pool.fetch_price_data(&client).await.expect("tick fetch failed");

    // Access internal ticks via a second fetch to verify
    // We use the CetusPool's internal ticks through the raw module
    let ticks = dex_cetus::fetch_ticks_for_pool(&client, &registry, &object_id)
        .await
        .expect("tick re-fetch failed");

    println!("\n=== Cetus Tick Verification ===");
    println!("  Pool: {}", pool_id_str);
    println!("  Total ticks: {}", ticks.len());
    assert!(!ticks.is_empty(), "SUI/USDC pool should have ticks");

    if let Some(first) = ticks.first() {
        println!("  First: index={}, liq_net={}, liq_gross={}, sqrt_price={}",
            first.index, first.liquidity_net, first.liquidity_gross, first.sqrt_price);
    }
    if let Some(last) = ticks.last() {
        println!("  Last:  index={}, liq_net={}, liq_gross={}, sqrt_price={}",
            last.index, last.liquidity_net, last.liquidity_gross, last.sqrt_price);
    }

    // Verify sorted
    for w in ticks.windows(2) {
        assert!(w[0].index < w[1].index, "ticks must be sorted by index");
    }

    // Verify liquidity_net sums to ~0 (total added = total removed across all ticks)
    let net_sum: i128 = ticks.iter().map(|t| t.liquidity_net).sum();
    println!("  Sum of liquidity_net: {}", net_sum);
    // Allow small rounding — should be very close to 0
    assert!(
        net_sum.unsigned_abs() < 1_000_000,
        "liquidity_net should sum to ~0, got {}",
        net_sum
    );

    println!("  ALL CHECKS PASSED");
}

/// Find a liquid Turbos pool and verify tick fetching
#[tokio::test]
#[ignore]
async fn verify_turbos_ticks() {
    let config = AppConfig::load("../../config/mainnet.toml").unwrap();
    let client = make_client();

    // Find a Turbos pool with activity by querying recent swap events
    let events = client
        .query_events(
            EventFilter::MoveEventType(
                "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1::pool::SwapEvent".to_string(),
            ),
            None,
            Some(5),
            true, // most recent
        )
        .await
        .expect("query Turbos SwapEvent failed");

    println!("Recent Turbos swap events: {}", events.data.len());

    let mut turbos_pool_id = None;
    for event in &events.data {
        if let Some(json) = &event.parsed_json {
            if let Some(pool_str) = json["pool"].as_str() {
                println!("  Turbos pool with recent swap: {}", pool_str);
                turbos_pool_id = Some(pool_str.to_string());
                break;
            }
        }
    }

    let pool_id_str = turbos_pool_id.expect("no Turbos pool found with recent swaps");
    println!("\nUsing Turbos pool: {}", pool_id_str);

    let registry = dex_turbos::TurbosRegistry::new(&config.turbos);
    ingest_pool(&client, &registry, &pool_id_str).await;

    let object_id = object_id_from_hex(&pool_id_str).unwrap();
    let pool = registry.pool(&object_id).unwrap();

    println!("  dex:    {:?}", pool.dex());
    println!("  coins:  {:?}", pool.coins());
    println!("  fee:    {} PPM", pool.fee_rate());
    println!("  active: {}", pool.is_active());

    println!("\nFetching Turbos ticks...");
    pool.fetch_price_data(&client).await.expect("tick fetch failed");

    // Re-fetch to get count
    let ticks = dex_turbos::fetch_ticks_for_pool(&client, &registry, &object_id)
        .await
        .expect("tick re-fetch failed");

    println!("\n=== Turbos Tick Verification ===");
    println!("  Total ticks: {}", ticks.len());

    if ticks.is_empty() {
        println!("  WARNING: no initialized ticks found");
        return;
    }

    if let Some(first) = ticks.first() {
        println!("  First: index={}", first.index);
    }
    if let Some(last) = ticks.last() {
        println!("  Last:  index={}", last.index);
    }

    // Verify sorted
    for w in ticks.windows(2) {
        assert!(w[0].index < w[1].index, "ticks must be sorted by index");
    }

    // Turbos ticks from bitmap only have indices (liquidity_net/gross are 0 until Phase 2)
    println!("  Note: liquidity data populated via devInspect in Phase 2");
    println!("  ALL CHECKS PASSED");
}

async fn ingest_pool<R: dex_common::DexRegistry>(
    client: &SuiClient,
    registry: &R,
    pool_id_str: &str,
) {
    let resp = client
        .get_object(pool_id_str, ObjectDataOptions::bcs())
        .await
        .expect("fetch pool failed");

    let data = resp.data.expect("no object data");
    let bcs_bytes = data.bcs_bytes().expect("no BCS");
    let type_str = data.bcs_type().expect("no type");

    let (coin_params, fee_type) = dex_common::parse_type_params_with_fee(type_str);
    let mut type_params = coin_params;
    if let Some(ft) = fee_type {
        type_params.push(ft);
    }

    let object_id = object_id_from_hex(pool_id_str).unwrap();
    registry
        .ingest_pool_object(
            object_id,
            &bcs_bytes,
            &type_params,
            data.version_number(),
            data.initial_shared_version().unwrap_or(0),
        )
        .expect("ingest failed");
}
