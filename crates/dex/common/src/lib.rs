use arb_types::error::ArbError;
use arb_types::event::SwapEventData;
use arb_types::pool::{ObjectId, PoolState};
use arb_types::tick::Tick;

/// Deserialize raw BCS bytes into a normalized PoolState.
pub trait PoolDeserializer {
    /// `type_params` are the Move type parameters extracted from the object's type string.
    fn deserialize_pool(
        object_id: ObjectId,
        bcs_bytes: &[u8],
        type_params: &[String],
        object_version: u64,
        initial_shared_version: u64,
    ) -> Result<PoolState, ArbError>;
}

/// Fetch initialized ticks for a pool from on-chain dynamic fields.
#[async_trait::async_trait]
pub trait TickFetcher {
    async fn fetch_ticks(
        client: &sui_client::SuiClient,
        pool: &PoolState,
    ) -> Result<Vec<Tick>, ArbError>;
}

/// Parse swap event from JSON into SwapEventData.
pub trait SwapEventParser {
    fn parse_swap_event(
        event_type: &str,
        parsed_json: &serde_json::Value,
    ) -> Result<Option<SwapEventData>, ArbError>;
}

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

    // Split by ", " but handle nested generics by tracking angle bracket depth
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
///
/// Returns (coin_type_params, optional_fee_type).
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
