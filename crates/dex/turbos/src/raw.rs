// BCS raw structs must define all fields in exact Move struct order,
// even if we only read a subset. Suppress dead_code warnings.
#![allow(dead_code)]

use serde::Deserialize;

/// Top-level BCS layout matching Turbos `Pool<CoinA, CoinB, FeeType>` Move struct.
///
/// CRITICAL: Turbos is closed source. This layout is inferred from SDK stubs
/// and must be validated against real BCS snapshots.
#[derive(Deserialize, Debug)]
pub(crate) struct TurbosPoolRaw {
    /// UID — 32 bytes.
    pub id: [u8; 32],
    /// Balance<CoinA> — u64.
    pub coin_a: u64,
    /// Balance<CoinB> — u64.
    pub coin_b: u64,
    pub protocol_fees_a: u64,
    pub protocol_fees_b: u64,
    /// Q64.64 fixed-point sqrt price.
    pub sqrt_price: u128,
    /// I32 { bits: u32 } — current tick index.
    pub tick_current_index: TurbosI32,
    pub tick_spacing: u32,
    pub max_liquidity_per_tick: u128,
    /// Fee numerator, denominator = 1_000_000.
    pub fee: u32,
    pub fee_protocol: u32,
    /// Pool is operational when true (inverse of Cetus is_pause).
    pub unlocked: bool,
    /// Q64.64 fee growth accumulators.
    pub fee_growth_global_a: u128,
    pub fee_growth_global_b: u128,
    /// Active liquidity at current tick.
    pub liquidity: u128,
    /// Table<I32, u256> tick bitmap — stored as UID (32 bytes).
    /// Actual bitmap entries are dynamic fields on this UID.
    pub tick_map: [u8; 32],
    pub deploy_time_ms: u64,
    pub reward_infos: Vec<TurbosRewardInfoRaw>,
    pub reward_last_updated_time_ms: u64,
}

#[derive(Deserialize, Debug)]
pub(crate) struct TurbosI32 {
    pub bits: u32,
}

impl TurbosI32 {
    pub fn to_i32(&self) -> i32 {
        self.bits as i32
    }
}

/// Turbos reward info — layout inferred from SDK.
#[derive(Deserialize, Debug)]
pub(crate) struct TurbosRewardInfoRaw {
    /// TypeName stored as String in BCS.
    pub reward_coin_type: TurbosTypeName,
    pub emissions_per_second: u128,
    pub growth_global: u128,
}

/// Move TypeName wrapper.
#[derive(Deserialize, Debug)]
pub(crate) struct TurbosTypeName {
    pub name: String,
}

/// Turbos tick data — individual dynamic field on the pool.
///
/// Layout inferred from UniV3-style tick structure.
#[derive(Deserialize, Debug)]
pub(crate) struct TurbosTickRaw {
    pub initialized: bool,
    pub liquidity_gross: u128,
    pub liquidity_net: TurbosI128,
    pub fee_growth_outside_a: u128,
    pub fee_growth_outside_b: u128,
    pub reward_growths_outside: Vec<u128>,
    pub tick_cumulative_outside: TurbosI64,
    pub seconds_per_liquidity_outside: u128,
    pub seconds_outside: u32,
}

#[derive(Deserialize, Debug)]
pub(crate) struct TurbosI128 {
    pub bits: u128,
}

impl TurbosI128 {
    pub fn to_i128(&self) -> i128 {
        self.bits as i128
    }
}

#[derive(Deserialize, Debug)]
pub(crate) struct TurbosI64 {
    pub bits: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_turbos_i32_positive() {
        let i = TurbosI32 { bits: 100 };
        assert_eq!(i.to_i32(), 100);
    }

    #[test]
    fn test_turbos_i32_negative() {
        let i = TurbosI32 {
            bits: (-100i32) as u32,
        };
        assert_eq!(i.to_i32(), -100);
    }
}
