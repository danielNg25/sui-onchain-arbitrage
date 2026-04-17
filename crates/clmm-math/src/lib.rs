pub mod error;
pub mod math_u256;
pub mod simulate;
pub mod swap_math;
pub mod tick_math;

pub use error::MathError;
pub use simulate::{simulate_swap, SwapResult};
pub use swap_math::{compute_swap_step, SwapStepResult};
pub use tick_math::{sqrt_price_to_tick, tick_to_sqrt_price};

/// Fee rate denominator (1,000,000 = 100%). Fee rate is in PPM.
pub const FEE_RATE_DENOMINATOR: u64 = 1_000_000;

/// Minimum valid sqrt price (Q64.64), corresponds to MIN_TICK.
pub const MIN_SQRT_PRICE: u128 = 4_295_048_016;

/// Maximum valid sqrt price (Q64.64), corresponds to MAX_TICK.
pub const MAX_SQRT_PRICE: u128 = 79_226_673_515_401_279_992_447_579_055;

/// Minimum valid tick index.
pub const MIN_TICK: i32 = -443_636;

/// Maximum valid tick index.
pub const MAX_TICK: i32 = 443_636;
