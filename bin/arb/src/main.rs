use std::collections::HashMap;
use std::sync::Arc;

use arb_types::config::AppConfig;
use arb_types::pool::object_id_to_hex;
use dex_common::DexRegistry;
use pool_manager::collector::{CollectorConfig, SwapEventParser};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

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
        "will poll {} event types",
        all_event_types.len()
    );

    // Create pool manager
    let pool_manager = pool_manager::PoolManager::new(sui_client.clone(), vec![cetus, turbos]);

    // Load pools based on discovery mode
    let checkpoint = match config.strategy.pool_discovery_mode {
        arb_types::config::PoolDiscoveryMode::Preconfigured => {
            let preconfigured = config.strategy.preconfigured_pools.as_ref()
                .expect("preconfigured_pools required when pool_discovery_mode = preconfigured");
            info!("loading preconfigured pools...");
            // Registry order: [0]=cetus, [1]=turbos (matches vec![cetus, turbos] above)
            let pool_ids_per_registry = vec![
                preconfigured.cetus.clone(),
                preconfigured.turbos.clone(),
            ];
            pool_manager.load_pools_by_id(&pool_ids_per_registry).await?
        }
        arb_types::config::PoolDiscoveryMode::Auto => {
            let whitelisted: std::collections::HashSet<String> =
                config.strategy.whitelisted_tokens.iter().cloned().collect();
            info!("discovering pools (auto)...");
            pool_manager.discover_all_pools(&whitelisted).await?
        }
        arb_types::config::PoolDiscoveryMode::Both => {
            // Load preconfigured first, then discover remaining
            if let Some(preconfigured) = &config.strategy.preconfigured_pools {
                info!("loading preconfigured pools...");
                let pool_ids_per_registry = vec![
                    preconfigured.cetus.clone(),
                    preconfigured.turbos.clone(),
                ];
                pool_manager.load_pools_by_id(&pool_ids_per_registry).await?;
            }
            let whitelisted: std::collections::HashSet<String> =
                config.strategy.whitelisted_tokens.iter().cloned().collect();
            info!("discovering additional pools (auto)...");
            pool_manager.discover_all_pools(&whitelisted).await?
        }
    };
    info!(
        pools = pool_manager.pool_count(),
        checkpoint = checkpoint,
        "pool loading complete"
    );

    // Fetch ticks for all pools (one-time RPC at startup)
    info!("fetching tick data for all pools...");
    let mut tick_ok = 0u32;
    let mut tick_err = 0u32;
    for registry in pool_manager.registries() {
        for pool_id in registry.pool_ids() {
            if let Some(pool) = registry.pool(&pool_id) {
                match pool.fetch_price_data(&sui_client).await {
                    Ok(()) => tick_ok += 1,
                    Err(e) => {
                        debug!(pool = %object_id_to_hex(&pool_id), error = %e, "tick fetch failed");
                        tick_err += 1;
                    }
                }
            }
        }
    }
    info!(ok = tick_ok, errors = tick_err, "tick data loaded");

    let pool_manager = Arc::new(pool_manager);

    // Build arb engine
    info!("building arbitrage engine...");
    let engine = arb_engine::ArbEngine::build(
        pool_manager.clone(),
        profit_registry.clone(),
        &config.strategy,
    )?;
    info!(cycles = engine.cycle_count(), "arbitrage engine ready");

    // Log cycle breakdown
    let mut by_length: HashMap<usize, usize> = HashMap::new();
    for rc in engine.cycle_index().iter() {
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
    // Start collector service + consumer loop
    // -----------------------------------------------------------------------

    let (swap_tx, mut swap_rx) = mpsc::channel::<arb_types::event::SwapEventData>(1000);

    // Swap event parser — routes by event type to the correct DEX parser
    let swap_parser: SwapEventParser = Arc::new(|event_type, json| {
        if event_type == dex_cetus::CETUS_SWAP_EVENT_TYPE {
            dex_cetus::events::parse_swap_event_data(json).ok()
        } else if event_type == dex_turbos::TURBOS_SWAP_EVENT_TYPE {
            dex_turbos::events::parse_swap_event_data(json).ok()
        } else {
            None
        }
    });

    let collector_config = CollectorConfig {
        event_types: all_event_types,
        batch_size: 50,
        poll_interval_ms: config.strategy.poll_interval_ms,
    };

    let _collector_handle = pool_manager::collector::start_collector(
        sui_client.clone(),
        pool_manager.clone(),
        collector_config,
        swap_parser,
        swap_tx,
    );

    info!("collector started, waiting for swap events...");

    // Consumer loop — fully decoupled from collector
    while let Some(swap_event) = swap_rx.recv().await {
        let opps = engine.process_event(&swap_event).await;
        for opp in &opps {
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

    Ok(())
}
