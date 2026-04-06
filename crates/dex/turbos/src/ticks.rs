use arb_types::error::ArbError;
use arb_types::pool::{object_id_to_hex, ObjectId};
use arb_types::tick::Tick;
use sui_client::{ObjectDataOptions, SuiClient};
use tracing::debug;

use crate::raw::TurbosTickRaw;

/// Fetch all initialized ticks from a Turbos pool's tick table.
pub(crate) async fn fetch_turbos_ticks(
    client: &SuiClient,
    ticks_table_id: &ObjectId,
    pool_id: &ObjectId,
) -> Result<Vec<Tick>, ArbError> {
    let parent_id = object_id_to_hex(ticks_table_id);
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

            let tick_index = extract_tick_index_from_field(&page.data, &data.object_id);

            match deserialize_turbos_tick(&bcs_bytes, tick_index) {
                Ok(Some(tick)) => ticks.push(tick),
                Ok(None) => {}
                Err(e) => {
                    debug!(
                        "failed to deserialize Turbos tick for pool {}: {}",
                        object_id_to_hex(pool_id),
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

fn extract_tick_index_from_field(
    fields: &[sui_client::DynamicFieldInfo],
    object_id: &str,
) -> Option<i32> {
    for field in fields {
        if field.object_id == object_id {
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

    Ok(Some(Tick {
        index: tick_index.unwrap_or(0),
        liquidity_net: raw.liquidity_net.to_i128(),
        liquidity_gross: raw.liquidity_gross,
        sqrt_price: 0, // computed by clmm-math in Phase 2
    }))
}
