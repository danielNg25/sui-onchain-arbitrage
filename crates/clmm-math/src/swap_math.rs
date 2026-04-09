use ethnum::U256;

use crate::math_u256::{
    checked_div_round_u128, checked_shlw, div_round_u256, full_mul_u128, mul_div_ceil_u64,
    mul_div_floor_u64,
};
use crate::{FEE_RATE_DENOMINATOR, MAX_SQRT_PRICE, MIN_SQRT_PRICE};

/// Result of a single swap step within one tick range.
#[derive(Debug, Clone, Copy)]
pub struct SwapStepResult {
    pub sqrt_price_next: u128,
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_amount: u64,
}

/// Compute one swap step within a single tick range.
///
/// Ported from CetusProtocol/cetus-clmm-interface clmm_math.move `compute_swap_step`.
///
/// `by_amount_in`: true = exact input, false = exact output.
pub fn compute_swap_step(
    sqrt_price_current: u128,
    sqrt_price_target: u128,
    liquidity: u128,
    amount: u64,
    fee_rate: u64,
    a_to_b: bool,
    by_amount_in: bool,
) -> SwapStepResult {
    let mut next_sqrt_price = sqrt_price_target;
    let mut amount_in: u64 = 0;
    let mut amount_out: u64 = 0;
    let mut fee_amount: u64 = 0;

    if liquidity == 0 {
        return SwapStepResult {
            sqrt_price_next: next_sqrt_price,
            amount_in,
            amount_out,
            fee_amount,
        };
    }

    if a_to_b {
        assert!(sqrt_price_current >= sqrt_price_target);
    } else {
        assert!(sqrt_price_current < sqrt_price_target);
    }

    if by_amount_in {
        // Deduct fee from input amount first
        let amount_remain =
            mul_div_floor_u64(amount, FEE_RATE_DENOMINATOR - fee_rate, FEE_RATE_DENOMINATOR);
        let max_amount_in =
            get_delta_up_from_input(sqrt_price_current, sqrt_price_target, liquidity, a_to_b);

        if max_amount_in > U256::from(amount_remain) {
            // Partial fill — doesn't reach target
            amount_in = amount_remain;
            fee_amount = amount - amount_remain;
            next_sqrt_price =
                get_next_sqrt_price_from_input(sqrt_price_current, liquidity, amount_remain, a_to_b);
        } else {
            // Full fill — reaches target tick
            amount_in = max_amount_in.as_u64();
            fee_amount = mul_div_ceil_u64(
                amount_in,
                fee_rate,
                FEE_RATE_DENOMINATOR - fee_rate,
            );
            next_sqrt_price = sqrt_price_target;
        }

        amount_out = get_delta_down_from_output(
            sqrt_price_current,
            next_sqrt_price,
            liquidity,
            a_to_b,
        )
        .as_u64();
    } else {
        // by_amount_out
        let max_amount_out = get_delta_down_from_output(
            sqrt_price_current,
            sqrt_price_target,
            liquidity,
            a_to_b,
        );

        if max_amount_out > U256::from(amount) {
            // Partial fill
            amount_out = amount;
            next_sqrt_price =
                get_next_sqrt_price_from_output(sqrt_price_current, liquidity, amount, a_to_b);
        } else {
            // Full fill
            amount_out = max_amount_out.as_u64();
            next_sqrt_price = sqrt_price_target;
        }

        amount_in = get_delta_up_from_input(
            sqrt_price_current,
            next_sqrt_price,
            liquidity,
            a_to_b,
        )
        .as_u64();
        fee_amount = mul_div_ceil_u64(
            amount_in,
            fee_rate,
            FEE_RATE_DENOMINATOR - fee_rate,
        );
    }

    SwapStepResult {
        sqrt_price_next: next_sqrt_price,
        amount_in,
        amount_out,
        fee_amount,
    }
}

/// Token A delta given sqrt price range and liquidity.
/// delta_a = L * |sqrt_P1 - sqrt_P0| * 2^64 / (sqrt_P0 * sqrt_P1)
pub fn get_amount_a_delta(
    sqrt_price_0: u128,
    sqrt_price_1: u128,
    liquidity: u128,
    round_up: bool,
) -> u64 {
    let sqrt_price_diff = sqrt_price_0.abs_diff(sqrt_price_1);
    if sqrt_price_diff == 0 || liquidity == 0 {
        return 0;
    }

    let (numerator, overflowing) = checked_shlw(full_mul_u128(liquidity, sqrt_price_diff));
    assert!(!overflowing, "mul overflow in get_amount_a_delta");

    let denominator = full_mul_u128(sqrt_price_0, sqrt_price_1);
    let quotient = div_round_u256(numerator, denominator, round_up);
    quotient.as_u64()
}

/// Token B delta given sqrt price range and liquidity.
/// delta_b = L * |sqrt_P1 - sqrt_P0| / 2^64
pub fn get_amount_b_delta(
    sqrt_price_0: u128,
    sqrt_price_1: u128,
    liquidity: u128,
    round_up: bool,
) -> u64 {
    let sqrt_price_diff = sqrt_price_0.abs_diff(sqrt_price_1);
    if sqrt_price_diff == 0 || liquidity == 0 {
        return 0;
    }

    let product = full_mul_u128(liquidity, sqrt_price_diff);
    let lo64_mask = U256::from(u64::MAX);
    let shifted = product >> 64u32;
    let should_round_up = round_up && (product & lo64_mask) > U256::ZERO;

    if should_round_up {
        (shifted + U256::ONE).as_u64()
    } else {
        shifted.as_u64()
    }
}

/// Get next sqrt price when swapping token A (price moves down for a2b input, up for b2a output).
/// Uses: new_P = L * P * 2^64 / (L * 2^64 ± amount * P)
fn get_next_sqrt_price_a_up(
    sqrt_price: u128,
    liquidity: u128,
    amount: u64,
    by_amount_input: bool,
) -> u128 {
    if amount == 0 {
        return sqrt_price;
    }

    let (numerator, overflowing) = checked_shlw(full_mul_u128(sqrt_price, liquidity));
    assert!(!overflowing, "mul overflow in get_next_sqrt_price_a_up");

    let liquidity_shl_64 = U256::from(liquidity) << 64;
    let product = full_mul_u128(sqrt_price, amount as u128);

    let new_sqrt_price = if by_amount_input {
        div_round_u256(numerator, liquidity_shl_64 + product, true).as_u128()
    } else {
        div_round_u256(numerator, liquidity_shl_64 - product, true).as_u128()
    };

    assert!(
        (MIN_SQRT_PRICE..=MAX_SQRT_PRICE).contains(&new_sqrt_price),
        "sqrt price out of bounds after get_next_sqrt_price_a_up"
    );
    new_sqrt_price
}

/// Get next sqrt price when swapping token B (price moves up for b2a input, down for a2b output).
/// Uses: new_P = P ± amount * 2^64 / L
fn get_next_sqrt_price_b_down(
    sqrt_price: u128,
    liquidity: u128,
    amount: u64,
    by_amount_input: bool,
) -> u128 {
    let delta_sqrt_price =
        checked_div_round_u128((amount as u128) << 64, liquidity, !by_amount_input);

    let new_sqrt_price = if by_amount_input {
        sqrt_price + delta_sqrt_price
    } else {
        sqrt_price - delta_sqrt_price
    };

    assert!(
        (MIN_SQRT_PRICE..=MAX_SQRT_PRICE).contains(&new_sqrt_price),
        "sqrt price out of bounds after get_next_sqrt_price_b_down"
    );
    new_sqrt_price
}

/// Get next sqrt price from a given input amount.
pub fn get_next_sqrt_price_from_input(
    sqrt_price: u128,
    liquidity: u128,
    amount: u64,
    a_to_b: bool,
) -> u128 {
    if a_to_b {
        get_next_sqrt_price_a_up(sqrt_price, liquidity, amount, true)
    } else {
        get_next_sqrt_price_b_down(sqrt_price, liquidity, amount, true)
    }
}

/// Get next sqrt price from a desired output amount.
pub fn get_next_sqrt_price_from_output(
    sqrt_price: u128,
    liquidity: u128,
    amount: u64,
    a_to_b: bool,
) -> u128 {
    if a_to_b {
        get_next_sqrt_price_b_down(sqrt_price, liquidity, amount, false)
    } else {
        get_next_sqrt_price_a_up(sqrt_price, liquidity, amount, false)
    }
}

/// Max input amount to move from current to target sqrt price (rounded up).
/// For a2b: delta_a (token A in). For b2a: delta_b (token B in).
fn get_delta_up_from_input(
    current_sqrt_price: u128,
    target_sqrt_price: u128,
    liquidity: u128,
    a_to_b: bool,
) -> U256 {
    let sqrt_price_diff = current_sqrt_price.abs_diff(target_sqrt_price);
    if sqrt_price_diff == 0 || liquidity == 0 {
        return U256::ZERO;
    }

    if a_to_b {
        // delta_a = L * diff * 2^64 / (P_current * P_target), rounded up
        let (numerator, overflowing) = checked_shlw(full_mul_u128(liquidity, sqrt_price_diff));
        assert!(!overflowing, "mul overflow in get_delta_up_from_input");
        let denominator = full_mul_u128(current_sqrt_price, target_sqrt_price);
        div_round_u256(numerator, denominator, true)
    } else {
        // delta_b = L * diff / 2^64, rounded up
        let product = full_mul_u128(liquidity, sqrt_price_diff);
        let lo64_mask = U256::from(u64::MAX);
        let should_round_up = (product & lo64_mask) > U256::ZERO;
        if should_round_up {
            (product >> 64) + U256::ONE
        } else {
            product >> 64
        }
    }
}

/// Max output amount when moving from current to target sqrt price (rounded down).
/// For a2b: delta_b (token B out). For b2a: delta_a (token A out).
fn get_delta_down_from_output(
    current_sqrt_price: u128,
    target_sqrt_price: u128,
    liquidity: u128,
    a_to_b: bool,
) -> U256 {
    let sqrt_price_diff = current_sqrt_price.abs_diff(target_sqrt_price);
    if sqrt_price_diff == 0 || liquidity == 0 {
        return U256::ZERO;
    }

    if a_to_b {
        // delta_b = L * diff / 2^64, rounded down
        let product = full_mul_u128(liquidity, sqrt_price_diff);
        product >> 64
    } else {
        // delta_a = L * diff * 2^64 / (P_current * P_target), rounded down
        let (numerator, overflowing) = checked_shlw(full_mul_u128(liquidity, sqrt_price_diff));
        assert!(!overflowing, "mul overflow in get_delta_down_from_output");
        let denominator = full_mul_u128(current_sqrt_price, target_sqrt_price);
        div_round_u256(numerator, denominator, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tick_math::tick_to_sqrt_price;

    #[test]
    fn test_get_amount_a_delta_basic() {
        let p0 = tick_to_sqrt_price(0); // 1 << 64
        let p1 = tick_to_sqrt_price(10);
        let liquidity = 1_000_000_000u128; // 1B

        let delta = get_amount_a_delta(p0, p1, liquidity, true);
        assert!(delta > 0);

        // round_up should be >= round_down
        let delta_down = get_amount_a_delta(p0, p1, liquidity, false);
        assert!(delta >= delta_down);
    }

    #[test]
    fn test_get_amount_b_delta_basic() {
        let p0 = tick_to_sqrt_price(0);
        let p1 = tick_to_sqrt_price(10);
        let liquidity = 1_000_000_000u128;

        let delta = get_amount_b_delta(p0, p1, liquidity, true);
        assert!(delta > 0);

        let delta_down = get_amount_b_delta(p0, p1, liquidity, false);
        assert!(delta >= delta_down);
    }

    #[test]
    fn test_get_amount_delta_zero_liquidity() {
        let p0 = tick_to_sqrt_price(0);
        let p1 = tick_to_sqrt_price(10);
        assert_eq!(get_amount_a_delta(p0, p1, 0, true), 0);
        assert_eq!(get_amount_b_delta(p0, p1, 0, true), 0);
    }

    #[test]
    fn test_get_amount_delta_same_price() {
        let p = tick_to_sqrt_price(100);
        assert_eq!(get_amount_a_delta(p, p, 1_000_000, true), 0);
        assert_eq!(get_amount_b_delta(p, p, 1_000_000, true), 0);
    }

    #[test]
    fn test_compute_swap_step_zero_liquidity() {
        let result = compute_swap_step(
            tick_to_sqrt_price(0),
            tick_to_sqrt_price(-10),
            0,
            1_000_000,
            2500,
            true,
            true,
        );
        assert_eq!(result.amount_in, 0);
        assert_eq!(result.amount_out, 0);
        assert_eq!(result.fee_amount, 0);
    }

    #[test]
    fn test_compute_swap_step_partial_fill_a2b() {
        // Small swap that doesn't reach target tick
        let current = tick_to_sqrt_price(0);
        let target = tick_to_sqrt_price(-1000);
        let liquidity = 10_000_000_000_000u128; // large liquidity

        let result = compute_swap_step(current, target, liquidity, 100, 2500, true, true);
        assert!(result.amount_in > 0);
        assert!(result.amount_out > 0);
        assert!(result.fee_amount > 0);
        // Didn't reach target
        assert!(result.sqrt_price_next > target);
        assert!(result.sqrt_price_next < current);
    }

    #[test]
    fn test_compute_swap_step_full_fill_a2b() {
        // Large swap that fills the entire tick range
        let current = tick_to_sqrt_price(0);
        let target = tick_to_sqrt_price(-10);
        let liquidity = 1_000_000u128; // small liquidity

        let result = compute_swap_step(
            current,
            target,
            liquidity,
            u64::MAX / 2,
            2500,
            true,
            true,
        );
        // Should reach target
        assert_eq!(result.sqrt_price_next, target);
    }

    #[test]
    fn test_compute_swap_step_fee_calculation() {
        let current = tick_to_sqrt_price(0);
        let target = tick_to_sqrt_price(-100);
        let liquidity = 1_000_000_000_000u128;
        let fee_rate = 2500u64; // 0.25%

        let result = compute_swap_step(current, target, liquidity, 10_000, fee_rate, true, true);

        // fee_rate = 2500/1_000_000 = 0.25%
        // amount_in + fee_amount should approximately equal the input
        assert!(result.amount_in + result.fee_amount <= 10_000);
    }

    #[test]
    fn test_compute_swap_step_b2a() {
        let current = tick_to_sqrt_price(0);
        let target = tick_to_sqrt_price(100);
        let liquidity = 1_000_000_000_000u128;

        let result = compute_swap_step(current, target, liquidity, 10_000, 2500, false, true);
        assert!(result.amount_in > 0);
        assert!(result.amount_out > 0);
        assert!(result.sqrt_price_next > current);
    }

    #[test]
    fn test_compute_swap_step_by_amount_out() {
        let current = tick_to_sqrt_price(0);
        let target = tick_to_sqrt_price(-100);
        let liquidity = 1_000_000_000_000u128;

        let result = compute_swap_step(current, target, liquidity, 5_000, 2500, true, false);
        assert!(result.amount_out <= 5_000);
        assert!(result.amount_in > 0);
        assert!(result.fee_amount > 0);
    }
}
