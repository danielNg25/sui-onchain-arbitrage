use arb_types::error::ArbError;
use arb_types::pool::{object_id_to_hex, ObjectId};
use arb_types::tick::Tick;
use sui_client::{ObjectDataOptions, SuiClient};
use tracing::debug;

/// Fetch all initialized tick indices from a Turbos pool's tick_map bitmap.
///
/// Turbos stores tick initialization as a `Table<I32, u256>` bitmap.
/// Each entry maps a word index (I32) to a 256-bit bitmap where each bit
/// represents whether a tick at that position is initialized.
///
/// Tick data (liquidity_net, liquidity_gross) is not directly available from
/// the bitmap — it requires devInspect calls or position reconstruction.
/// For Phase 1, we return ticks with index only (liquidity fields zero).
/// Phase 2 (clmm-math) will populate these via devInspect.
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
            .map_err(|e| ArbError::Rpc(format!("fetch Turbos tick_map fields: {}", e)))?;

        if page.data.is_empty() {
            break;
        }

        // Each dynamic field is a bitmap word: I32 → u256
        // Fetch the actual u256 values
        let obj_ids: Vec<String> = page.data.iter().map(|f| f.object_id.clone()).collect();
        let objects = client
            .multi_get_objects(&obj_ids, ObjectDataOptions::bcs())
            .await
            .map_err(|e| ArbError::Rpc(format!("batch fetch Turbos bitmap entries: {}", e)))?;

        for (field_info, obj_resp) in page.data.iter().zip(objects.iter()) {
            let Some(data) = &obj_resp.data else { continue };
            let bcs_bytes = match data.bcs_bytes() {
                Ok(b) => b,
                Err(e) => {
                    debug!("skip Turbos bitmap entry: {}", e);
                    continue;
                }
            };

            // Extract word index from dynamic field name
            let word_index: i32 = field_info
                .name
                .value
                .get("bits")
                .and_then(|v| v.as_u64())
                .map(|bits| bits as u32 as i32)
                .unwrap_or(0);

            // BCS bytes: Field { id: [u8;32], name: I32 { bits: u32 }, value: u256 }
            // Skip 32 (id) + 4 (I32.bits) = 36 bytes, then read 32 bytes (u256 LE)
            if bcs_bytes.len() < 68 {
                debug!("Turbos bitmap entry too short: {} bytes", bcs_bytes.len());
                continue;
            }

            let mut bitmap_bytes = [0u8; 32];
            bitmap_bytes.copy_from_slice(&bcs_bytes[36..68]);

            // Decode initialized tick indices from bitmap
            let initialized = decode_bitmap(word_index, &bitmap_bytes);
            for tick_index in initialized {
                ticks.push(Tick {
                    index: tick_index,
                    liquidity_net: 0,   // populated via devInspect in Phase 2
                    liquidity_gross: 0, // populated via devInspect in Phase 2
                    sqrt_price: 0,      // computed by clmm-math in Phase 2
                });
            }
        }

        if !page.has_next_page {
            break;
        }
        cursor = page.next_cursor;
    }

    ticks.sort_by_key(|t| t.index);
    debug!(
        count = ticks.len(),
        pool = object_id_to_hex(pool_id),
        "fetched Turbos initialized tick indices"
    );
    Ok(ticks)
}

/// Decode a 256-bit bitmap word into initialized tick indices.
///
/// Each bit i in the bitmap represents tick at index:
///   `word_index * 256 + i`
/// where word_index is the I32 key from the Table.
fn decode_bitmap(word_index: i32, bitmap: &[u8; 32]) -> Vec<i32> {
    let mut indices = Vec::new();
    let base = (word_index as i64) * 256;

    for (byte_idx, &byte) in bitmap.iter().enumerate() {
        if byte == 0 {
            continue;
        }
        for bit in 0..8 {
            if byte & (1 << bit) != 0 {
                let tick_index = base + (byte_idx as i64) * 8 + (bit as i64);
                indices.push(tick_index as i32);
            }
        }
    }
    indices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_bitmap_empty() {
        let bitmap = [0u8; 32];
        assert!(decode_bitmap(0, &bitmap).is_empty());
    }

    #[test]
    fn test_decode_bitmap_single_bit() {
        let mut bitmap = [0u8; 32];
        bitmap[0] = 1; // bit 0 set
        let indices = decode_bitmap(0, &bitmap);
        assert_eq!(indices, vec![0]);
    }

    #[test]
    fn test_decode_bitmap_word_index() {
        let mut bitmap = [0u8; 32];
        bitmap[0] = 1; // bit 0 set
        let indices = decode_bitmap(5, &bitmap);
        assert_eq!(indices, vec![5 * 256]);
    }

    #[test]
    fn test_decode_bitmap_negative_word() {
        let mut bitmap = [0u8; 32];
        bitmap[0] = 1;
        let indices = decode_bitmap(-1, &bitmap);
        assert_eq!(indices, vec![-256]);
    }

    #[test]
    fn test_decode_bitmap_multiple_bits() {
        let mut bitmap = [0u8; 32];
        bitmap[0] = 0b00000101; // bits 0 and 2
        bitmap[1] = 0b00000010; // bit 9 (byte 1, bit 1)
        let indices = decode_bitmap(0, &bitmap);
        assert_eq!(indices, vec![0, 2, 9]);
    }
}
