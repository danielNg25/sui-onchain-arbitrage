use arb_types::pool::{CoinType, ObjectId};

use crate::cycle::RotatedCycle;

/// A detected arbitrage opportunity, ready for execution.
#[derive(Debug, Clone)]
pub struct Opportunity {
    /// The cycle to execute (rotated so profit token is start/end).
    pub cycle: RotatedCycle,
    /// Optimal input amount in profit token base units.
    pub amount_in: u64,
    /// Expected output amount.
    pub amount_out: u64,
    /// Expected profit in profit token base units.
    pub profit: u64,
    /// Expected profit in USD.
    pub profit_usd: f64,
    /// The profit token (same as cycle.cycle.profit_token()).
    pub profit_token: CoinType,
    /// The pool that triggered this opportunity (swap event source).
    pub trigger_pool_id: ObjectId,
    /// Timestamp when this opportunity was detected (ms since epoch).
    pub detected_at_ms: u64,
}
