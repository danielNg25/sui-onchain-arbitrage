use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arb_types::config::ProfitTokenConfig;
use arb_types::pool::CoinType;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::error::EngineError;

const GECKO_TERMINAL_BATCH_SIZE: usize = 30;
const GECKO_TERMINAL_NETWORK: &str = "sui-network";

/// GeckoTerminal simple token price response.
#[derive(Debug, serde::Deserialize)]
struct GeckoTerminalResponse {
    data: GeckoTerminalData,
}

#[derive(Debug, serde::Deserialize)]
struct GeckoTerminalData {
    attributes: GeckoTerminalAttributes,
}

#[derive(Debug, serde::Deserialize)]
struct GeckoTerminalAttributes {
    token_prices: HashMap<String, Option<String>>,
}

/// A profit token with price tracking.
#[derive(Debug, Clone)]
pub struct ProfitToken {
    pub token: CoinType,
    pub symbol: String,
    pub decimals: u8,
    pub default_price_usd: f64,
    pub min_profit_base_units: u64,
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

    /// Update prices from GeckoTerminal using the simple token price endpoint.
    /// Batches up to 30 token addresses per request, matching the EVM reference.
    /// Uses the token's Move type string (e.g. "0x2::sui::SUI") as the address key.
    /// On failure, retains the previous price (or default_price_usd).
    pub async fn update_prices(&self) -> Result<(), EngineError> {
        let tokens = self.tokens.read().await;
        let token_types: Vec<CoinType> = tokens.iter().map(|pt| pt.token.clone()).collect();
        drop(tokens);

        if token_types.is_empty() {
            return Ok(());
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| EngineError::PriceFetch(e.to_string()))?;

        let mut all_prices: HashMap<String, f64> = HashMap::new();

        for chunk in token_types.chunks(GECKO_TERMINAL_BATCH_SIZE) {
            let address_str = chunk
                .iter()
                .map(|t| t.as_ref())
                .collect::<Vec<_>>()
                .join(",");

            info!(
                count = chunk.len(),
                "fetching prices from GeckoTerminal"
            );

            let url = format!(
                "https://api.geckoterminal.com/api/v2/simple/networks/{}/token_price/{}",
                GECKO_TERMINAL_NETWORK, address_str
            );

            let resp = client
                .get(&url)
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(|e| EngineError::PriceFetch(e.to_string()))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                warn!(
                    status = %status,
                    body = %body,
                    "GeckoTerminal API error, keeping previous prices"
                );
                continue;
            }

            let data: GeckoTerminalResponse = resp
                .json()
                .await
                .map_err(|e| EngineError::PriceFetch(e.to_string()))?;

            for token in chunk {
                // GeckoTerminal returns keys matching the input token type string
                if let Some(Some(price_str)) = data
                    .data
                    .attributes
                    .token_prices
                    .get(token.as_ref())
                {
                    if let Ok(price) = price_str.parse::<f64>() {
                        all_prices.insert(token.to_string(), price);
                    }
                }
            }
        }

        // Apply fetched prices
        let mut tokens = self.tokens.write().await;
        for pt in tokens.iter_mut() {
            if let Some(&price) = all_prices.get(pt.token.as_ref()) {
                info!(
                    symbol = %pt.symbol,
                    old_price = pt.price_usd,
                    new_price = price,
                    "updated profit token price"
                );
                pt.price_usd = price;
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
            },
            ProfitTokenConfig {
                token: "0xusdc::usdc::USDC".into(),
                symbol: "USDC".into(),
                decimals: 6,
                default_price_usd: 1.00,
                min_profit: 100_000,
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

    #[tokio::test]
    #[ignore]
    async fn test_gecko_terminal_price_fetch() {
        let registry = ProfitTokenRegistry::from_config(&[
            ProfitTokenConfig {
                token: "0x2::sui::SUI".into(),
                symbol: "SUI".into(),
                decimals: 9,
                default_price_usd: 1.50,
                min_profit: 1_000_000,
            },
            ProfitTokenConfig {
                token: "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC".into(),
                symbol: "USDC".into(),
                decimals: 6,
                default_price_usd: 1.00,
                min_profit: 100_000,
            },
        ]);

        registry.update_prices().await.unwrap();

        // SUI price should be updated from default
        let sui_price = {
            let tokens = registry.tokens.read().await;
            tokens[0].price_usd
        };
        println!("SUI price: ${:.4}", sui_price);
        assert!(sui_price > 0.0, "SUI price should be positive");
        assert!(sui_price != 1.50, "SUI price should have been updated from default");

        // USDC price should be ~$1.00
        let usdc_price = {
            let tokens = registry.tokens.read().await;
            tokens[1].price_usd
        };
        println!("USDC price: ${:.6}", usdc_price);
        assert!((usdc_price - 1.0).abs() < 0.05, "USDC should be ~$1.00");
    }
}
