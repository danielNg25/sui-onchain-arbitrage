use arb_types::error::ArbError;
use arb_types::pool::{object_id_to_hex, PoolState};
use arb_types::tick::Tick;
use sui_client::{ObjectDataOptions, SuiClient};
use tracing::debug;

use crate::raw::CetusSkipListNodeRaw;

pub struct CetusTickFetcher;

#[async_trait::async_trait]
impl dex_common::TickFetcher for CetusTickFetcher {
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
                .map_err(|e| ArbError::Rpc(format!("fetch tick fields: {}", e)))?;

            if page.data.is_empty() {
                break;
            }

            // Batch-fetch the dynamic field objects to get BCS data
            let obj_ids: Vec<String> = page.data.iter().map(|f| f.object_id.clone()).collect();
            let objects = client
                .multi_get_objects(&obj_ids, ObjectDataOptions::bcs())
                .await
                .map_err(|e| ArbError::Rpc(format!("batch fetch tick objects: {}", e)))?;

            for obj_resp in &objects {
                let Some(data) = &obj_resp.data else {
                    continue;
                };
                let bcs_bytes = data.bcs_bytes().map_err(|e| {
                    ArbError::BcsDeserialize(format!("tick BCS bytes: {}", e))
                })?;

                match deserialize_tick_node(&bcs_bytes) {
                    Ok(tick) => ticks.push(tick),
                    Err(e) => {
                        debug!(
                            "failed to deserialize Cetus tick node for pool {}: {}",
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

/// Deserialize a Cetus SkipList node containing tick data from BCS bytes.
///
/// The dynamic field value for SkipList entries wraps the node in a
/// `Field<u64, SkipListNode<Tick>>` structure. The outer Field adds:
/// - id: [u8; 32]  (UID of the dynamic field object)
/// - name: u64      (the score key)
/// - value: SkipListNode<Tick>
fn deserialize_tick_node(bcs_bytes: &[u8]) -> Result<Tick, ArbError> {
    // Dynamic field wrapper: Field { id: UID, name: T, value: V }
    // For SkipList entries: Field { id: [u8;32], name: u64, value: SkipListNode<Tick> }
    // We need to skip the Field wrapper to get the SkipListNode.
    //
    // id: 32 bytes
    // name (u64): 8 bytes
    // value: SkipListNode<Tick>
    if bcs_bytes.len() < 40 {
        return Err(ArbError::BcsDeserialize(
            "tick BCS bytes too short for Field wrapper".into(),
        ));
    }

    let node: CetusSkipListNodeRaw = bcs::from_bytes(&bcs_bytes[40..]).map_err(|e| {
        ArbError::BcsDeserialize(format!("Cetus SkipListNode deser: {}", e))
    })?;

    Ok(Tick {
        index: node.value.index.to_i32(),
        liquidity_net: node.value.liquidity_net.to_i128(),
        liquidity_gross: node.value.liquidity_gross,
        sqrt_price: node.value.sqrt_price,
    })
}
