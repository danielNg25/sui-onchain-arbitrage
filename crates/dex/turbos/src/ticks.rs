use arb_types::error::ArbError;
use arb_types::pool::{object_id_to_hex, ObjectId};
use arb_types::tick::Tick;
use sui_client::{ObjectDataOptions, SuiClient};
use tracing::debug;

/// Fetch all initialized ticks from a Turbos pool.
///
/// Turbos stores individual tick data as dynamic fields on the **pool object**,
/// keyed by `I32` (tick index). These are mixed with `Position` entries
/// (keyed by `String`), so we filter by key type containing "::i32::I32".
///
/// We paginate pool dynamic fields, collect I32-keyed object IDs, then
/// batch-fetch their BCS data with `multi_get_objects` to minimize RPC calls.
pub(crate) async fn fetch_turbos_ticks(
    client: &SuiClient,
    pool_id: &ObjectId,
    _ticks_table_id: &ObjectId,
) -> Result<Vec<Tick>, ArbError> {
    let pool_id_hex = object_id_to_hex(pool_id);

    // Step 1: Paginate pool dynamic fields, collect I32-keyed entries (ticks)
    let mut tick_entries: Vec<(String, i32)> = Vec::new(); // (object_id, tick_index)
    let mut cursor: Option<String> = None;

    loop {
        let page = client
            .get_dynamic_fields(&pool_id_hex, cursor, Some(50))
            .await
            .map_err(|e| ArbError::Rpc(format!("fetch Turbos pool fields: {}", e)))?;

        for field in &page.data {
            if field.name.type_.contains("::i32::I32") {
                let tick_index = field
                    .name
                    .value
                    .get("bits")
                    .and_then(|v| v.as_u64())
                    .map(|bits| bits as u32 as i32)
                    .unwrap_or(0);
                tick_entries.push((field.object_id.clone(), tick_index));
            }
        }

        if !page.has_next_page {
            break;
        }
        cursor = page.next_cursor;
    }

    debug!(
        count = tick_entries.len(),
        pool = pool_id_hex,
        "found Turbos tick dynamic fields"
    );

    if tick_entries.is_empty() {
        return Ok(Vec::new());
    }

    // Step 2: Batch-fetch tick objects with multi_get_objects
    let mut ticks = Vec::new();
    for chunk in tick_entries.chunks(50) {
        let obj_ids: Vec<String> = chunk.iter().map(|(id, _)| id.clone()).collect();
        let objects = client
            .multi_get_objects(&obj_ids, ObjectDataOptions::bcs())
            .await
            .map_err(|e| ArbError::Rpc(format!("batch fetch Turbos ticks: {}", e)))?;

        for (obj_resp, (_, tick_index)) in objects.iter().zip(chunk.iter()) {
            let Some(data) = &obj_resp.data else { continue };
            let bcs_bytes = match data.bcs_bytes() {
                Ok(b) => b,
                Err(e) => {
                    debug!("skip Turbos tick {}: {}", tick_index, e);
                    continue;
                }
            };

            match deserialize_turbos_tick(&bcs_bytes, *tick_index) {
                Ok(Some(tick)) => ticks.push(tick),
                Ok(None) => {}
                Err(e) => {
                    debug!("failed to deser Turbos tick {}: {}", tick_index, e);
                }
            }
        }
    }

    ticks.sort_by_key(|t| t.index);
    debug!(count = ticks.len(), pool = pool_id_hex, "fetched Turbos ticks");
    Ok(ticks)
}

/// Deserialize a Turbos tick from BCS bytes.
///
/// Dynamic field: Field { id: UID(32), name: I32{bits:u32}(4), value: Tick }
/// Tick: { id: UID(32), initialized: bool(1), liquidity_gross: u128(16),
///         liquidity_net: I128{bits:u128}(16), ... }
fn deserialize_turbos_tick(bcs_bytes: &[u8], tick_index: i32) -> Result<Option<Tick>, ArbError> {
    // Field wrapper: id(32) + name I32(4) = 36 bytes
    // Tick: id(32) + initialized(1) + liquidity_gross(16) + liquidity_net(16) = 65 bytes minimum
    if bcs_bytes.len() < 36 + 33 {
        return Err(ArbError::BcsDeserialize("Turbos tick too short".into()));
    }

    let tick_data = &bcs_bytes[36..];
    let mut pos = 32; // skip Tick.id (UID)

    let initialized = tick_data[pos] != 0;
    pos += 1;

    if !initialized {
        return Ok(None);
    }

    if pos + 32 > tick_data.len() {
        return Err(ArbError::BcsDeserialize("Turbos tick: missing liquidity".into()));
    }

    let liquidity_gross = u128::from_le_bytes(tick_data[pos..pos + 16].try_into().unwrap());
    pos += 16;

    let liquidity_net_bits = u128::from_le_bytes(tick_data[pos..pos + 16].try_into().unwrap());
    let liquidity_net = liquidity_net_bits as i128;

    let sqrt_price = clmm_math::tick_to_sqrt_price(tick_index)
        .map_err(|e| ArbError::InvalidData(format!("Turbos tick sqrt_price: {}", e)))?;

    Ok(Some(Tick {
        index: tick_index,
        liquidity_net,
        liquidity_gross,
        sqrt_price,
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_i128_negative_conversion() {
        let bits: u128 = 340282366920938463463374607305674605003;
        let val = bits as i128;
        assert!(val < 0);
    }
}
