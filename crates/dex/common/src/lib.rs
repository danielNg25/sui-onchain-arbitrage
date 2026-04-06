use std::collections::HashSet;
use std::sync::Arc;

use arb_types::error::ArbError;
use arb_types::event::SwapEstimate;
use arb_types::pool::{CoinType, Dex, ObjectId};

// ---------------------------------------------------------------------------
// DexRegistry — DEX-level operations (discovery, pool lookup)
// ---------------------------------------------------------------------------

/// A DEX registry manages pool discovery and provides pool handles.
/// Each DEX implementation owns its internal pool state.
#[async_trait::async_trait]
pub trait DexRegistry: Send + Sync {
    /// Which DEX this registry is for.
    fn dex(&self) -> Dex;

    /// On-chain event type strings this DEX emits (for routing).
    fn event_types(&self) -> &[&str];

    /// Check if a Move type string belongs to this DEX's pools.
    fn matches_pool_type(&self, type_string: &str) -> bool;

    /// Discover all pools from on-chain registries.
    /// Ingests them internally and returns pool IDs + coin pairs.
    async fn discover_pools(
        &self,
        client: &sui_client::SuiClient,
        whitelisted_tokens: &HashSet<String>,
    ) -> Result<Vec<(ObjectId, CoinType, CoinType)>, ArbError>;

    /// Ingest a single pool object (BCS bytes from RPC).
    /// Returns the pool ID and coin pair if successfully ingested.
    fn ingest_pool_object(
        &self,
        object_id: ObjectId,
        bcs_bytes: &[u8],
        type_params: &[String],
        object_version: u64,
        initial_shared_version: u64,
    ) -> Result<Option<(ObjectId, CoinType, CoinType)>, ArbError>;

    /// Get a pool handle for pool-level operations.
    fn pool(&self, pool_id: &ObjectId) -> Option<Arc<dyn Pool>>;

    /// Get all pool IDs managed by this registry.
    fn pool_ids(&self) -> Vec<ObjectId>;

    /// Get pools containing a specific token.
    fn pools_for_token(&self, token: &CoinType) -> Vec<ObjectId>;

    /// Number of pools managed.
    fn pool_count(&self) -> usize;
}

// ---------------------------------------------------------------------------
// Pool — Pool-level operations (state, events, swap estimation)
// ---------------------------------------------------------------------------

/// A single pool instance. Each DEX implements this with its own internal state.
/// Upstream code interacts through this trait without knowing the DEX-specific details.
#[async_trait::async_trait]
pub trait Pool: Send + Sync {
    /// Pool's on-chain object ID.
    fn id(&self) -> ObjectId;

    /// Which DEX this pool belongs to.
    fn dex(&self) -> Dex;

    /// The tokens this pool trades.
    fn coins(&self) -> Vec<CoinType>;

    /// Whether this pool is active/tradeable.
    fn is_active(&self) -> bool;

    /// Fee rate in PPM (denominator 1_000_000).
    fn fee_rate(&self) -> u64;

    /// Fetch tick/level data from chain and store internally.
    async fn fetch_price_data(
        &self,
        client: &sui_client::SuiClient,
    ) -> Result<(), ArbError>;

    /// Apply a raw on-chain event to update internal state.
    ///
    /// Each pool decodes events in its own format (swap, add/remove liquidity,
    /// order fills, etc). The caller doesn't know or care about internals.
    ///
    /// Returns:
    /// - `Ok(None)` — event not relevant to this pool
    /// - `Ok(Some(false))` — applied, price data still valid
    /// - `Ok(Some(true))` — applied, price data needs re-fetching
    fn apply_event(
        &self,
        event_type: &str,
        parsed_json: &serde_json::Value,
    ) -> Result<Option<bool>, ArbError>;

    /// Estimate swap output using internal state. Pure local computation.
    /// `token_in` specifies direction — caller doesn't need to know internal ordering.
    fn estimate_swap(
        &self,
        token_in: &CoinType,
        amount_in: u64,
    ) -> Result<SwapEstimate, ArbError>;
}

// ---------------------------------------------------------------------------
// Helper functions (shared across DEX implementations)
// ---------------------------------------------------------------------------

/// Parse Move type string into type parameters.
///
/// Input: `"0xabc::pool::Pool<0x2::sui::SUI, 0xdef::usdc::USDC>"`
/// Output: `["0x2::sui::SUI", "0xdef::usdc::USDC"]`
pub fn parse_type_params(type_string: &str) -> Vec<String> {
    let Some(start) = type_string.find('<') else {
        return Vec::new();
    };
    let Some(end) = type_string.rfind('>') else {
        return Vec::new();
    };
    if start >= end {
        return Vec::new();
    }
    let inner = &type_string[start + 1..end];

    let mut params = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for ch in inner.chars() {
        match ch {
            '<' => {
                depth += 1;
                current.push(ch);
            }
            '>' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                params.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        params.push(trimmed);
    }
    params
}

/// Check if a type parameter looks like a Turbos fee type.
pub fn is_fee_type(type_param: &str) -> bool {
    type_param.contains("::fee") && type_param.to_uppercase().contains("BPS")
}

/// Parse type params, separating coin types from fee type (for Turbos 3-param pools).
pub fn parse_type_params_with_fee(type_string: &str) -> (Vec<String>, Option<String>) {
    let all_params = parse_type_params(type_string);
    let mut coin_params = Vec::new();
    let mut fee_type = None;
    for param in all_params {
        if is_fee_type(&param) {
            fee_type = Some(param);
        } else {
            coin_params.push(param);
        }
    }
    (coin_params, fee_type)
}

// Re-export Tick for convenience (shared by CLMM implementations)
pub use arb_types::tick::Tick;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cetus_type_params() {
        let type_str = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::pool::Pool<0x2::sui::SUI, 0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC>";
        let params = parse_type_params(type_str);
        assert_eq!(params.len(), 2);
        assert_eq!(params[0], "0x2::sui::SUI");
        assert_eq!(params[1], "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC");
    }

    #[test]
    fn test_parse_turbos_type_params() {
        let type_str = "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1::pool::Pool<0x2::sui::SUI, 0xdba34672::usdc::USDC, 0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1::fee3000bps::FEE3000BPS>";
        let (coins, fee) = parse_type_params_with_fee(type_str);
        assert_eq!(coins.len(), 2);
        assert_eq!(coins[0], "0x2::sui::SUI");
        assert!(fee.is_some());
        assert!(fee.unwrap().contains("fee3000bps"));
    }

    #[test]
    fn test_is_fee_type() {
        assert!(is_fee_type("0x91bf::fee3000bps::FEE3000BPS"));
        assert!(is_fee_type("0x91bf::fee10000bps::FEE10000BPS"));
        assert!(!is_fee_type("0x2::sui::SUI"));
        assert!(!is_fee_type("0xdba::usdc::USDC"));
    }

    #[test]
    fn test_parse_no_type_params() {
        assert!(parse_type_params("SomeType").is_empty());
    }
}
