use arb_types::error::ArbError;
use arb_types::event::SwapEventData;
use arb_types::pool::{object_id_from_hex, Dex};
use dex_common::SwapEventParser;

use crate::TURBOS_SWAP_EVENT_TYPE;

pub struct TurbosEventParser;

impl SwapEventParser for TurbosEventParser {
    fn parse_swap_event(
        event_type: &str,
        parsed_json: &serde_json::Value,
    ) -> Result<Option<SwapEventData>, ArbError> {
        if event_type != TURBOS_SWAP_EVENT_TYPE {
            return Ok(None);
        }

        let pool_id_str = parsed_json["pool"]
            .as_str()
            .ok_or_else(|| ArbError::InvalidData("missing pool in Turbos SwapEvent".into()))?;

        Ok(Some(SwapEventData {
            pool_id: object_id_from_hex(pool_id_str)?,
            dex: Dex::Turbos,
            a_to_b: parsed_json["atob"].as_bool().unwrap_or(true),
            amount_in: parse_u64_field(parsed_json, "amount_in")?,
            amount_out: parse_u64_field(parsed_json, "amount_out")?,
            fee_amount: parse_u64_field(parsed_json, "fee_amount")?,
            after_sqrt_price: parse_u128_field(parsed_json, "after_sqrt_price")?,
            vault_a_amount: parse_u64_field(parsed_json, "vault_a_amount")?,
            vault_b_amount: parse_u64_field(parsed_json, "vault_b_amount")?,
            steps: parse_u64_field(parsed_json, "steps")?,
        }))
    }
}

fn parse_u64_field(json: &serde_json::Value, field: &str) -> Result<u64, ArbError> {
    json[field]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| json[field].as_u64())
        .ok_or_else(|| ArbError::InvalidData(format!("missing or invalid {} in SwapEvent", field)))
}

fn parse_u128_field(json: &serde_json::Value, field: &str) -> Result<u128, ArbError> {
    json[field]
        .as_str()
        .and_then(|s| s.parse::<u128>().ok())
        .ok_or_else(|| {
            ArbError::InvalidData(format!("missing or invalid {} in SwapEvent", field))
        })
}
