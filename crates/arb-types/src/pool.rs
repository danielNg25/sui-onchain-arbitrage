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

#[derive(Debug, Clone)]
pub struct PoolState {
    pub id: ObjectId,
    pub dex: Dex,
    pub coin_a: CoinType,
    pub coin_b: CoinType,
    /// Q64.64 fixed-point sqrt price.
    pub sqrt_price: u128,
    pub tick_current: i32,
    /// Active liquidity at current tick.
    pub liquidity: u128,
    /// Fee rate in PPM (denominator = 1_000_000).
    pub fee_rate: u64,
    pub tick_spacing: u32,
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub is_active: bool,
    /// Object ID of the ticks storage (SkipList UID for Cetus, Table UID for Turbos).
    pub ticks_table_id: ObjectId,
    pub metadata: DexPoolMetadata,
    pub object_version: u64,
}

#[derive(Debug, Clone)]
pub enum DexPoolMetadata {
    Cetus {
        initial_shared_version: u64,
    },
    Turbos {
        fee_type: CoinType,
        initial_shared_version: u64,
    },
}

/// Parse a hex string (with or without "0x" prefix) into a 32-byte ObjectId.
pub fn object_id_from_hex(hex_str: &str) -> Result<ObjectId, crate::error::ArbError> {
    let stripped = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    // Pad to 64 hex chars (32 bytes) if shorter
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

impl PoolState {
    /// Canonical pair key with coin types in sorted order.
    pub fn pair_key(&self) -> (CoinType, CoinType) {
        if self.coin_a <= self.coin_b {
            (self.coin_a.clone(), self.coin_b.clone())
        } else {
            (self.coin_b.clone(), self.coin_a.clone())
        }
    }
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
