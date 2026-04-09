use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub network: NetworkConfig,
    pub cetus: CetusConfig,
    pub turbos: TurbosConfig,
    pub shio: ShioConfig,
    pub gas: GasConfig,
    pub strategy: StrategyConfig,
}

// --- Pool Discovery ---

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PoolDiscoveryMode {
    #[default]
    Auto,
    Preconfigured,
    Both,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PreconfiguredPools {
    #[serde(default)]
    pub cetus: Vec<String>,
    #[serde(default)]
    pub turbos: Vec<String>,
}

// --- Profit Tokens ---

#[derive(Debug, Clone, Deserialize)]
pub struct ProfitTokenConfig {
    pub token: String,
    pub symbol: String,
    pub decimals: u8,
    pub default_price_usd: f64,
    pub min_profit: u64,
    #[serde(default)]
    pub gecko_pool_address: Option<String>,
}

// --- Search Strategy ---

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SearchStrategy {
    Fast,
    #[default]
    Normal,
    Thorough,
}

#[derive(Debug, Deserialize)]
pub struct NetworkConfig {
    pub rpc_url: String,
}

#[derive(Debug, Deserialize)]
pub struct CetusConfig {
    pub package_types: String,
    pub package_published_at: String,
    pub global_config: String,
    pub pools_registry: String,
}

#[derive(Debug, Deserialize)]
pub struct TurbosConfig {
    pub package_types: String,
    pub package_published_at: String,
    pub swap_router_package: String,
    pub versioned: String,
    pub pool_table_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ShioConfig {
    pub feed_url: String,
    pub rpc_url: String,
    pub auctioneer_package: String,
    pub bid_percentage: u32,
}

#[derive(Debug, Deserialize)]
pub struct GasConfig {
    pub budget: u64,
    pub rgp_multiplier_normal: u64,
    pub rgp_multiplier_high: u64,
    pub pre_split_count: u32,
    pub pre_split_amount: u64,
}

#[derive(Debug, Deserialize)]
pub struct StrategyConfig {
    pub max_hops: u32,
    pub min_profit_mist: u64,
    pub binary_search_iterations: u32,
    pub poll_interval_ms: u64,
    pub whitelisted_tokens: Vec<String>,
    #[serde(default)]
    pub pool_discovery_mode: PoolDiscoveryMode,
    #[serde(default)]
    pub preconfigured_pools: Option<PreconfiguredPools>,
    #[serde(default)]
    pub profit_tokens: Vec<ProfitTokenConfig>,
    #[serde(default = "default_min_profit_usd")]
    pub min_profit_usd: f64,
    #[serde(default = "default_price_update_interval")]
    pub price_update_interval_secs: u64,
    #[serde(default = "default_event_timeout")]
    pub event_timeout_ms: u64,
    #[serde(default)]
    pub search_strategy: SearchStrategy,
}

fn default_min_profit_usd() -> f64 {
    0.10
}

fn default_price_update_interval() -> u64 {
    3600
}

fn default_event_timeout() -> u64 {
    10_000
}

impl AppConfig {
    pub fn load(path: &str) -> Result<Self, crate::error::ArbError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::error::ArbError::Config(format!("failed to read {}: {}", path, e)))?;
        toml::from_str(&content)
            .map_err(|e| crate::error::ArbError::Config(format!("failed to parse {}: {}", path, e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let toml_str = r#"
[network]
rpc_url = "https://fullnode.mainnet.sui.io:443"

[cetus]
package_types = "0xabc"
package_published_at = "0xdef"
global_config = "0x111"
pools_registry = "0x222"

[turbos]
package_types = "0x333"
package_published_at = "0x444"
swap_router_package = "0x555"
versioned = "0x666"
pool_table_id = "0x777"

[shio]
feed_url = "wss://example.com/feed"
rpc_url = "https://example.com"
auctioneer_package = "0x888"
bid_percentage = 90

[gas]
budget = 50000000
rgp_multiplier_normal = 5
rgp_multiplier_high = 100
pre_split_count = 10
pre_split_amount = 1000000000

[strategy]
max_hops = 3
min_profit_mist = 1000000
binary_search_iterations = 20
poll_interval_ms = 3000
whitelisted_tokens = ["0x2::sui::SUI"]
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.strategy.max_hops, 3);
        assert_eq!(config.gas.budget, 50_000_000);
        assert_eq!(config.cetus.package_types, "0xabc");
    }
}
