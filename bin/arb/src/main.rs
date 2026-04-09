use std::sync::Arc;

use arb_types::config::AppConfig;
use dex_common::DexRegistry;
use tracing::info;

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

    // Fetch ticks for all pools
    info!("fetching tick data for all pools...");
    for registry in pool_manager.registries() {
        for pool_id in registry.pool_ids() {
            if let Some(pool) = registry.pool(&pool_id) {
                if let Err(e) = pool.fetch_price_data(&sui_client).await {
                    tracing::warn!(
                        pool = %arb_types::pool::object_id_to_hex(&pool_id),
                        error = %e,
                        "failed to fetch tick data, skipping"
                    );
                }
            }
        }
    }
    info!("tick data loaded");

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
    let mut by_length: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for rc in cycle_index.iter() {
        *by_length.entry(rc.cycle.len()).or_default() += 1;
    }
    for (len, count) in &by_length {
        info!(hops = len, count = count, "cycles by length");
    }

    // TODO Phase 4+: event polling loop
    info!("Phase 3 verification complete. Engine built successfully.");

    Ok(())
}
