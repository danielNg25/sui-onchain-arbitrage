use crate::pool::{Dex, ObjectId};

/// Parsed swap event data, shared across DEX implementations.
#[derive(Debug, Clone)]
pub struct SwapEventData {
    pub pool_id: ObjectId,
    pub dex: Dex,
    pub a_to_b: bool,
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_amount: u64,
    pub after_sqrt_price: u128,
    /// Pool's coin_a balance after the swap.
    pub vault_a_amount: u64,
    /// Pool's coin_b balance after the swap.
    pub vault_b_amount: u64,
    /// Number of tick crossings. If > 1, ticks need refresh.
    pub steps: u64,
}
