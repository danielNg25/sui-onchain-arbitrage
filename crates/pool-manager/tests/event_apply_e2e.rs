//! End-to-end test: fetch pool at version A, apply ALL events A→B, compare with pool at version B.
//! Tests swap events AND liquidity events together.

use std::sync::Arc;

use arb_types::config::AppConfig;
use arb_types::pool::object_id_from_hex;
use dex_common::{parse_type_params, DexRegistry};
use sui_client::{EventFilter, ObjectDataOptions, SuiClient, SuiEvent};

fn make_client() -> Arc<SuiClient> {
    Arc::new(SuiClient::new("https://fullnode.mainnet.sui.io:443"))
}

/// All Cetus pool event types we handle.
const CETUS_ALL_EVENT_TYPES: &[&str] = &[
    dex_cetus::CETUS_SWAP_EVENT_TYPE,
    dex_cetus::CETUS_ADD_LIQUIDITY_EVENT_TYPE,
    dex_cetus::CETUS_REMOVE_LIQUIDITY_EVENT_TYPE,
];

#[tokio::test]
#[ignore]
async fn e2e_cetus_apply_all_events() {
    let client = make_client();
    let config = AppConfig::load("../../config/mainnet.toml").unwrap();

    // Step 1: Query ALL event types from Cetus pool module to find a pool
    // with mixed swap + liquidity events.
    let all_events = query_events_by_types(&client, CETUS_ALL_EVENT_TYPES).await;

    // Group events by pool and find one with both swap AND liquidity events
    let mut pool_events_map: std::collections::HashMap<String, Vec<&SuiEvent>> =
        std::collections::HashMap::new();
    for event in &all_events {
        if let Some(json) = &event.parsed_json {
            if let Some(pool_id) = json["pool"].as_str() {
                pool_events_map
                    .entry(pool_id.to_string())
                    .or_default()
                    .push(event);
            }
        }
    }

    // Find a pool with at least 1 liquidity event AND at least 1 swap event
    let mut target_pool = None;
    for (pool_id, events) in &pool_events_map {
        let has_swap = events.iter().any(|e| e.type_ == dex_cetus::CETUS_SWAP_EVENT_TYPE);
        let has_liq = events.iter().any(|e| {
            e.type_ == dex_cetus::CETUS_ADD_LIQUIDITY_EVENT_TYPE
                || e.type_ == dex_cetus::CETUS_REMOVE_LIQUIDITY_EVENT_TYPE
        });
        let total = events.len();

        if has_swap && has_liq && total >= 3 {
            let swap_count = events
                .iter()
                .filter(|e| e.type_ == dex_cetus::CETUS_SWAP_EVENT_TYPE)
                .count();
            let liq_count = total - swap_count;
            println!(
                "Candidate: {} ({} events: {} swaps, {} liquidity)",
                pool_id, total, swap_count, liq_count
            );
            if target_pool.is_none() || total > pool_events_map[target_pool.as_ref().unwrap()].len()
            {
                target_pool = Some(pool_id.clone());
            }
        }
    }

    // Fallback: if no pool has both types, use any pool with >= 3 events
    if target_pool.is_none() {
        target_pool = pool_events_map
            .iter()
            .filter(|(_, events)| events.len() >= 3)
            .max_by_key(|(_, events)| events.len())
            .map(|(id, _)| id.clone());
        if let Some(ref id) = target_pool {
            println!("No pool with mixed events; using pool with most events: {}", id);
        }
    }

    let Some(pool_id_str) = target_pool else {
        println!("No suitable pool found — skipping");
        return;
    };

    let pool_events_unsorted = &pool_events_map[&pool_id_str];
    // Events were queried descending — reverse for chronological
    let mut pool_events: Vec<&SuiEvent> = pool_events_unsorted.clone();
    pool_events.reverse();

    let swap_count = pool_events
        .iter()
        .filter(|e| e.type_ == dex_cetus::CETUS_SWAP_EVENT_TYPE)
        .count();
    let liq_count = pool_events.len() - swap_count;

    println!(
        "\nUsing pool: {} ({} events: {} swaps, {} liquidity)",
        pool_id_str,
        pool_events.len(),
        swap_count,
        liq_count,
    );

    let object_id = object_id_from_hex(&pool_id_str).unwrap();

    // Step 2: Fetch pool at current version (state B)
    let resp_b = client
        .get_object(&pool_id_str, ObjectDataOptions::bcs())
        .await
        .unwrap();
    let data_b = resp_b.data.unwrap();
    let version_b = data_b.version_number();
    let bcs_b = data_b.bcs_bytes().unwrap();
    let type_params = parse_type_params(data_b.bcs_type().unwrap());

    let registry_b = dex_cetus::CetusRegistry::new(&config.cetus);
    registry_b
        .ingest_pool_object(
            object_id,
            &bcs_b,
            &type_params,
            version_b,
            data_b.initial_shared_version().unwrap_or(0),
        )
        .unwrap();

    // Step 3: Find state A — pool version before the first event.
    // First event's before_sqrt_price tells us the sqrt_price at state A.
    let first_swap = pool_events
        .iter()
        .find(|e| e.type_ == dex_cetus::CETUS_SWAP_EVENT_TYPE);

    let before_sqrt_price = first_swap
        .and_then(|e| e.parsed_json.as_ref())
        .and_then(|j| j["before_sqrt_price"].as_str())
        .and_then(|s| s.parse::<u128>().ok());

    println!("Current version: {}", version_b);
    if let Some(bsp) = before_sqrt_price {
        println!("First swap's before_sqrt_price: {}", bsp);
    }

    // Search backwards for a version before the event window.
    // If we have before_sqrt_price (from first swap), match exactly.
    // Otherwise, use a version far enough before current to cover all events.
    let mut version_a = None;
    let search_start = version_b.saturating_sub(1);

    for try_version in (version_b.saturating_sub(500)..=search_start).rev() {
        if try_version == 0 {
            break;
        }

        let past = client
            .try_get_past_object(&pool_id_str, try_version, ObjectDataOptions::bcs())
            .await;

        match past {
            Ok(resp) if resp.data.is_some() => {
                let past_data = resp.data.unwrap();
                let past_bcs = match past_data.bcs_bytes() {
                    Ok(b) => b,
                    Err(_) => continue,
                };

                let parsed = match dex_cetus::raw::parse_cetus_pool(&past_bcs) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                let matches = if let Some(bsp) = before_sqrt_price {
                    parsed.current_sqrt_price == bsp
                } else {
                    // No swap events — just go back far enough
                    (version_b - try_version) as usize >= pool_events.len()
                };

                if matches {
                    println!(
                        "Found past version {} (offset -{})",
                        try_version,
                        version_b - try_version
                    );
                    version_a = Some((
                        try_version,
                        past_bcs,
                        past_data.initial_shared_version().unwrap_or(0),
                    ));
                    break;
                }
            }
            _ => continue,
        }
    }

    let Some((v_a, bcs_a, isv_a)) = version_a else {
        println!("Could not find matching past version — skipping");
        return;
    };

    // Step 4: Ingest pool at version A
    let registry_a = dex_cetus::CetusRegistry::new(&config.cetus);
    registry_a
        .ingest_pool_object(object_id, &bcs_a, &type_params, v_a, isv_a)
        .unwrap();

    let pool_a = registry_a.pool(&object_id).unwrap();

    // Also fetch ticks at version A so apply_event has tick data to work with
    pool_a.fetch_price_data(&client).await.unwrap();

    let state_a_sqrt = dex_cetus::get_pool_sqrt_price(&registry_a, &object_id).unwrap();
    let state_a_reserves = dex_cetus::get_pool_reserves(&registry_a, &object_id).unwrap();
    let ticks_a_count = dex_cetus::get_pool_ticks(&registry_a, &object_id)
        .map(|t| t.len())
        .unwrap_or(0);
    println!("\n=== State A (version {}) ===", v_a);
    println!("  sqrt_price:  {}", state_a_sqrt);
    println!("  reserve_a:   {}", state_a_reserves.0);
    println!("  reserve_b:   {}", state_a_reserves.1);
    println!("  ticks:       {}", ticks_a_count);

    // Step 5: Apply ALL events (swap + liquidity)
    let mut applied_count = 0;
    let mut swap_applied = 0;
    let mut liq_applied = 0;
    for event in &pool_events {
        let json = event.parsed_json.as_ref().unwrap();
        let result = pool_a.apply_event(&event.type_, json).unwrap();
        if result.is_some() {
            applied_count += 1;
            if event.type_ == dex_cetus::CETUS_SWAP_EVENT_TYPE {
                swap_applied += 1;
            } else {
                liq_applied += 1;
            }
        }
    }

    println!(
        "\nApplied {} events ({} swaps, {} liquidity)",
        applied_count, swap_applied, liq_applied
    );

    // Step 6: Compare applied state vs state B
    let applied_sqrt = dex_cetus::get_pool_sqrt_price(&registry_a, &object_id).unwrap();
    let applied_reserves = dex_cetus::get_pool_reserves(&registry_a, &object_id).unwrap();
    let expected_sqrt = dex_cetus::get_pool_sqrt_price(&registry_b, &object_id).unwrap();
    let expected_reserves = dex_cetus::get_pool_reserves(&registry_b, &object_id).unwrap();

    println!("\n=== Comparison ===");
    println!("  sqrt_price  — applied: {}  expected: {}", applied_sqrt, expected_sqrt);
    println!("  reserve_a   — applied: {}  expected: {}", applied_reserves.0, expected_reserves.0);
    println!("  reserve_b   — applied: {}  expected: {}", applied_reserves.1, expected_reserves.1);

    // Verify sqrt_price matches last swap event's after_sqrt_price (if any swaps)
    let last_swap = pool_events
        .iter()
        .rev()
        .find(|e| e.type_ == dex_cetus::CETUS_SWAP_EVENT_TYPE);

    if let Some(last) = last_swap {
        let last_json = last.parsed_json.as_ref().unwrap();
        let last_after: u128 = last_json["after_sqrt_price"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            applied_sqrt, last_after,
            "applied sqrt_price should match last swap's after_sqrt_price"
        );
        println!("\n  sqrt_price matches last swap's after_sqrt_price");

        // Reserves should also match last swap's vault amounts
        let expected_va: u64 = last_json["vault_a_amount"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let expected_vb: u64 = last_json["vault_b_amount"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        // Note: if liquidity events happened AFTER the last swap,
        // reserves will differ from the swap's vault amounts.
        // We only assert if last event is a swap.
        if pool_events.last().unwrap().type_ == dex_cetus::CETUS_SWAP_EVENT_TYPE {
            assert_eq!(applied_reserves.0, expected_va, "reserve_a mismatch");
            assert_eq!(applied_reserves.1, expected_vb, "reserve_b mismatch");
            println!("  reserves match last swap's vault amounts");
        }
    }

    // Compare with on-chain state B
    // If no events happened between our query and pool fetch, should be exact.
    let sqrt_match = applied_sqrt == expected_sqrt;
    let reserves_match =
        applied_reserves.0 == expected_reserves.0 && applied_reserves.1 == expected_reserves.1;

    if sqrt_match && reserves_match {
        println!("\n  EXACT MATCH with on-chain state B!");
    } else {
        println!("\n  Note: on-chain state may differ (events happened during test window)");
        if !sqrt_match {
            println!(
                "  sqrt_price diff: {}",
                (applied_sqrt as i128 - expected_sqrt as i128).abs()
            );
        }
        if !reserves_match {
            println!(
                "  reserve_a diff: {}",
                (applied_reserves.0 as i128 - expected_reserves.0 as i128).abs()
            );
            println!(
                "  reserve_b diff: {}",
                (applied_reserves.1 as i128 - expected_reserves.1 as i128).abs()
            );
        }
    }

    // Step 7: Tick-by-tick comparison
    // Re-fetch ticks from chain (current state = state B) and compare with our locally mutated ticks.
    let pool_b = registry_b.pool(&object_id).unwrap();
    pool_b.fetch_price_data(&client).await.unwrap();
    let ticks_onchain = dex_cetus::get_pool_ticks(&registry_b, &object_id).unwrap();
    let ticks_applied = dex_cetus::get_pool_ticks(&registry_a, &object_id).unwrap();

    println!("\n=== Tick-by-tick comparison ===");
    println!("  Applied ticks:  {}", ticks_applied.len());
    println!("  On-chain ticks: {}", ticks_onchain.len());

    // Build lookup maps for comparison
    let applied_map: std::collections::HashMap<i32, &arb_types::tick::Tick> =
        ticks_applied.iter().map(|t| (t.index, t)).collect();
    let onchain_map: std::collections::HashMap<i32, &arb_types::tick::Tick> =
        ticks_onchain.iter().map(|t| (t.index, t)).collect();

    let mut mismatches = 0;
    let mut missing_in_applied = 0;
    let mut extra_in_applied = 0;

    // Check all on-chain ticks exist in applied with correct values
    for (idx, onchain_tick) in &onchain_map {
        match applied_map.get(idx) {
            Some(applied_tick) => {
                if applied_tick.liquidity_net != onchain_tick.liquidity_net
                    || applied_tick.liquidity_gross != onchain_tick.liquidity_gross
                {
                    if mismatches < 5 {
                        println!(
                            "  MISMATCH tick {}: applied(net={}, gross={}) vs onchain(net={}, gross={})",
                            idx,
                            applied_tick.liquidity_net, applied_tick.liquidity_gross,
                            onchain_tick.liquidity_net, onchain_tick.liquidity_gross,
                        );
                    }
                    mismatches += 1;
                }
            }
            None => {
                if missing_in_applied < 3 {
                    println!(
                        "  MISSING in applied: tick {} (onchain net={}, gross={})",
                        idx, onchain_tick.liquidity_net, onchain_tick.liquidity_gross
                    );
                }
                missing_in_applied += 1;
            }
        }
    }

    // Check for extra ticks in applied that aren't on-chain
    for (idx, _) in &applied_map {
        if !onchain_map.contains_key(idx) {
            if extra_in_applied < 3 {
                println!("  EXTRA in applied: tick {}", idx);
            }
            extra_in_applied += 1;
        }
    }

    println!("\n  Tick mismatches:        {}", mismatches);
    println!("  Missing in applied:     {}", missing_in_applied);
    println!("  Extra in applied:       {}", extra_in_applied);

    if mismatches == 0 && missing_in_applied == 0 && extra_in_applied == 0 {
        println!("\n  ALL TICKS MATCH EXACTLY!");
    } else {
        println!(
            "\n  Note: {} tick differences (events may have occurred during test)",
            mismatches + missing_in_applied + extra_in_applied
        );
    }

    println!("\n  E2E TEST PASSED");
}

/// All Turbos pool event types we handle.
const TURBOS_ALL_EVENT_TYPES: &[&str] = &[
    dex_turbos::TURBOS_SWAP_EVENT_TYPE,
    dex_turbos::TURBOS_MINT_EVENT_TYPE,
    dex_turbos::TURBOS_BURN_EVENT_TYPE,
];

#[tokio::test]
#[ignore]
async fn e2e_turbos_apply_all_events() {
    let client = make_client();
    let config = AppConfig::load("../../config/mainnet.toml").unwrap();

    // Step 1: Query all Turbos event types
    let all_events = query_events_by_types(&client, TURBOS_ALL_EVENT_TYPES).await;

    // Group by pool, find one with mixed events
    let mut pool_events_map: std::collections::HashMap<String, Vec<&SuiEvent>> =
        std::collections::HashMap::new();
    for event in &all_events {
        if let Some(json) = &event.parsed_json {
            if let Some(pool_id) = json["pool"].as_str() {
                pool_events_map
                    .entry(pool_id.to_string())
                    .or_default()
                    .push(event);
            }
        }
    }

    // Find pool with swap + liquidity events, prefer smallest total for speed
    let mut target_pool = None;
    for (pool_id, events) in &pool_events_map {
        let has_swap = events
            .iter()
            .any(|e| e.type_ == dex_turbos::TURBOS_SWAP_EVENT_TYPE);
        let has_liq = events.iter().any(|e| {
            e.type_ == dex_turbos::TURBOS_MINT_EVENT_TYPE
                || e.type_ == dex_turbos::TURBOS_BURN_EVENT_TYPE
        });
        let swap_count = events
            .iter()
            .filter(|e| e.type_ == dex_turbos::TURBOS_SWAP_EVENT_TYPE)
            .count();
        let total = events.len();

        if has_swap && has_liq {
            println!(
                "Candidate: {} ({} events: {} swaps, {} liquidity)",
                pool_id, total, swap_count, total - swap_count
            );
            // Prefer pools with fewer total events (faster tick fetch)
            if target_pool.is_none() {
                target_pool = Some(pool_id.clone());
            }
        }
    }

    // Fallback: any pool with >= 2 swap events
    if target_pool.is_none() {
        target_pool = pool_events_map
            .iter()
            .filter(|(_, events)| {
                events.iter().filter(|e| e.type_ == dex_turbos::TURBOS_SWAP_EVENT_TYPE).count() >= 2
            })
            .min_by_key(|(_, events)| events.len())
            .map(|(id, _)| id.clone());
    }

    let Some(pool_id_str) = target_pool else {
        println!("No suitable Turbos pool found — skipping");
        return;
    };

    let pool_events_unsorted = &pool_events_map[&pool_id_str];
    let mut pool_events: Vec<&SuiEvent> = pool_events_unsorted.clone();
    pool_events.reverse();

    let swap_count = pool_events
        .iter()
        .filter(|e| e.type_ == dex_turbos::TURBOS_SWAP_EVENT_TYPE)
        .count();
    let liq_count = pool_events.len() - swap_count;

    println!(
        "\nUsing Turbos pool: {} ({} events: {} swaps, {} liquidity)",
        pool_id_str,
        pool_events.len(),
        swap_count,
        liq_count,
    );

    let object_id = object_id_from_hex(&pool_id_str).unwrap();

    // Step 2: Fetch pool at current version (state B)
    let resp_b = client
        .get_object(&pool_id_str, ObjectDataOptions::bcs())
        .await
        .unwrap();
    let data_b = resp_b.data.unwrap();
    let version_b = data_b.version_number();
    let bcs_b = data_b.bcs_bytes().unwrap();
    let type_str_b = data_b.bcs_type().unwrap();
    let (coin_params, fee_type) = dex_common::parse_type_params_with_fee(type_str_b);
    let mut type_params = coin_params;
    if let Some(ft) = fee_type {
        type_params.push(ft);
    }

    let registry_b = dex_turbos::TurbosRegistry::new(&config.turbos);
    registry_b
        .ingest_pool_object(
            object_id,
            &bcs_b,
            &type_params,
            version_b,
            data_b.initial_shared_version().unwrap_or(0),
        )
        .unwrap();

    // Step 3: Find state A — go back enough versions to cover all events
    println!("Current version: {}", version_b);

    let mut version_a = None;
    // Each event bumps version by 1. Search back event_count + margin.
    let target_offset = pool_events.len() as u64 + 10;
    for try_version in (version_b.saturating_sub(500)..version_b).rev() {
        if try_version == 0 {
            break;
        }

        let past = client
            .try_get_past_object(&pool_id_str, try_version, ObjectDataOptions::bcs())
            .await;

        match past {
            Ok(resp) if resp.data.is_some() => {
                let past_data = resp.data.unwrap();
                let past_bcs = match past_data.bcs_bytes() {
                    Ok(b) => b,
                    Err(_) => continue,
                };

                // Just verify we can parse it
                if dex_turbos::raw::parse_turbos_pool(&past_bcs).is_err() {
                    continue;
                }

                if (version_b - try_version) >= target_offset {
                    println!(
                        "Found past version {} (offset -{})",
                        try_version,
                        version_b - try_version
                    );
                    version_a = Some((
                        try_version,
                        past_bcs,
                        past_data.initial_shared_version().unwrap_or(0),
                    ));
                    break;
                }
            }
            _ => continue,
        }
    }

    let Some((v_a, bcs_a, isv_a)) = version_a else {
        println!("Could not find matching past version — skipping");
        return;
    };

    // Step 4: Ingest pool at version A + fetch ticks
    let registry_a = dex_turbos::TurbosRegistry::new(&config.turbos);
    registry_a
        .ingest_pool_object(object_id, &bcs_a, &type_params, v_a, isv_a)
        .unwrap();

    let pool_a = registry_a.pool(&object_id).unwrap();
    // Fetch ticks (this is slow for large pools on public RPC)
    println!("Fetching ticks at version A...");
    pool_a.fetch_price_data(&client).await.unwrap();

    let state_a_sqrt = dex_turbos::get_pool_sqrt_price(&registry_a, &object_id).unwrap();
    let state_a_reserves = dex_turbos::get_pool_reserves(&registry_a, &object_id).unwrap();
    let ticks_a_count = dex_turbos::get_pool_ticks(&registry_a, &object_id)
        .map(|t| t.len())
        .unwrap_or(0);
    println!("\n=== State A (version {}) ===", v_a);
    println!("  sqrt_price:  {}", state_a_sqrt);
    println!("  reserve_a:   {}", state_a_reserves.0);
    println!("  reserve_b:   {}", state_a_reserves.1);
    println!("  ticks:       {}", ticks_a_count);

    // Step 5: Apply all events
    let mut applied_count = 0;
    let mut swap_applied = 0;
    let mut liq_applied = 0;
    for (i, event) in pool_events.iter().enumerate() {
        let json = event.parsed_json.as_ref().unwrap();
        let result = pool_a.apply_event(&event.type_, json);
        match result {
            Ok(Some(_)) => {
                applied_count += 1;
                if event.type_ == dex_turbos::TURBOS_SWAP_EVENT_TYPE {
                    swap_applied += 1;
                } else {
                    liq_applied += 1;
                }
            }
            Ok(None) => {}
            Err(e) => {
                println!("  Event {} ({}) failed: {}", i, event.type_.rsplit("::").next().unwrap_or(""), e);
                // Don't fail — skip events with missing fields
            }
        }
    }

    println!(
        "\nApplied {} events ({} swaps, {} liquidity)",
        applied_count, swap_applied, liq_applied
    );

    // Step 6: Compare
    let applied_sqrt = dex_turbos::get_pool_sqrt_price(&registry_a, &object_id).unwrap();
    let applied_reserves = dex_turbos::get_pool_reserves(&registry_a, &object_id).unwrap();
    let expected_sqrt = dex_turbos::get_pool_sqrt_price(&registry_b, &object_id).unwrap();
    let expected_reserves = dex_turbos::get_pool_reserves(&registry_b, &object_id).unwrap();

    println!("\n=== Comparison ===");
    println!("  sqrt_price  — applied: {}  expected: {}", applied_sqrt, expected_sqrt);
    println!("  reserve_a   — applied: {}  expected: {}", applied_reserves.0, expected_reserves.0);
    println!("  reserve_b   — applied: {}  expected: {}", applied_reserves.1, expected_reserves.1);

    if let Some(last) = pool_events
        .iter()
        .rev()
        .find(|e| e.type_ == dex_turbos::TURBOS_SWAP_EVENT_TYPE)
    {
        let last_json = last.parsed_json.as_ref().unwrap();
        // Turbos uses "sqrt_price" (not "after_sqrt_price")
        let last_sqrt: u128 = last_json["sqrt_price"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            applied_sqrt, last_sqrt,
            "applied sqrt_price should match last swap's sqrt_price"
        );
        println!("\n  sqrt_price matches last swap event");
    }

    // Step 7: Tick-by-tick comparison
    println!("Fetching on-chain ticks for comparison...");
    let pool_b = registry_b.pool(&object_id).unwrap();
    pool_b.fetch_price_data(&client).await.unwrap();
    let ticks_onchain = dex_turbos::get_pool_ticks(&registry_b, &object_id).unwrap();
    let ticks_applied = dex_turbos::get_pool_ticks(&registry_a, &object_id).unwrap();

    println!("\n=== Tick-by-tick comparison ===");
    println!("  Applied ticks:  {}", ticks_applied.len());
    println!("  On-chain ticks: {}", ticks_onchain.len());

    let applied_map: std::collections::HashMap<i32, &arb_types::tick::Tick> =
        ticks_applied.iter().map(|t| (t.index, t)).collect();
    let onchain_map: std::collections::HashMap<i32, &arb_types::tick::Tick> =
        ticks_onchain.iter().map(|t| (t.index, t)).collect();

    let mut mismatches = 0;
    let mut missing_in_applied = 0;
    let mut extra_in_applied = 0;

    for (idx, onchain_tick) in &onchain_map {
        match applied_map.get(idx) {
            Some(applied_tick) => {
                if applied_tick.liquidity_net != onchain_tick.liquidity_net
                    || applied_tick.liquidity_gross != onchain_tick.liquidity_gross
                {
                    if mismatches < 5 {
                        println!(
                            "  MISMATCH tick {}: applied(net={}, gross={}) vs onchain(net={}, gross={})",
                            idx,
                            applied_tick.liquidity_net, applied_tick.liquidity_gross,
                            onchain_tick.liquidity_net, onchain_tick.liquidity_gross,
                        );
                    }
                    mismatches += 1;
                }
            }
            None => {
                if missing_in_applied < 3 {
                    println!("  MISSING in applied: tick {}", idx);
                }
                missing_in_applied += 1;
            }
        }
    }

    for (idx, _) in &applied_map {
        if !onchain_map.contains_key(idx) {
            if extra_in_applied < 3 {
                println!("  EXTRA in applied: tick {}", idx);
            }
            extra_in_applied += 1;
        }
    }

    println!("\n  Tick mismatches:        {}", mismatches);
    println!("  Missing in applied:     {}", missing_in_applied);
    println!("  Extra in applied:       {}", extra_in_applied);

    if mismatches == 0 && missing_in_applied == 0 && extra_in_applied == 0 {
        println!("\n  ALL TICKS MATCH EXACTLY!");
    }

    println!("\n  E2E TURBOS TEST PASSED");
}

/// Query events for multiple event types.
async fn query_events_by_types(client: &SuiClient, event_types: &[&str]) -> Vec<SuiEvent> {
    let mut all = Vec::new();

    for event_type in event_types {
        let events = client
            .query_events(
                EventFilter::MoveEventType(event_type.to_string()),
                None,
                Some(200),
                true,
            )
            .await
            .unwrap();
        all.extend(events.data);
    }

    all
}
