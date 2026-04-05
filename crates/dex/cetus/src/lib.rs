mod events;
mod raw;
mod ticks;

pub use events::CetusEventParser;
pub use ticks::CetusTickFetcher;

use std::sync::Arc;

use arb_types::error::ArbError;
use arb_types::pool::{Dex, DexPoolMetadata, ObjectId, PoolState};
use dex_common::PoolDeserializer;

pub struct CetusDeserializer;

impl PoolDeserializer for CetusDeserializer {
    fn deserialize_pool(
        object_id: ObjectId,
        bcs_bytes: &[u8],
        type_params: &[String],
        object_version: u64,
        initial_shared_version: u64,
    ) -> Result<PoolState, ArbError> {
        let raw = raw::parse_cetus_pool(bcs_bytes)?;

        if type_params.len() < 2 {
            return Err(ArbError::InvalidData(format!(
                "Cetus pool requires 2 type params, got {}",
                type_params.len()
            )));
        }

        Ok(PoolState {
            id: object_id,
            dex: Dex::Cetus,
            coin_a: Arc::from(type_params[0].as_str()),
            coin_b: Arc::from(type_params[1].as_str()),
            sqrt_price: raw.current_sqrt_price,
            tick_current: raw.current_tick_index,
            liquidity: raw.liquidity,
            fee_rate: raw.fee_rate,
            tick_spacing: raw.tick_spacing,
            reserve_a: raw.coin_a,
            reserve_b: raw.coin_b,
            is_active: !raw.is_pause,
            ticks_table_id: raw.ticks_table_id,
            metadata: DexPoolMetadata::Cetus {
                initial_shared_version,
            },
            object_version,
        })
    }
}

/// Check if a Cetus pool is paused from JSON content.
/// Use this as a fallback since BCS parsing of `is_pause` requires
/// fully deserializing RewarderManager and PositionManager.
pub fn is_pool_paused(content: &serde_json::Value) -> bool {
    content
        .get("fields")
        .and_then(|f| f.get("is_pause"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Cetus CLMM swap event type string.
pub const CETUS_SWAP_EVENT_TYPE: &str =
    "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::pool::SwapEvent";
