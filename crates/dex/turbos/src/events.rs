use arb_types::error::ArbError;
use arb_types::event::SwapEventData;
use arb_types::pool::{object_id_from_hex, Dex};

pub fn parse_u64_field(json: &serde_json::Value, field: &str) -> Result<u64, ArbError> {
    json[field]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| json[field].as_u64())
        .ok_or_else(|| ArbError::InvalidData(format!("missing or invalid {} in event", field)))
}

pub fn parse_u128_field(json: &serde_json::Value, field: &str) -> Result<u128, ArbError> {
    json[field]
        .as_str()
        .and_then(|s| s.parse::<u128>().ok())
        .ok_or_else(|| ArbError::InvalidData(format!("missing or invalid {} in event", field)))
}

/// Parse a Sui I32 field { "bits": u32 } → i32.
pub fn parse_i32_field(json: &serde_json::Value, field: &str) -> Result<i32, ArbError> {
    let bits = json[field]
        .get("bits")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ArbError::InvalidData(format!("missing or invalid {} in event", field)))?;
    Ok(bits as u32 as i32)
}

/// Parse a Turbos SwapEvent JSON into SwapEventData.
///
/// Turbos SwapEvent fields:
///   pool, a_to_b, amount_a, amount_b, fee_amount,
///   sqrt_price, tick_current_index, tick_pre_index, liquidity
///
/// Turbos doesn't provide vault_a_amount/vault_b_amount — set to 0.
/// Steps derived from tick_current_index != tick_pre_index.
pub fn parse_swap_event_data(json: &serde_json::Value) -> Result<SwapEventData, ArbError> {
    let pool_id_str = json["pool"]
        .as_str()
        .ok_or_else(|| ArbError::InvalidData("missing pool in Turbos SwapEvent".into()))?;
    let pool_id = object_id_from_hex(pool_id_str)?;

    let a_to_b = json["a_to_b"].as_bool().unwrap_or(true);
    let amount_a = parse_u64_field(json, "amount_a")?;
    let amount_b = parse_u64_field(json, "amount_b")?;
    let after_sqrt_price = parse_u128_field(json, "sqrt_price")?;
    let tick_current = parse_i32_field(json, "tick_current_index")?;
    let tick_pre = parse_i32_field(json, "tick_pre_index")?;

    // Derive amount_in/out from direction
    let (amount_in, amount_out) = if a_to_b {
        (amount_a, amount_b)
    } else {
        (amount_b, amount_a)
    };

    // fee_amount may or may not be present in Turbos events
    let fee_amount = parse_u64_field(json, "fee_amount").unwrap_or(0);

    let steps = if tick_current != tick_pre { 1 } else { 0 };

    Ok(SwapEventData {
        pool_id,
        dex: Dex::Turbos,
        a_to_b,
        amount_in,
        amount_out,
        fee_amount,
        after_sqrt_price,
        vault_a_amount: 0,
        vault_b_amount: 0,
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_turbos_swap_event_a_to_b() {
        let json = serde_json::json!({
            "pool": "0x0000000000000000000000000000000000000000000000000000000000000002",
            "a_to_b": true,
            "amount_a": "500000",
            "amount_b": "495000",
            "fee_amount": "1250",
            "sqrt_price": "18446744073709551616",
            "tick_current_index": {"bits": 100},
            "tick_pre_index": {"bits": 101},
            "liquidity": "1000000000"
        });

        let data = parse_swap_event_data(&json).unwrap();
        assert_eq!(data.dex, Dex::Turbos);
        assert!(data.a_to_b);
        assert_eq!(data.amount_in, 500_000); // amount_a when a_to_b
        assert_eq!(data.amount_out, 495_000); // amount_b when a_to_b
        assert_eq!(data.fee_amount, 1250);
        assert_eq!(data.steps, 1); // tick_current != tick_pre
        assert_eq!(data.pool_id[31], 2);
    }

    #[test]
    fn test_parse_turbos_swap_event_b_to_a() {
        let json = serde_json::json!({
            "pool": "0x0000000000000000000000000000000000000000000000000000000000000003",
            "a_to_b": false,
            "amount_a": "495000",
            "amount_b": "500000",
            "sqrt_price": "18446744073709551616",
            "tick_current_index": {"bits": 100},
            "tick_pre_index": {"bits": 100},
            "liquidity": "1000000000"
        });

        let data = parse_swap_event_data(&json).unwrap();
        assert!(!data.a_to_b);
        assert_eq!(data.amount_in, 500_000); // amount_b when b_to_a
        assert_eq!(data.amount_out, 495_000); // amount_a when b_to_a
        assert_eq!(data.fee_amount, 0); // no fee_amount field
        assert_eq!(data.steps, 0); // tick_current == tick_pre
    }
}
