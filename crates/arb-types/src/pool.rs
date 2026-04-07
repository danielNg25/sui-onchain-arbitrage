use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Full Move type string, e.g. "0x2::sui::SUI".
pub type CoinType = Arc<str>;

/// 32-byte Sui object ID.
pub type ObjectId = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Dex {
    Cetus,
    Turbos,
}

impl fmt::Display for Dex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Dex::Cetus => write!(f, "Cetus"),
            Dex::Turbos => write!(f, "Turbos"),
        }
    }
}

/// Canonical pair key with coin types in sorted order.
pub fn pair_key(a: &CoinType, b: &CoinType) -> (CoinType, CoinType) {
    if a <= b {
        (a.clone(), b.clone())
    } else {
        (b.clone(), a.clone())
    }
}

/// Parse a hex string (with or without "0x" prefix) into a 32-byte ObjectId.
pub fn object_id_from_hex(hex_str: &str) -> Result<ObjectId, crate::error::ArbError> {
    let stripped = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let padded = format!("{:0>64}", stripped);
    let bytes = hex_decode(&padded).map_err(|e| {
        crate::error::ArbError::Config(format!("invalid hex object ID '{}': {}", hex_str, e))
    })?;
    let mut id = [0u8; 32];
    id.copy_from_slice(&bytes);
    Ok(id)
}

/// Format a 32-byte ObjectId as "0x..." hex string.
pub fn object_id_to_hex(id: &ObjectId) -> String {
    format!("0x{}", hex_encode(id))
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd length".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_object_id_hex_roundtrip() {
        let hex = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";
        let id = object_id_from_hex(hex).unwrap();
        let back = object_id_to_hex(&id);
        assert_eq!(back, hex);
    }

    #[test]
    fn test_object_id_from_hex_no_prefix() {
        let hex = "1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";
        let id = object_id_from_hex(hex).unwrap();
        let back = object_id_to_hex(&id);
        assert_eq!(back, format!("0x{}", hex));
    }

    #[test]
    fn test_object_id_from_hex_short() {
        let hex = "0x2";
        let id = object_id_from_hex(hex).unwrap();
        assert_eq!(id[31], 2);
        assert_eq!(id[0], 0);
    }
}
