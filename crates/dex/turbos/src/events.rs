use arb_types::error::ArbError;

pub(crate) fn parse_u64_field(json: &serde_json::Value, field: &str) -> Result<u64, ArbError> {
    json[field]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| json[field].as_u64())
        .ok_or_else(|| ArbError::InvalidData(format!("missing or invalid {} in event", field)))
}

pub(crate) fn parse_u128_field(json: &serde_json::Value, field: &str) -> Result<u128, ArbError> {
    json[field]
        .as_str()
        .and_then(|s| s.parse::<u128>().ok())
        .ok_or_else(|| ArbError::InvalidData(format!("missing or invalid {} in event", field)))
}

/// Parse a Sui I32 field { "bits": u32 } → i32.
pub(crate) fn parse_i32_field(json: &serde_json::Value, field: &str) -> Result<i32, ArbError> {
    let bits = json[field]
        .get("bits")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ArbError::InvalidData(format!("missing or invalid {} in event", field)))?;
    Ok(bits as u32 as i32)
}
