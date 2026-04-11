use std::collections::HashMap;
use std::sync::Arc;

use arb_types::config::AppConfig;
use arb_types::event::SwapEventData;
use arb_types::pool::object_id_to_hex;
use dex_common::DexRegistry;
use sui_client::{EventCursor, EventFilter};
use tracing::{debug, info, warn};

/// Try to parse a swap event from raw event JSON.
/// Returns None for non-swap event types.
fn try_parse_swap_event(
    event_type: &str,
    parsed_json: &serde_json::Value,
) -> Option<SwapEventData> {
    if event_type == dex_cetus::CETUS_SWAP_EVENT_TYPE {
        dex_cetus::events::parse_swap_event_data(parsed_json).ok()
    } else if event_type == dex_turbos::TURBOS_SWAP_EVENT_TYPE {
        dex_turbos::events::parse_swap_event_data(parsed_json).ok()
    } else {
        None
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config/mainnet.toml".into());

    info!(config = %config_path, "loading configuration");
    let config = AppConfig::load(&config_path)?;

    // Build profit token registry
    let profit_registry = Arc::new(
        arb_engine::profit_token::ProfitTokenRegistry::from_config(&config.strategy.profit_tokens),
    );

    // Spawn background price updater
    let _price_handle =
        profit_registry.spawn_price_updater(config.strategy.price_update_interval_secs);
    info!("spawned price updater");

    // Create SUI client
    let sui_client = Arc::new(sui_client::SuiClient::new(&config.network.rpc_url));

    // Create DEX registries
    let cetus = Arc::new(dex_cetus::CetusRegistry::new(&config.cetus)) as Arc<dyn DexRegistry>;
    let turbos = Arc::new(dex_turbos::TurbosRegistry::new(&config.turbos)) as Arc<dyn DexRegistry>;

    // Collect all event types to poll
    let mut all_event_types: Vec<String> = Vec::new();
    for registry in [&cetus, &turbos] {
        for et in registry.event_types() {
            all_event_types.push(et.to_string());
        }
    }
    info!(
        event_types = all_event_types.len(),
        "polling {} event types",
        all_event_types.len()
    );

    // Create pool manager
    let whitelisted: std::collections::HashSet<String> =
        config.strategy.whitelisted_tokens.iter().cloned().collect();
    let pool_manager = pool_manager::PoolManager::new(sui_client.clone(), vec![cetus, turbos]);

    // Discover pools
    info!("discovering pools...");
    let checkpoint = pool_manager.discover_all_pools(&whitelisted).await?;
    info!(
        pools = pool_manager.pool_count(),
        checkpoint = checkpoint,
        "pool discovery complete"
    );

    // Fetch ticks for all pools (one-time RPC at startup)
    info!("fetching tick data for all pools...");
    let mut tick_fetch_ok = 0u32;
    let mut tick_fetch_err = 0u32;
    for registry in pool_manager.registries() {
        for pool_id in registry.pool_ids() {
            if let Some(pool) = registry.pool(&pool_id) {
                match pool.fetch_price_data(&sui_client).await {
                    Ok(()) => tick_fetch_ok += 1,
                    Err(e) => {
                        debug!(
                            pool = %object_id_to_hex(&pool_id),
                            error = %e,
                            "failed to fetch tick data"
                        );
                        tick_fetch_err += 1;
                    }
                }
            }
        }
    }
    info!(ok = tick_fetch_ok, errors = tick_fetch_err, "tick data loaded");

    let pool_manager = Arc::new(pool_manager);

    // Build arb engine
    info!("building arbitrage engine...");
    let engine = arb_engine::ArbEngine::build(
        pool_manager.clone(),
        profit_registry.clone(),
        &config.strategy,
    )?;
    info!(
        cycles = engine.cycle_count(),
        "arbitrage engine ready"
    );

    // Log cycle breakdown
    let cycle_index = engine.cycle_index();
    let mut by_length: HashMap<usize, usize> = HashMap::new();
    for rc in cycle_index.iter() {
        *by_length.entry(rc.cycle.len()).or_default() += 1;
    }
    for (len, count) in &by_length {
        info!(hops = len, count = count, "cycles by length");
    }

    if engine.cycle_count() == 0 {
        warn!("no arbitrage cycles found — nothing to do");
        return Ok(());
    }

    // -----------------------------------------------------------------------
    // Event polling loop
    // -----------------------------------------------------------------------
    info!(
        poll_interval_ms = config.strategy.poll_interval_ms,
        "starting event polling loop"
    );

    // Track cursor per event type for pagination
    let mut cursors: HashMap<String, Option<EventCursor>> = HashMap::new();
    for et in &all_event_types {
        cursors.insert(et.clone(), None);
    }

    let poll_interval = std::time::Duration::from_millis(config.strategy.poll_interval_ms);
    let mut total_events_applied = 0u64;
    let mut total_opportunities = 0u64;

    loop {
        let poll_start = std::time::Instant::now();
        let mut events_this_round = 0u32;
        let mut opps_this_round = 0u32;

        for event_type in &all_event_types {
            let cursor = cursors.get(event_type).cloned().flatten();

            let page = match sui_client
                .query_events(
                    EventFilter::MoveEventType(event_type.clone()),
                    cursor,
                    Some(50),
                    false, // ascending — oldest first for chronological processing
                )
                .await
            {
                Ok(page) => page,
                Err(e) => {
                    warn!(
                        event_type = %event_type,
                        error = %e,
                        "failed to query events"
                    );
                    continue;
                }
            };

            if !page.data.is_empty() {
                debug!(
                    event_type = %event_type,
                    count = page.data.len(),
                    "received events"
                );
            }

            for event in &page.data {
                let json = match &event.parsed_json {
                    Some(j) => j,
                    None => continue,
                };

                // 1. Apply to pool state (ALL events — swap + liquidity)
                //    Pure local computation, no RPC calls.
                match pool_manager.apply_event(&event.type_, json) {
                    Ok(Some(pool_id)) => {
                        events_this_round += 1;
                        debug!(
                            event_type = %event.type_,
                            pool = %object_id_to_hex(&pool_id),
                            "applied event to pool"
                        );
                    }
                    Ok(None) => {
                        // Event didn't match any pool we track
                    }
                    Err(e) => {
                        debug!(
                            event_type = %event.type_,
                            error = %e,
                            "failed to apply event"
                        );
                    }
                }

                // 2. If swap event, trigger arb-engine search
                if let Some(swap_data) = try_parse_swap_event(&event.type_, json) {
                    let opps = engine.process_event(&swap_data).await;
                    for opp in &opps {
                        opps_this_round += 1;
                        info!(
                            profit_token = %opp.profit_token,
                            profit = opp.profit,
                            profit_usd = format!("{:.4}", opp.profit_usd),
                            amount_in = opp.amount_in,
                            trigger_pool = %object_id_to_hex(&opp.trigger_pool_id),
                            hops = opp.cycle.cycle.len(),
                            "opportunity detected"
                        );
                    }
                }
            }

            // Update cursor for next poll
            if let Some(next_cursor) = page.next_cursor {
                cursors.insert(event_type.clone(), Some(next_cursor));
            } else if let Some(last) = page.data.last() {
                cursors.insert(event_type.clone(), Some(last.id.clone()));
            }
        }

        total_events_applied += events_this_round as u64;
        total_opportunities += opps_this_round as u64;

        if events_this_round > 0 {
            info!(
                events = events_this_round,
                opportunities = opps_this_round,
                total_events = total_events_applied,
                total_opportunities = total_opportunities,
                elapsed_ms = poll_start.elapsed().as_millis(),
                "poll cycle complete"
            );
        }

        tokio::time::sleep(poll_interval).await;
    }
}
