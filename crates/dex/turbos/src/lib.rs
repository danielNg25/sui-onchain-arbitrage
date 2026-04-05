mod events;
mod raw;
mod ticks;

pub use events::TurbosEventParser;
pub use ticks::TurbosTickFetcher;

use std::sync::Arc;

use arb_types::error::ArbError;
use arb_types::pool::{Dex, DexPoolMetadata, ObjectId, PoolState};
use dex_common::PoolDeserializer;

use raw::TurbosPoolRaw;

pub struct TurbosDeserializer;

impl PoolDeserializer for TurbosDeserializer {
    fn deserialize_pool(
        object_id: ObjectId,
        bcs_bytes: &[u8],
        type_params: &[String],
        object_version: u64,
        initial_shared_version: u64,
    ) -> Result<PoolState, ArbError> {
        let raw: TurbosPoolRaw = bcs::from_bytes(bcs_bytes).map_err(|e| {
            ArbError::BcsDeserialize(format!("Turbos pool deser failed: {}", e))
        })?;

        if type_params.len() < 2 {
            return Err(ArbError::InvalidData(format!(
                "Turbos pool requires at least 2 type params, got {}",
                type_params.len()
            )));
        }

        // Extract fee type from 3rd type param if present
        let fee_type = type_params
            .get(2)
            .map(|s| Arc::from(s.as_str()))
            .unwrap_or_else(|| Arc::from(""));

        Ok(PoolState {
            id: object_id,
            dex: Dex::Turbos,
            coin_a: Arc::from(type_params[0].as_str()),
            coin_b: Arc::from(type_params[1].as_str()),
            sqrt_price: raw.sqrt_price,
            tick_current: raw.tick_current_index.to_i32(),
            liquidity: raw.liquidity,
            // Turbos fee field is already in PPM (denominator 1_000_000).
            // The fee: u32 field stores the fee numerator directly.
            fee_rate: raw.fee as u64,
            tick_spacing: raw.tick_spacing,
            reserve_a: raw.coin_a,
            reserve_b: raw.coin_b,
            is_active: raw.unlocked,
            ticks_table_id: raw.tick_map,
            metadata: DexPoolMetadata::Turbos {
                fee_type,
                initial_shared_version,
            },
            object_version,
        })
    }
}

/// Turbos CLMM swap event type string.
pub const TURBOS_SWAP_EVENT_TYPE: &str =
    "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1::pool::SwapEvent";
