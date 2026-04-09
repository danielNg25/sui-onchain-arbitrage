use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arb_types::config::ProfitTokenConfig;
use arb_types::pool::CoinType;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::error::EngineError;

/// A profit token with price tracking.
#[derive(Debug, Clone)]
pub struct ProfitToken {
    pub token: CoinType,
    pub symbol: String,
    pub decimals: u8,
    pub default_price_usd: f64,
    pub min_profit_base_units: u64,
    pub gecko_pool_address: Option<String>,
    /// Current USD price (updated by background task).
    pub price_usd: f64,
}

impl ProfitToken {
    /// Convert a raw token amount to USD value.
    pub fn to_usd(&self, amount: u64) -> f64 {
        (amount as f64 / 10_f64.powi(self.decimals as i32)) * self.price_usd
    }

    /// Convert a USD value to token base units.
    pub fn from_usd(&self, usd: f64) -> u64 {
        ((usd / self.price_usd) * 10_f64.powi(self.decimals as i32)) as u64
    }

    /// Compute min profit in base units from USD threshold.
    /// Returns whichever is larger: the configured minimum or the USD-converted amount.
    pub fn min_profit_for_usd(&self, min_usd: f64) -> u64 {
        self.from_usd(min_usd).max(self.min_profit_base_units)
    }
}

/// Registry of profit tokens with price tracking.
pub struct ProfitTokenRegistry {
    /// Ordered list -- index 0 is highest priority profit token.
    tokens: Arc<RwLock<Vec<ProfitToken>>>,
    /// token CoinType -> index in `tokens` for O(1) lookup.
    index: HashMap<CoinType, usize>,
}

impl ProfitTokenRegistry {
    /// Build from config.
    pub fn from_config(configs: &[ProfitTokenConfig]) -> Self {
        let mut tokens = Vec::with_capacity(configs.len());
        let mut index = HashMap::new();

        for (i, cfg) in configs.iter().enumerate() {
            let token: CoinType = Arc::from(cfg.token.as_str());
            index.insert(token.clone(), i);
            tokens.push(ProfitToken {
                token,
                symbol: cfg.symbol.clone(),
                decimals: cfg.decimals,
                default_price_usd: cfg.default_price_usd,
                min_profit_base_units: cfg.min_profit,
                gecko_pool_address: cfg.gecko_pool_address.clone(),
                price_usd: cfg.default_price_usd,
            });
        }

        Self {
            tokens: Arc::new(RwLock::new(tokens)),
            index,
        }
    }

    /// Check if a token is a profit token. Returns its priority index if so.
    pub fn lookup(&self, token: &CoinType) -> Option<usize> {
        self.index.get(token).copied()
    }

    /// Get the list of all profit token CoinTypes (in priority order).
    pub fn profit_token_types(&self) -> Vec<CoinType> {
        self.index
            .iter()
            .collect::<Vec<_>>()
            .into_iter()
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .collect()
    }

    /// Get ordered profit token types (by priority index).
    pub fn ordered_profit_tokens(&self) -> Vec<CoinType> {
        let mut entries: Vec<_> = self.index.iter().collect();
        entries.sort_by_key(|(_, idx)| **idx);
        entries.into_iter().map(|(k, _)| k.clone()).collect()
    }

    /// Get the best (highest priority) profit token present in a set of tokens.
    pub fn best_profit_token(&self, tokens: &[CoinType]) -> Option<(CoinType, usize)> {
        let mut best: Option<(CoinType, usize)> = None;
        for t in tokens {
            if let Some(&idx) = self.index.get(t) {
                match &best {
                    Some((_, best_idx)) if idx >= *best_idx => {}
                    _ => best = Some((t.clone(), idx)),
                }
            }
        }
        best
    }

    /// Get a snapshot of a profit token by index.
    pub async fn get(&self, idx: usize) -> Option<ProfitToken> {
        let tokens = self.tokens.read().await;
        tokens.get(idx).cloned()
    }

    /// Get USD value for a token amount.
    pub async fn get_usd_value(&self, token: &CoinType, amount: u64) -> Option<f64> {
        let idx = self.index.get(token)?;
        let tokens = self.tokens.read().await;
        let pt = tokens.get(*idx)?;
        Some(pt.to_usd(amount))
    }

    /// Update prices from GeckoTerminal.
    pub async fn update_prices(&self) -> Result<(), EngineError> {
        let tokens = self.tokens.read().await;
        let mut updates: Vec<(usize, String)> = Vec::new();

        for (i, pt) in tokens.iter().enumerate() {
            if let Some(ref addr) = pt.gecko_pool_address {
                updates.push((i, addr.clone()));
            }
        }
        drop(tokens);

        if updates.is_empty() {
            return Ok(());
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| EngineError::PriceFetch(e.to_string()))?;

        for (idx, pool_address) in &updates {
            match fetch_gecko_price(&client, pool_address).await {
                Ok(price) => {
                    let mut tokens = self.tokens.write().await;
                    if let Some(pt) = tokens.get_mut(*idx) {
                        info!(
                            symbol = %pt.symbol,
                            old_price = pt.price_usd,
                            new_price = price,
                            "updated profit token price"
                        );
                        pt.price_usd = price;
                    }
                }
                Err(e) => {
                    warn!(
                        pool_address = %pool_address,
                        error = %e,
                        "failed to fetch price, keeping previous value"
                    );
                }
            }
        }

        Ok(())
    }

    /// Spawn a background task that updates prices on an interval.
    pub fn spawn_price_updater(
        self: &Arc<Self>,
        interval_secs: u64,
    ) -> tokio::task::JoinHandle<()> {
        let registry = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;
                if let Err(e) = registry.update_prices().await {
                    warn!(error = %e, "price update cycle failed");
                }
            }
        })
    }
}

/// Fetch token price from GeckoTerminal for a Sui pool.
async fn fetch_gecko_price(
    client: &reqwest::Client,
    pool_address: &str,
) -> Result<f64, EngineError> {
    let url = format!(
        "https://api.geckoterminal.com/api/v2/networks/sui/pools/{}",
        pool_address
    );

    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| EngineError::PriceFetch(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(EngineError::PriceFetch(format!(
            "HTTP {}",
            resp.status()
        )));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| EngineError::PriceFetch(e.to_string()))?;

    // Extract base_token_price_usd from response
    let price_str = body
        .pointer("/data/attributes/base_token_price_usd")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            EngineError::PriceFetch("missing base_token_price_usd in response".into())
        })?;

    price_str
        .parse::<f64>()
        .map_err(|e| EngineError::PriceFetch(format!("invalid price '{}': {}", price_str, e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_registry() -> ProfitTokenRegistry {
        ProfitTokenRegistry::from_config(&[
            ProfitTokenConfig {
                token: "0x2::sui::SUI".into(),
                symbol: "SUI".into(),
                decimals: 9,
                default_price_usd: 1.50,
                min_profit: 1_000_000,
                gecko_pool_address: None,
            },
            ProfitTokenConfig {
                token: "0xusdc::usdc::USDC".into(),
                symbol: "USDC".into(),
                decimals: 6,
                default_price_usd: 1.00,
                min_profit: 100_000,
                gecko_pool_address: None,
            },
        ])
    }

    #[test]
    fn test_to_usd() {
        let pt = ProfitToken {
            token: Arc::from("SUI"),
            symbol: "SUI".into(),
            decimals: 9,
            default_price_usd: 1.50,
            min_profit_base_units: 1_000_000,
            gecko_pool_address: None,
            price_usd: 1.50,
        };

        // 1 SUI = 1_000_000_000 base units at $1.50
        let usd = pt.to_usd(1_000_000_000);
        assert!((usd - 1.50).abs() < 0.001);

        // 0.001 SUI = 1_000_000 base units
        let usd = pt.to_usd(1_000_000);
        assert!((usd - 0.0015).abs() < 0.0001);
    }

    #[test]
    fn test_from_usd() {
        let pt = ProfitToken {
            token: Arc::from("SUI"),
            symbol: "SUI".into(),
            decimals: 9,
            default_price_usd: 1.50,
            min_profit_base_units: 1_000_000,
            gecko_pool_address: None,
            price_usd: 1.50,
        };

        // $1.50 should be ~1 SUI = 1_000_000_000 base units
        let amount = pt.from_usd(1.50);
        assert_eq!(amount, 1_000_000_000);
    }

    #[test]
    fn test_min_profit_for_usd() {
        let pt = ProfitToken {
            token: Arc::from("SUI"),
            symbol: "SUI".into(),
            decimals: 9,
            default_price_usd: 1.50,
            min_profit_base_units: 1_000_000,
            gecko_pool_address: None,
            price_usd: 1.50,
        };

        // $0.10 in SUI at $1.50 = 0.0667 SUI = 66,666,666 base units
        // This exceeds min_profit_base_units (1_000_000)
        let min = pt.min_profit_for_usd(0.10);
        assert!(min > 1_000_000);

        // Very small USD threshold → falls back to configured minimum
        let min = pt.min_profit_for_usd(0.000001);
        assert_eq!(min, 1_000_000);
    }

    #[test]
    fn test_lookup() {
        let registry = make_registry();
        assert_eq!(
            registry.lookup(&Arc::from("0x2::sui::SUI")),
            Some(0)
        );
        assert_eq!(
            registry.lookup(&Arc::from("0xusdc::usdc::USDC")),
            Some(1)
        );
        assert_eq!(
            registry.lookup(&Arc::from("UNKNOWN")),
            None
        );
    }

    #[test]
    fn test_best_profit_token() {
        let registry = make_registry();

        // SUI is index 0 (highest priority), USDC is index 1
        let tokens: Vec<CoinType> = vec![
            Arc::from("0xusdc::usdc::USDC"),
            Arc::from("0x2::sui::SUI"),
        ];
        let (best, idx) = registry.best_profit_token(&tokens).unwrap();
        assert_eq!(best.as_ref(), "0x2::sui::SUI");
        assert_eq!(idx, 0);

        // Only USDC
        let tokens: Vec<CoinType> = vec![Arc::from("0xusdc::usdc::USDC")];
        let (best, idx) = registry.best_profit_token(&tokens).unwrap();
        assert_eq!(best.as_ref(), "0xusdc::usdc::USDC");
        assert_eq!(idx, 1);

        // No profit tokens
        let tokens: Vec<CoinType> = vec![Arc::from("RANDOM")];
        assert!(registry.best_profit_token(&tokens).is_none());
    }

    #[tokio::test]
    async fn test_get_usd_value() {
        let registry = make_registry();

        // 1 SUI at default price $1.50
        let usd = registry
            .get_usd_value(&Arc::from("0x2::sui::SUI"), 1_000_000_000)
            .await;
        assert!((usd.unwrap() - 1.50).abs() < 0.001);

        // Unknown token
        let usd = registry
            .get_usd_value(&Arc::from("UNKNOWN"), 1_000_000)
            .await;
        assert!(usd.is_none());
    }
}
