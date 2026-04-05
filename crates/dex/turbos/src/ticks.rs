use arb_types::error::ArbError;
use arb_types::pool::{object_id_to_hex, PoolState};
use arb_types::tick::Tick;
use sui_client::{ObjectDataOptions, SuiClient};
use tracing::debug;

use crate::raw::TurbosTickRaw;

pub struct TurbosTickFetcher;

#[async_trait::async_trait]
impl dex_common::TickFetcher for TurbosTickFetcher {
    async fn fetch_ticks(
        client: &SuiClient,
        pool: &PoolState,
    ) -> Result<Vec<Tick>, ArbError> {
        let parent_id = object_id_to_hex(&pool.ticks_table_id);
        let mut ticks = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let page = client
                .get_dynamic_fields(&parent_id, cursor, Some(50))
                .await
                .map_err(|e| ArbError::Rpc(format!("fetch Turbos tick fields: {}", e)))?;

            if page.data.is_empty() {
                break;
            }

            let obj_ids: Vec<String> = page.data.iter().map(|f| f.object_id.clone()).collect();
            let objects = client
                .multi_get_objects(&obj_ids, ObjectDataOptions::bcs())
                .await
                .map_err(|e| ArbError::Rpc(format!("batch fetch Turbos tick objects: {}", e)))?;

            for obj_resp in &objects {
                let Some(data) = &obj_resp.data else {
                    continue;
                };
                let bcs_bytes = data.bcs_bytes().map_err(|e| {
                    ArbError::BcsDeserialize(format!("Turbos tick BCS bytes: {}", e))
                })?;

                // Extract tick index from the dynamic field name.
                // The name is the I32 tick index stored in the DynamicFieldInfo.
                let tick_index = extract_tick_index_from_field(
                    &page.data,
                    &data.object_id,
                );

                match deserialize_turbos_tick(&bcs_bytes, tick_index) {
                    Ok(Some(tick)) => ticks.push(tick),
                    Ok(None) => {} // uninitialized tick
                    Err(e) => {
                        debug!(
                            "failed to deserialize Turbos tick for pool {}: {}",
                            object_id_to_hex(&pool.id),
                            e
                        );
                    }
                }
            }

            if !page.has_next_page {
                break;
            }
            cursor = page.next_cursor;
        }

        ticks.sort_by_key(|t| t.index);
        Ok(ticks)
    }
}

/// Try to extract tick index from dynamic field info by matching object ID.
fn extract_tick_index_from_field(
    fields: &[sui_client::DynamicFieldInfo],
    object_id: &str,
) -> Option<i32> {
    for field in fields {
        if field.object_id == object_id {
            // The name value contains the I32 { bits: u32 } tick index.
            // It could be serialized as { "bits": N } or just N.
            if let Some(bits) = field.name.value.get("bits").and_then(|v| v.as_u64()) {
                return Some(bits as u32 as i32);
            }
            if let Some(n) = field.name.value.as_u64() {
                return Some(n as u32 as i32);
            }
        }
    }
    None
}

/// Deserialize a Turbos tick from BCS bytes.
///
/// Dynamic field wrapper: Field { id: [u8;32], name: I32, value: Tick }
/// I32 is { bits: u32 } = 4 bytes. So: 32 + 4 = 36 bytes before the tick value.
fn deserialize_turbos_tick(
    bcs_bytes: &[u8],
    tick_index: Option<i32>,
) -> Result<Option<Tick>, ArbError> {
    if bcs_bytes.len() < 36 {
        return Err(ArbError::BcsDeserialize(
            "Turbos tick BCS bytes too short".into(),
        ));
    }

    let raw: TurbosTickRaw = bcs::from_bytes(&bcs_bytes[36..]).map_err(|e| {
        ArbError::BcsDeserialize(format!("Turbos tick deser: {}", e))
    })?;

    if !raw.initialized {
        return Ok(None);
    }

    let index = tick_index.unwrap_or(0);

    Ok(Some(Tick {
        index,
        liquidity_net: raw.liquidity_net.to_i128(),
        liquidity_gross: raw.liquidity_gross,
        // sqrt_price will be computed by clmm-math in Phase 2.
        // For now store 0; pool-manager can fill this in later.
        sqrt_price: 0,
    }))
}
