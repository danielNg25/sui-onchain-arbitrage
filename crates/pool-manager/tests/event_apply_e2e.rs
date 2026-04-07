//! End-to-end test: fetch pool at version A, apply events A→B, compare with pool at version B.

use std::sync::Arc;

use arb_types::config::AppConfig;
use arb_types::pool::object_id_from_hex;
use dex_common::{parse_type_params, DexRegistry};
use sui_client::{EventFilter, ObjectDataOptions, SuiClient};

fn make_client() -> Arc<SuiClient> {
    Arc::new(SuiClient::new("https://fullnode.mainnet.sui.io:443"))
}

#[tokio::test]
#[ignore] // requires network
async fn e2e_cetus_apply_events() {
    let client = make_client();
    let config = AppConfig::load("../../config/mainnet.toml").unwrap();

    // Step 1: Find a Cetus pool with multiple recent swaps
    let events = client
        .query_events(
            EventFilter::MoveEventType(dex_cetus::CETUS_SWAP_EVENT_TYPE.to_string()),
            None,
            Some(200),
            true, // most recent first
        )
        .await
        .unwrap();

    // Count events per pool, find one with >= 3 events
    let mut pool_event_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for event in &events.data {
        if let Some(json) = &event.parsed_json {
            if let Some(pool_id) = json["pool"].as_str() {
                *pool_event_counts.entry(pool_id.to_string()).or_default() += 1;
            }
        }
    }

    let target_pool = pool_event_counts
        .iter()
        .filter(|(_, count)| **count >= 3)
        .max_by_key(|(_, count)| **count)
        .map(|(id, _)| id.clone());

    let Some(pool_id_str) = target_pool else {
        println!("No Cetus pool with >= 3 recent events found — skipping");
        return;
    };

    // Collect events for this pool in chronological order
    let mut pool_events: Vec<_> = events
        .data
        .iter()
        .filter(|e| {
            e.parsed_json
                .as_ref()
                .and_then(|j| j["pool"].as_str())
                .map(|p| p == pool_id_str)
                .unwrap_or(false)
        })
        .collect();
    pool_events.reverse();

    println!("Using pool: {}", pool_id_str);
    println!("Found {} recent events", pool_events.len());

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

    // Step 3: Get the version BEFORE the earliest event
    // The event's transaction changed the pool — we need the pool state before that tx.
    // Use the object version from before the first event.
    // We can get this from the event's transaction digest or by trying version_b - N.
    // Simpler: fetch pool at version = (current_version - pool_events.len() * 2)
    // as an approximation (each swap bumps version).
    // Better: use the first event's before_sqrt_price to validate.

    let first_event = pool_events[0];
    let first_json = first_event.parsed_json.as_ref().unwrap();
    let before_sqrt_price: u128 = first_json["before_sqrt_price"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // Find past version matching first event's before_sqrt_price.
    // Each tx that modifies the pool bumps version by 1 — search wider range.
    println!("Current version: {}, looking for sqrt_price={}", version_b, before_sqrt_price);
    let mut version_a = None;
    for offset in 1..500 {
        let try_version = version_b.saturating_sub(offset);
        if try_version == 0 {
            break;
        }

        let past = client
            .try_get_past_object(&pool_id_str, try_version, ObjectDataOptions::bcs())
            .await;

        match past {
            Ok(resp) if resp.data.is_some() => {
                let past_data = resp.data.unwrap();
                let past_bcs = past_data.bcs_bytes().unwrap();
                // Parse to check sqrt_price
                let past_parsed = dex_cetus::raw::parse_cetus_pool(&past_bcs);
                if let Ok(parsed) = past_parsed {
                    if parsed.current_sqrt_price == before_sqrt_price {
                        println!(
                            "Found matching past version {} (offset -{})",
                            try_version, offset
                        );
                        version_a = Some((try_version, past_bcs, past_data.initial_shared_version().unwrap_or(0)));
                        break;
                    }
                }
            }
            _ => continue,
        }
    }

    let Some((v_a, bcs_a, isv_a)) = version_a else {
        println!("Could not find past version matching first event's before_sqrt_price — skipping");
        return;
    };

    // Step 4: Ingest pool at version A
    let registry_a = dex_cetus::CetusRegistry::new(&config.cetus);
    registry_a
        .ingest_pool_object(object_id, &bcs_a, &type_params, v_a, isv_a)
        .unwrap();

    let pool_a = registry_a.pool(&object_id).unwrap();

    // Read state A
    let state_a_sqrt = get_cetus_sqrt_price(&registry_a, &object_id);
    println!("\n=== State A (version {}) ===", v_a);
    println!("  sqrt_price: {}", state_a_sqrt);
    assert_eq!(
        state_a_sqrt, before_sqrt_price,
        "state A sqrt_price should match first event's before_sqrt_price"
    );

    // Step 5: Apply events from A to B
    for (i, event) in pool_events.iter().enumerate() {
        let json = event.parsed_json.as_ref().unwrap();
        let result = pool_a.apply_event(dex_cetus::CETUS_SWAP_EVENT_TYPE, json).unwrap();
        assert!(result.is_some(), "event {} should be applied", i);
    }

    // Step 6: Compare applied state vs state B
    let applied_sqrt = get_cetus_sqrt_price(&registry_a, &object_id);
    let expected_sqrt = get_cetus_sqrt_price(&registry_b, &object_id);

    println!("\n=== State after applying {} events ===", pool_events.len());
    println!("  Applied sqrt_price:  {}", applied_sqrt);
    println!("  Expected sqrt_price: {}", expected_sqrt);

    // The applied state should match state B's sqrt_price exactly
    // (since the last event's after_sqrt_price IS the current on-chain sqrt_price)
    let last_event_json = pool_events.last().unwrap().parsed_json.as_ref().unwrap();
    let last_after_sqrt: u128 = last_event_json["after_sqrt_price"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    assert_eq!(
        applied_sqrt, last_after_sqrt,
        "applied sqrt_price should match last event's after_sqrt_price"
    );

    // If no events happened between our event query and the pool fetch,
    // applied should also match on-chain state B
    if applied_sqrt == expected_sqrt {
        println!("\n  EXACT MATCH with on-chain state!");
    } else {
        println!("\n  Note: on-chain state differs (events happened during test)");
        println!("  Applied vs on-chain diff: {}",
            (applied_sqrt as i128 - expected_sqrt as i128).abs());
    }

    println!("\n  E2E TEST PASSED");
}

/// Helper to read sqrt_price from a CetusRegistry's internal state.
fn get_cetus_sqrt_price(registry: &dex_cetus::CetusRegistry, pool_id: &[u8; 32]) -> u128 {
    // Use the raw parser to read BCS state... but we don't have access to internal state.
    // Instead, we'll use a public test helper. Let me add one to dex-cetus.
    dex_cetus::get_pool_sqrt_price(registry, pool_id).unwrap()
}
