use std::collections::HashMap;
use std::sync::Arc;

use arb_types::event::SwapEventData;
use sui_client::{EventCursor, EventFilter, SuiClient};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::PoolManager;

/// Configuration for the event collector.
#[derive(Debug, Clone)]
pub struct CollectorConfig {
    /// All event type strings to poll (swap + liquidity from all DEXes).
    pub event_types: Vec<String>,
    /// Maximum events per query.
    pub batch_size: u32,
    /// Polling interval in milliseconds.
    pub poll_interval_ms: u64,
}

/// A function that tries to parse a swap event from raw event JSON.
/// Returns None for non-swap event types (e.g. liquidity events).
/// The caller provides this so the collector stays DEX-agnostic.
pub type SwapEventParser =
    Arc<dyn Fn(&str, &serde_json::Value) -> Option<SwapEventData> + Send + Sync>;

/// Handle for controlling a running collector.
pub struct CollectorHandle {
    cancel_tx: Option<tokio::sync::oneshot::Sender<()>>,
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl CollectorHandle {
    /// Stop the collector gracefully.
    pub async fn stop(&mut self) {
        if let Some(tx) = self.cancel_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.task_handle.take() {
            let _ = handle.await;
        }
    }
}

/// Start the event collector service.
///
/// The collector polls ALL configured event types (swap + liquidity) and:
/// 1. Applies every event to pool state via `pool_manager.apply_event()` (zero RPC)
/// 2. For swap events, parses them via `swap_parser` and sends through `swap_event_tx`
///
/// Returns a [`CollectorHandle`] for lifecycle control.
pub fn start_collector(
    client: Arc<SuiClient>,
    pool_manager: Arc<PoolManager>,
    config: CollectorConfig,
    swap_parser: SwapEventParser,
    swap_event_tx: mpsc::Sender<SwapEventData>,
) -> CollectorHandle {
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();

    let task_handle = tokio::spawn(collector_loop(
        client,
        pool_manager,
        config,
        swap_parser,
        swap_event_tx,
        cancel_rx,
    ));

    CollectorHandle {
        cancel_tx: Some(cancel_tx),
        task_handle: Some(task_handle),
    }
}

async fn collector_loop(
    client: Arc<SuiClient>,
    pool_manager: Arc<PoolManager>,
    config: CollectorConfig,
    swap_parser: SwapEventParser,
    swap_event_tx: mpsc::Sender<SwapEventData>,
    mut cancel_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let poll_interval = std::time::Duration::from_millis(config.poll_interval_ms);

    // Bootstrap cursors: skip to latest event for each type so we only
    // process NEW events, not replay the entire on-chain history.
    let mut cursors: HashMap<String, Option<EventCursor>> = HashMap::new();
    for event_type in &config.event_types {
        match client
            .query_events(
                EventFilter::MoveEventType(event_type.clone()),
                None,
                Some(1),
                true, // descending — get the most recent event
            )
            .await
        {
            Ok(page) => {
                if let Some(event) = page.data.first() {
                    cursors.insert(event_type.clone(), Some(event.id.clone()));
                    debug!(event_type = %event_type, "bootstrapped cursor to latest event");
                } else {
                    cursors.insert(event_type.clone(), None);
                }
            }
            Err(e) => {
                warn!(event_type = %event_type, error = %e, "failed to bootstrap cursor, starting from beginning");
                cursors.insert(event_type.clone(), None);
            }
        }
    }

    let mut total_events_applied = 0u64;
    let mut total_swaps_sent = 0u64;

    info!(
        event_types = config.event_types.len(),
        poll_interval_ms = config.poll_interval_ms,
        "collector started"
    );

    loop {
        // Check for cancellation before polling
        if cancel_rx.try_recv().is_ok() {
            info!("collector received stop signal");
            break;
        }

        let poll_start = std::time::Instant::now();
        let mut events_this_round = 0u32;
        let mut swaps_this_round = 0u32;

        for event_type in &config.event_types {
            let cursor = cursors.get(event_type).cloned().flatten();

            let page = match client
                .query_events(
                    EventFilter::MoveEventType(event_type.clone()),
                    cursor,
                    Some(config.batch_size),
                    false, // ascending — oldest first for chronological processing
                )
                .await
            {
                Ok(page) => page,
                Err(e) => {
                    warn!(
                        event_type = %event_type,
                        error = %e,
                        "failed to query events"
                    );
                    continue;
                }
            };

            if !page.data.is_empty() {
                debug!(
                    event_type = %event_type,
                    count = page.data.len(),
                    "received events"
                );
            }

            for event in &page.data {
                let json = match &event.parsed_json {
                    Some(j) => j,
                    None => continue,
                };

                // 1. Apply to pool state (ALL events — swap + liquidity)
                //    Pure local computation, zero RPC.
                match pool_manager.apply_event(&event.type_, json) {
                    Ok(Some(_pool_id)) => {
                        events_this_round += 1;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        debug!(
                            event_type = %event.type_,
                            error = %e,
                            "failed to apply event"
                        );
                    }
                }

                // 2. If swap event, parse and send to consumer
                if let Some(swap_data) = swap_parser(&event.type_, json) {
                    swaps_this_round += 1;
                    if swap_event_tx.send(swap_data).await.is_err() {
                        info!("swap event channel closed, stopping collector");
                        return;
                    }
                }
            }

            // Update cursor for next poll
            if let Some(next_cursor) = page.next_cursor {
                cursors.insert(event_type.clone(), Some(next_cursor));
            } else if let Some(last) = page.data.last() {
                cursors.insert(event_type.clone(), Some(last.id.clone()));
            }
        }

        total_events_applied += events_this_round as u64;
        total_swaps_sent += swaps_this_round as u64;

        if events_this_round > 0 {
            info!(
                events = events_this_round,
                swaps = swaps_this_round,
                total_events = total_events_applied,
                total_swaps = total_swaps_sent,
                elapsed_ms = poll_start.elapsed().as_millis(),
                "poll cycle complete"
            );
        }

        tokio::select! {
            _ = tokio::time::sleep(poll_interval) => {}
            _ = &mut cancel_rx => {
                info!("collector received stop signal");
                break;
            }
        }
    }

    info!(
        total_events = total_events_applied,
        total_swaps = total_swaps_sent,
        "collector stopped"
    );
}
