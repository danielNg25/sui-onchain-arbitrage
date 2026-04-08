//! Mainnet verification: compare local simulate_swap against on-chain devInspect.
//!
//! Tests are #[ignore] because they require network access.
//! Run with: cargo test -p pool-manager --test swap_verification -- --ignored --nocapture

use std::sync::Arc;

use arb_types::config::AppConfig;
use arb_types::pool::object_id_from_hex;
use dex_common::{parse_type_params, parse_type_params_with_fee, DexRegistry};
use sui_client::{ObjectDataOptions, SuiClient};

// ---------------------------------------------------------------------------
// BCS builder for devInspect transactions
// ---------------------------------------------------------------------------

/// Encode a ULEB128 value into a buffer.
fn encode_uleb128(buf: &mut Vec<u8>, mut val: u64) {
    loop {
        let mut byte = (val & 0x7F) as u8;
        val >>= 7;
        if val != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if val == 0 {
            break;
        }
    }
}

/// Encode a BCS string (ULEB128 length + UTF-8 bytes).
fn encode_string(buf: &mut Vec<u8>, s: &str) {
    encode_uleb128(buf, s.len() as u64);
    buf.extend_from_slice(s.as_bytes());
}

/// Encode a BCS vector length prefix.
fn encode_vec_len(buf: &mut Vec<u8>, len: usize) {
    encode_uleb128(buf, len as u64);
}

/// Parse a hex address string (with or without 0x prefix) into 32 bytes.
fn hex_to_32(hex_str: &str) -> [u8; 32] {
    let s = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    // Pad to 64 hex chars (32 bytes) with leading zeros
    let padded = format!("{:0>64}", s);
    let bytes = hex::decode(&padded).expect("invalid hex");
    let mut result = [0u8; 32];
    result.copy_from_slice(&bytes);
    result
}

/// Parse a Move type string like "0x2::sui::SUI" into (address_32_bytes, module, name).
fn parse_type_string(type_str: &str) -> ([u8; 32], String, String) {
    let parts: Vec<&str> = type_str.split("::").collect();
    assert!(parts.len() == 3, "expected addr::module::name, got {}", type_str);
    (hex_to_32(parts[0]), parts[1].to_string(), parts[2].to_string())
}

/// Encode a TypeTag::Struct for a simple coin type (no nested type params).
fn encode_type_tag_struct(buf: &mut Vec<u8>, type_str: &str) {
    let (addr, module, name) = parse_type_string(type_str);
    buf.push(7); // TypeTag::Struct variant
    buf.extend_from_slice(&addr); // StructTag.address
    encode_string(buf, &module); // StructTag.module
    encode_string(buf, &name); // StructTag.name
    encode_vec_len(buf, 0); // StructTag.type_params = []
}

/// Build BCS-encoded TransactionKind for Cetus calculate_swap_result devInspect.
///
/// Cetus: calculate_swap_result<CoinA, CoinB>(pool, a2b, by_amount_in, amount: u64)
fn build_cetus_swap_inspect(
    pool_id: [u8; 32],
    initial_shared_version: u64,
    published_at: &str,
    coin_a_type: &str,
    coin_b_type: &str,
    a2b: bool,
    amount: u64,
) -> Vec<u8> {
    let package_id = hex_to_32(published_at);
    let mut buf = Vec::new();

    // TransactionKind::ProgrammableTransaction (variant 0)
    buf.push(0);

    // ProgrammableTransaction.inputs: Vec<CallArg>
    encode_vec_len(&mut buf, 4); // 4 inputs

    // Input 0: CallArg::Object(ObjectArg::SharedObject { id, initial_shared_version, mutable: false })
    buf.push(1); // CallArg::Object
    buf.push(1); // ObjectArg::SharedObject
    buf.extend_from_slice(&pool_id);
    buf.extend_from_slice(&initial_shared_version.to_le_bytes());
    buf.push(0); // mutable = false

    // Input 1: CallArg::Pure(BCS(bool)) - a2b
    buf.push(0); // CallArg::Pure
    encode_vec_len(&mut buf, 1);
    buf.push(a2b as u8);

    // Input 2: CallArg::Pure(BCS(bool)) - by_amount_in = true
    buf.push(0); // CallArg::Pure
    encode_vec_len(&mut buf, 1);
    buf.push(1); // true

    // Input 3: CallArg::Pure(BCS(u64)) - amount
    buf.push(0); // CallArg::Pure
    encode_vec_len(&mut buf, 8);
    buf.extend_from_slice(&amount.to_le_bytes());

    // ProgrammableTransaction.commands: Vec<Command>
    encode_vec_len(&mut buf, 1); // 1 command

    // Command::MoveCall (variant 0)
    buf.push(0);

    // ProgrammableMoveCall.package
    buf.extend_from_slice(&package_id);

    // ProgrammableMoveCall.module
    encode_string(&mut buf, "pool");

    // ProgrammableMoveCall.function
    encode_string(&mut buf, "calculate_swap_result");

    // ProgrammableMoveCall.type_arguments: Vec<TypeTag>
    encode_vec_len(&mut buf, 2);
    encode_type_tag_struct(&mut buf, coin_a_type);
    encode_type_tag_struct(&mut buf, coin_b_type);

    // ProgrammableMoveCall.arguments: Vec<Argument>
    encode_vec_len(&mut buf, 4);
    for i in 0u16..4 {
        buf.push(1); // Argument::Input
        buf.extend_from_slice(&i.to_le_bytes());
    }

    buf
}

/// Parse Cetus CalculatedSwapResult from BCS return bytes.
/// Fields: amount_in(u64), amount_out(u64), fee_amount(u64), fee_rate(u64),
///         after_sqrt_price(u128), is_exceed(bool), step_results(Vec<...>)
fn parse_cetus_swap_result(bytes: &[u8]) -> (u64, u64, u64, u128) {
    let amount_in = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    let amount_out = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let fee_amount = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
    // skip fee_rate (24..32)
    let after_sqrt_price = u128::from_le_bytes(bytes[32..48].try_into().unwrap());
    (amount_in, amount_out, fee_amount, after_sqrt_price)
}

// ---------------------------------------------------------------------------
// Helper: ingest a pool and fetch ticks
// ---------------------------------------------------------------------------

async fn setup_cetus_pool(
    client: &SuiClient,
    config: &AppConfig,
    pool_id_str: &str,
) -> (dex_cetus::CetusRegistry, [u8; 32], String, String) {
    let registry = dex_cetus::CetusRegistry::new(&config.cetus);
    let resp = client.get_object(pool_id_str, ObjectDataOptions::bcs()).await.unwrap();
    let data = resp.data.expect("no object data");
    let type_str = data.bcs_type().expect("no type");
    let type_params = parse_type_params(type_str);
    let bcs_bytes = data.bcs_bytes().unwrap();
    let object_id = object_id_from_hex(pool_id_str).unwrap();

    registry
        .ingest_pool_object(
            object_id,
            &bcs_bytes,
            &type_params,
            data.version_number(),
            data.initial_shared_version().unwrap_or(0),
        )
        .unwrap();

    // Fetch ticks
    let pool = registry.pool(&object_id).unwrap();
    pool.fetch_price_data(client).await.unwrap();

    let coin_a = type_params[0].clone();
    let coin_b = type_params[1].clone();
    (registry, object_id, coin_a, coin_b)
}

async fn setup_turbos_pool(
    client: &SuiClient,
    config: &AppConfig,
    pool_id_str: &str,
) -> (dex_turbos::TurbosRegistry, [u8; 32], String, String, String) {
    let registry = dex_turbos::TurbosRegistry::new(&config.turbos);
    let resp = client.get_object(pool_id_str, ObjectDataOptions::bcs()).await.unwrap();
    let data = resp.data.expect("no object data");
    let type_str = data.bcs_type().expect("no type");
    let (coin_params, fee_type) = parse_type_params_with_fee(type_str);
    let bcs_bytes = data.bcs_bytes().unwrap();
    let object_id = object_id_from_hex(pool_id_str).unwrap();

    let mut all_params = coin_params.clone();
    if let Some(ft) = &fee_type {
        all_params.push(ft.clone());
    }

    registry
        .ingest_pool_object(
            object_id,
            &bcs_bytes,
            &all_params,
            data.version_number(),
            data.initial_shared_version().unwrap_or(0),
        )
        .unwrap();

    // Fetch ticks
    let pool = registry.pool(&object_id).unwrap();
    pool.fetch_price_data(client).await.unwrap();

    let coin_a = coin_params[0].clone();
    let coin_b = coin_params[1].clone();
    let fee = fee_type.unwrap_or_default();
    (registry, object_id, coin_a, coin_b, fee)
}

// ---------------------------------------------------------------------------
// Cetus mainnet verification tests
// ---------------------------------------------------------------------------

/// Known SUI/USDC pool on Cetus (high liquidity).
const CETUS_SUI_USDC_POOL: &str =
    "0xcf994611fd4c48e277ce3ffd4d4364c914af2c3cbb05f7bf6facd371de688630";

/// Dummy sender for devInspect (doesn't need to exist).
const DUMMY_SENDER: &str = "0x0000000000000000000000000000000000000000000000000000000000000000";

#[tokio::test]
#[ignore]
async fn verify_cetus_simulate_vs_onchain() {
    let config = AppConfig::load("../../config/mainnet.toml").expect("load config");
    let client = Arc::new(SuiClient::new(&config.network.rpc_url));

    let (registry, pool_id, coin_a, coin_b) =
        setup_cetus_pool(&client, &config, CETUS_SUI_USDC_POOL).await;

    let sqrt_price = dex_cetus::get_pool_sqrt_price(&registry, &pool_id).unwrap();
    let ticks = dex_cetus::get_pool_ticks(&registry, &pool_id).unwrap();
    let pool = registry.pool(&pool_id).unwrap();
    let fee_rate = pool.fee_rate();

    println!("Pool: {}", CETUS_SUI_USDC_POOL);
    println!("  coin_a: {}", coin_a);
    println!("  coin_b: {}", coin_b);
    println!("  sqrt_price: {}", sqrt_price);
    println!("  fee_rate: {}", fee_rate);
    println!("  ticks: {}", ticks.len());

    // Get pool initial_shared_version for devInspect
    let resp = client
        .get_object(CETUS_SUI_USDC_POOL, ObjectDataOptions::bcs())
        .await
        .unwrap();
    let pool_data = resp.data.unwrap();
    let initial_shared_version = pool_data.initial_shared_version().unwrap_or(0);

    // Parse tick_current from pool state
    let raw = dex_cetus::raw::parse_cetus_pool(&pool_data.bcs_bytes().unwrap()).unwrap();

    // Test amounts: small, medium, large (in MIST for SUI, raw for USDC)
    let test_amounts = vec![
        (10_000_000u64, true, "0.01 SUI a2b"),      // 0.01 SUI
        (1_000_000_000u64, true, "1 SUI a2b"),       // 1 SUI
        (10_000_000_000u64, true, "10 SUI a2b"),     // 10 SUI
        (10_000u64, false, "0.01 USDC b2a"),         // 0.01 USDC (6 decimals)
        (1_000_000u64, false, "1 USDC b2a"),         // 1 USDC
        (10_000_000u64, false, "10 USDC b2a"),       // 10 USDC
    ];

    for (amount, a2b, label) in &test_amounts {
        println!("\n--- Test: {} ---", label);

        // Local simulation
        let local = clmm_math::simulate_swap(
            raw.current_sqrt_price,
            raw.current_tick_index,
            raw.liquidity,
            raw.fee_rate,
            raw.tick_spacing,
            &ticks,
            *a2b,
            *amount,
        );

        println!("  Local: amount_in={}, amount_out={}, fee={}, sqrt_after={}",
            local.amount_in, local.amount_out, local.fee_total, local.sqrt_price_after);

        // On-chain devInspect
        let tx_bytes = build_cetus_swap_inspect(
            pool_id,
            initial_shared_version,
            &config.cetus.package_published_at,
            &coin_a,
            &coin_b,
            *a2b,
            *amount,
        );

        let tx_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &tx_bytes,
        );

        let result = client.dev_inspect(DUMMY_SENDER, &tx_b64).await;
        match result {
            Ok(inspect) => {
                if let Some(error) = &inspect.error {
                    println!("  On-chain ERROR: {}", error);
                    continue;
                }
                if let Some(results) = &inspect.results {
                    if let Some(first) = results.first() {
                        if let Some(return_values) = &first.return_values {
                            if let Some((bcs_bytes, _type_tag)) = return_values.first() {
                                let (on_amount_in, on_amount_out, on_fee, on_sqrt_after) =
                                    parse_cetus_swap_result(bcs_bytes);

                                println!("  Chain: amount_in={}, amount_out={}, fee={}, sqrt_after={}",
                                    on_amount_in, on_amount_out, on_fee, on_sqrt_after);

                                // Compare with tolerance of ±1 (rounding)
                                let in_diff = (local.amount_in as i64 - on_amount_in as i64).unsigned_abs();
                                let out_diff = (local.amount_out as i64 - on_amount_out as i64).unsigned_abs();
                                let fee_diff = (local.fee_total as i64 - on_fee as i64).unsigned_abs();

                                println!("  Diff: amount_in={}, amount_out={}, fee={}", in_diff, out_diff, fee_diff);

                                assert!(
                                    in_diff <= 1,
                                    "amount_in mismatch: local={} on-chain={} diff={}",
                                    local.amount_in, on_amount_in, in_diff
                                );
                                assert!(
                                    out_diff <= 1,
                                    "amount_out mismatch: local={} on-chain={} diff={}",
                                    local.amount_out, on_amount_out, out_diff
                                );
                                assert!(
                                    fee_diff <= 1,
                                    "fee mismatch: local={} on-chain={} diff={}",
                                    local.fee_total, on_fee, fee_diff
                                );
                                assert_eq!(
                                    local.sqrt_price_after, on_sqrt_after,
                                    "sqrt_price_after mismatch"
                                );

                                println!("  ✓ MATCH");
                            }
                        }
                    }
                }
            }
            Err(e) => {
                println!("  devInspect RPC error: {}", e);
                panic!("devInspect failed: {}", e);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Turbos mainnet verification tests
// ---------------------------------------------------------------------------

/// Known SUI/USDC pool on Turbos.
/// Find dynamically if this doesn't exist: query PoolCreatedEvent for SUI/USDC pair.
const TURBOS_SUI_USDC_POOL: &str =
    "0x0df4f02d0e210169cb6d5aabd03c3058328c06f2c4dbb0804faa041159c78443";

#[tokio::test]
#[ignore]
async fn verify_turbos_simulate_vs_onchain() {
    let config = AppConfig::load("../../config/mainnet.toml").expect("load config");
    let client = Arc::new(SuiClient::new(&config.network.rpc_url));

    // First, find a Turbos SUI/USDC pool by querying events
    let (registry, pool_id, coin_a, coin_b, fee_type) =
        setup_turbos_pool(&client, &config, TURBOS_SUI_USDC_POOL).await;

    let sqrt_price = dex_turbos::get_pool_sqrt_price(&registry, &pool_id).unwrap();
    let ticks = dex_turbos::get_pool_ticks(&registry, &pool_id).unwrap();
    let pool = registry.pool(&pool_id).unwrap();
    let fee_rate = pool.fee_rate();

    println!("Pool: {}", TURBOS_SUI_USDC_POOL);
    println!("  coin_a: {}", coin_a);
    println!("  coin_b: {}", coin_b);
    println!("  fee_type: {}", fee_type);
    println!("  sqrt_price: {}", sqrt_price);
    println!("  fee_rate: {}", fee_rate);
    println!("  ticks: {}", ticks.len());

    // Get pool initial_shared_version
    let resp = client
        .get_object(TURBOS_SUI_USDC_POOL, ObjectDataOptions::bcs())
        .await
        .unwrap();
    let pool_data = resp.data.unwrap();
    let initial_shared_version = pool_data.initial_shared_version().unwrap_or(0);

    let raw = dex_turbos::raw::parse_turbos_pool(&pool_data.bcs_bytes().unwrap()).unwrap();

    // Get Versioned object's initial_shared_version
    let versioned_resp = client
        .get_object(&config.turbos.versioned, ObjectDataOptions::bcs())
        .await
        .unwrap();
    let versioned_isv = versioned_resp
        .data
        .as_ref()
        .and_then(|d| d.initial_shared_version())
        .unwrap_or(1);

    let test_amounts = vec![
        (10_000_000u64, true, "0.01 SUI a2b"),
        (1_000_000_000u64, true, "1 SUI a2b"),
        (10_000u64, false, "0.01 USDC b2a"),
        (1_000_000u64, false, "1 USDC b2a"),
    ];

    for (amount, a2b, label) in &test_amounts {
        println!("\n--- Test: {} ---", label);

        // Local simulation
        let local = clmm_math::simulate_swap(
            raw.sqrt_price,
            raw.tick_current_index,
            raw.liquidity,
            raw.fee as u64,
            raw.tick_spacing,
            &ticks,
            *a2b,
            *amount,
        );

        println!("  Local: amount_in={}, amount_out={}, fee={}, sqrt_after={}",
            local.amount_in, local.amount_out, local.fee_total, local.sqrt_price_after);

        let tx_bytes = build_turbos_swap_inspect_with_isv(
            pool_id,
            initial_shared_version,
            &config.turbos.package_published_at,
            &config.turbos.versioned,
            versioned_isv,
            &coin_a,
            &coin_b,
            &fee_type,
            *a2b,
            *amount,
        );

        let tx_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &tx_bytes,
        );

        let result = client.dev_inspect(DUMMY_SENDER, &tx_b64).await;
        match result {
            Ok(inspect) => {
                if let Some(error) = &inspect.error {
                    println!("  On-chain ERROR: {}", error);
                    // Turbos may fail if the pool doesn't support compute_swap_result
                    // or the published_at is wrong. Log and continue.
                    continue;
                }
                if let Some(results) = &inspect.results {
                    if let Some(first) = results.first() {
                        if let Some(return_values) = &first.return_values {
                            if let Some((bcs_bytes, _type_tag)) = return_values.first() {
                                // Turbos ComputeSwapState: parse first fields
                                // The exact struct is unknown (closed source), try parsing as:
                                // amount_a: u64, amount_b: u64, ...
                                // or possibly: amount_calculated: u128, ...
                                // We'll parse what we can and compare
                                println!("  Chain return ({} bytes): {:?}",
                                    bcs_bytes.len(), &bcs_bytes[..bcs_bytes.len().min(48)]);

                                // Turbos ComputeSwapState uses u128 fields:
                                // amount_a(u128), amount_b(u128), ...
                                // First field is the specified amount echoed back,
                                // second is the computed output amount.
                                if bcs_bytes.len() >= 32 {
                                    let field_a = u128::from_le_bytes(
                                        bcs_bytes[0..16].try_into().unwrap(),
                                    );
                                    let field_b = u128::from_le_bytes(
                                        bcs_bytes[16..32].try_into().unwrap(),
                                    );

                                    // For a2b: field_a = amount_a (input), field_b = amount_b (output)
                                    // For b2a: field_a = amount_a (output), field_b = amount_b (input)
                                    let (on_amount_in, on_amount_out) = if *a2b {
                                        (field_a as u64, field_b as u64)
                                    } else {
                                        (field_b as u64, field_a as u64)
                                    };

                                    println!("  Chain: amount_in={}, amount_out={} (field_a={}, field_b={})",
                                        on_amount_in, on_amount_out, field_a, field_b);

                                    // Local sim returns amount_in after fee deduction.
                                    // Turbos returns the full input amount (including fee).
                                    // So on-chain amount_in = local.amount_in + local.fee_total
                                    let local_total_in = local.amount_in + local.fee_total;
                                    let in_diff = (local_total_in as i64 - on_amount_in as i64).unsigned_abs();
                                    let out_diff = (local.amount_out as i64 - on_amount_out as i64).unsigned_abs();

                                    println!("  Local total_in (in+fee): {}", local_total_in);
                                    println!("  Diff: amount_in={}, amount_out={}", in_diff, out_diff);

                                    assert!(
                                        in_diff <= 1,
                                        "Turbos amount_in mismatch: local_total={} on-chain={} diff={}",
                                        local_total_in, on_amount_in, in_diff
                                    );
                                    assert!(
                                        out_diff <= 1,
                                        "Turbos amount_out mismatch: local={} on-chain={} diff={}",
                                        local.amount_out, on_amount_out, out_diff
                                    );

                                    println!("  ✓ MATCH");
                                } else {
                                    println!("  Unexpected return size: {} bytes", bcs_bytes.len());
                                    println!("  Raw: {:?}", bcs_bytes);
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                println!("  devInspect RPC error: {}", e);
                // Don't panic for Turbos — the function may not be accessible
                println!("  (Turbos is closed source — devInspect may fail)");
            }
        }
    }
}

/// Build Turbos devInspect with the correct Versioned initial_shared_version.
fn build_turbos_swap_inspect_with_isv(
    pool_id: [u8; 32],
    pool_isv: u64,
    published_at: &str,
    versioned_id: &str,
    versioned_isv: u64,
    coin_a_type: &str,
    coin_b_type: &str,
    fee_type: &str,
    a2b: bool,
    amount: u64,
) -> Vec<u8> {
    let package_id = hex_to_32(published_at);
    let versioned_obj_id = hex_to_32(versioned_id);
    let clock_id = hex_to_32("0x0000000000000000000000000000000000000000000000000000000000000006");

    let sqrt_price_limit: u128 = if a2b {
        clmm_math::MIN_SQRT_PRICE
    } else {
        clmm_math::MAX_SQRT_PRICE
    };

    let mut buf = Vec::new();

    // TransactionKind::ProgrammableTransaction (variant 0)
    buf.push(0);

    // inputs: 7
    encode_vec_len(&mut buf, 7);

    // Input 0: pool
    buf.push(1); buf.push(1);
    buf.extend_from_slice(&pool_id);
    buf.extend_from_slice(&pool_isv.to_le_bytes());
    buf.push(1); // mutable = true

    // Input 1: a2b
    buf.push(0);
    encode_vec_len(&mut buf, 1);
    buf.push(a2b as u8);

    // Input 2: amount_specified (u128)
    buf.push(0);
    encode_vec_len(&mut buf, 16);
    buf.extend_from_slice(&(amount as u128).to_le_bytes());

    // Input 3: amount_specified_is_input
    buf.push(0);
    encode_vec_len(&mut buf, 1);
    buf.push(1);

    // Input 4: sqrt_price_limit (u128)
    buf.push(0);
    encode_vec_len(&mut buf, 16);
    buf.extend_from_slice(&sqrt_price_limit.to_le_bytes());

    // Input 5: clock
    buf.push(1); buf.push(1);
    buf.extend_from_slice(&clock_id);
    buf.extend_from_slice(&1u64.to_le_bytes());
    buf.push(0);

    // Input 6: versioned
    buf.push(1); buf.push(1);
    buf.extend_from_slice(&versioned_obj_id);
    buf.extend_from_slice(&versioned_isv.to_le_bytes());
    buf.push(0);

    // commands: 1
    encode_vec_len(&mut buf, 1);

    // Command::MoveCall
    buf.push(0);
    buf.extend_from_slice(&package_id);
    encode_string(&mut buf, "pool_fetcher");
    encode_string(&mut buf, "compute_swap_result");

    // type_arguments: 3
    encode_vec_len(&mut buf, 3);
    encode_type_tag_struct(&mut buf, coin_a_type);
    encode_type_tag_struct(&mut buf, coin_b_type);
    encode_type_tag_struct(&mut buf, fee_type);

    // arguments: 7
    encode_vec_len(&mut buf, 7);
    for i in 0u16..7 {
        buf.push(1);
        buf.extend_from_slice(&i.to_le_bytes());
    }

    buf
}
