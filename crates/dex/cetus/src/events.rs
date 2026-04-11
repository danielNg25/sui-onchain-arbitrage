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

/// Parse a Cetus SwapEvent JSON into SwapEventData.
///
/// Cetus SwapEvent fields:
///   pool, atob, amount_in, amount_out, fee_amount,
///   after_sqrt_price, vault_a_amount, vault_b_amount, steps
pub fn parse_swap_event_data(json: &serde_json::Value) -> Result<SwapEventData, ArbError> {
    let pool_id_str = json["pool"]
        .as_str()
        .ok_or_else(|| ArbError::InvalidData("missing pool in Cetus SwapEvent".into()))?;
    let pool_id = object_id_from_hex(pool_id_str)?;

    let a_to_b = json["atob"].as_bool().unwrap_or(true);
    let amount_in = parse_u64_field(json, "amount_in")?;
    let amount_out = parse_u64_field(json, "amount_out")?;
    let fee_amount = parse_u64_field(json, "fee_amount")?;
    let after_sqrt_price = parse_u128_field(json, "after_sqrt_price")?;
    let vault_a_amount = parse_u64_field(json, "vault_a_amount")?;
    let vault_b_amount = parse_u64_field(json, "vault_b_amount")?;
    let steps = parse_u64_field(json, "steps")?;

    Ok(SwapEventData {
        pool_id,
        dex: Dex::Cetus,
        a_to_b,
        amount_in,
        amount_out,
        fee_amount,
        after_sqrt_price,
        vault_a_amount,
        vault_b_amount,
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cetus_swap_event() {
        let json = serde_json::json!({
            "pool": "0x0000000000000000000000000000000000000000000000000000000000000001",
            "atob": true,
            "amount_in": "1000000",
            "amount_out": "990000",
            "fee_amount": "2500",
            "after_sqrt_price": "18446744073709551616",
            "before_sqrt_price": "18446744073709551616",
            "vault_a_amount": "5000000",
            "vault_b_amount": "4000000",
            "steps": "1"
        });

        let data = parse_swap_event_data(&json).unwrap();
        assert_eq!(data.dex, Dex::Cetus);
        assert!(data.a_to_b);
        assert_eq!(data.amount_in, 1_000_000);
        assert_eq!(data.amount_out, 990_000);
        assert_eq!(data.fee_amount, 2500);
        assert_eq!(data.vault_a_amount, 5_000_000);
        assert_eq!(data.vault_b_amount, 4_000_000);
        assert_eq!(data.steps, 1);
        assert_eq!(data.pool_id[31], 1);
    }
}
