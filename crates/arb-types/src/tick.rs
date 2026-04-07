/// Normalized tick data, used for off-chain swap simulation.
#[derive(Debug, Clone)]
pub struct Tick {
    pub index: i32,
    /// Change in liquidity when this tick is crossed (signed).
    pub liquidity_net: i128,
    /// Total liquidity referencing this tick.
    pub liquidity_gross: u128,
    /// Precomputed Q64.64 sqrt price at this tick index.
    pub sqrt_price: u128,
}
