use ethnum::U256;

use crate::error::MathError;
use crate::math_u256::{
    checked_div_round_u128, checked_shlw, div_round_u256, full_mul_u128, mul_div_ceil_u64,
    mul_div_floor_u64, u256_to_u128, u256_to_u64,
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
) -> Result<SwapStepResult, MathError> {
    let mut next_sqrt_price = sqrt_price_target;
    let mut amount_in: u64 = 0;
    let mut amount_out: u64 = 0;
    let mut fee_amount: u64 = 0;

    if liquidity == 0 {
        return Ok(SwapStepResult {
            sqrt_price_next: next_sqrt_price,
            amount_in,
            amount_out,
            fee_amount,
        });
    }

    // If price is already past the target (stale state or rounding),
    // return zero — the caller will skip this step gracefully.
    if a_to_b {
        if sqrt_price_current < sqrt_price_target {
            return Ok(SwapStepResult {
                sqrt_price_next: sqrt_price_current,
                amount_in: 0,
                amount_out: 0,
                fee_amount: 0,
            });
        }
    } else if sqrt_price_current >= sqrt_price_target {
        return Ok(SwapStepResult {
            sqrt_price_next: sqrt_price_current,
            amount_in: 0,
            amount_out: 0,
            fee_amount: 0,
        });
    }

    if fee_rate >= FEE_RATE_DENOMINATOR {
        return Err(MathError::LiquidityOverflow("fee_rate exceeds denominator"));
    }

    if by_amount_in {
        // Deduct fee from input amount first
        let amount_remain =
            mul_div_floor_u64(amount, FEE_RATE_DENOMINATOR - fee_rate, FEE_RATE_DENOMINATOR);
        let max_amount_in =
            get_delta_up_from_input(sqrt_price_current, sqrt_price_target, liquidity, a_to_b)?;

        if max_amount_in > U256::from(amount_remain) {
            // Partial fill — doesn't reach target
            amount_in = amount_remain;
            fee_amount = amount - amount_remain;
            next_sqrt_price = get_next_sqrt_price_from_input(
                sqrt_price_current,
                liquidity,
                amount_remain,
                a_to_b,
            )?;
        } else {
            // Full fill — reaches target tick
            amount_in = u256_to_u64(max_amount_in, "compute_swap_step: amount_in")?;
            fee_amount = mul_div_ceil_u64(amount_in, fee_rate, FEE_RATE_DENOMINATOR - fee_rate);
            next_sqrt_price = sqrt_price_target;
        }

        amount_out = u256_to_u64(
            get_delta_down_from_output(sqrt_price_current, next_sqrt_price, liquidity, a_to_b)?,
            "compute_swap_step: amount_out",
        )?;
    } else {
        // by_amount_out
        let max_amount_out =
            get_delta_down_from_output(sqrt_price_current, sqrt_price_target, liquidity, a_to_b)?;

        if max_amount_out > U256::from(amount) {
            // Partial fill
            amount_out = amount;
            next_sqrt_price =
                get_next_sqrt_price_from_output(sqrt_price_current, liquidity, amount, a_to_b)?;
        } else {
            // Full fill
            amount_out = u256_to_u64(max_amount_out, "compute_swap_step: amount_out")?;
            next_sqrt_price = sqrt_price_target;
        }

        amount_in = u256_to_u64(
            get_delta_up_from_input(sqrt_price_current, next_sqrt_price, liquidity, a_to_b)?,
            "compute_swap_step: amount_in",
        )?;
        fee_amount = mul_div_ceil_u64(amount_in, fee_rate, FEE_RATE_DENOMINATOR - fee_rate);
    }

    Ok(SwapStepResult {
        sqrt_price_next: next_sqrt_price,
        amount_in,
        amount_out,
        fee_amount,
    })
}

/// Token A delta given sqrt price range and liquidity.
/// delta_a = L * |sqrt_P1 - sqrt_P0| * 2^64 / (sqrt_P0 * sqrt_P1)
pub fn get_amount_a_delta(
    sqrt_price_0: u128,
    sqrt_price_1: u128,
    liquidity: u128,
    round_up: bool,
) -> Result<u64, MathError> {
    let sqrt_price_diff = sqrt_price_0.abs_diff(sqrt_price_1);
    if sqrt_price_diff == 0 || liquidity == 0 {
        return Ok(0);
    }

    let (numerator, overflowing) = checked_shlw(full_mul_u128(liquidity, sqrt_price_diff));
    if overflowing {
        return Err(MathError::MulOverflow("get_amount_a_delta"));
    }

    let denominator = full_mul_u128(sqrt_price_0, sqrt_price_1);
    if denominator == U256::ZERO {
        return Err(MathError::DivisionByZero);
    }
    let quotient = div_round_u256(numerator, denominator, round_up);
    u256_to_u64(quotient, "get_amount_a_delta")
}

/// Token B delta given sqrt price range and liquidity.
/// delta_b = L * |sqrt_P1 - sqrt_P0| / 2^64
pub fn get_amount_b_delta(
    sqrt_price_0: u128,
    sqrt_price_1: u128,
    liquidity: u128,
    round_up: bool,
) -> Result<u64, MathError> {
    let sqrt_price_diff = sqrt_price_0.abs_diff(sqrt_price_1);
    if sqrt_price_diff == 0 || liquidity == 0 {
        return Ok(0);
    }

    let product = full_mul_u128(liquidity, sqrt_price_diff);
    let lo64_mask = U256::from(u64::MAX);
    let shifted = product >> 64u32;
    let should_round_up = round_up && (product & lo64_mask) > U256::ZERO;

    if should_round_up {
        u256_to_u64(shifted + U256::ONE, "get_amount_b_delta")
    } else {
        u256_to_u64(shifted, "get_amount_b_delta")
    }
}

/// Get next sqrt price when swapping token A (price moves down for a2b input, up for b2a output).
/// Uses: new_P = L * P * 2^64 / (L * 2^64 ± amount * P)
fn get_next_sqrt_price_a_up(
    sqrt_price: u128,
    liquidity: u128,
    amount: u64,
    by_amount_input: bool,
) -> Result<u128, MathError> {
    if amount == 0 {
        return Ok(sqrt_price);
    }

    let (numerator, overflowing) = checked_shlw(full_mul_u128(sqrt_price, liquidity));
    if overflowing {
        return Err(MathError::MulOverflow("get_next_sqrt_price_a_up"));
    }

    let liquidity_shl_64 = U256::from(liquidity) << 64;
    let product = full_mul_u128(sqrt_price, amount as u128);

    let denominator = if by_amount_input {
        liquidity_shl_64 + product
    } else {
        // Protect against underflow when amount * price >= liquidity << 64
        if liquidity_shl_64 < product {
            return Err(MathError::LiquidityOverflow(
                "get_next_sqrt_price_a_up: liquidity too small for output amount",
            ));
        }
        liquidity_shl_64 - product
    };

    if denominator == U256::ZERO {
        return Err(MathError::DivisionByZero);
    }

    let new_sqrt_price = u256_to_u128(
        div_round_u256(numerator, denominator, true),
        "get_next_sqrt_price_a_up",
    )?;

    if !(MIN_SQRT_PRICE..=MAX_SQRT_PRICE).contains(&new_sqrt_price) {
        return Err(MathError::SqrtPriceOutOfBounds(new_sqrt_price));
    }
    Ok(new_sqrt_price)
}

/// Get next sqrt price when swapping token B (price moves up for b2a input, down for a2b output).
/// Uses: new_P = P ± amount * 2^64 / L
fn get_next_sqrt_price_b_down(
    sqrt_price: u128,
    liquidity: u128,
    amount: u64,
    by_amount_input: bool,
) -> Result<u128, MathError> {
    if liquidity == 0 {
        return Err(MathError::DivisionByZero);
    }
    let delta_sqrt_price =
        checked_div_round_u128((amount as u128) << 64, liquidity, !by_amount_input);

    let new_sqrt_price = if by_amount_input {
        sqrt_price.checked_add(delta_sqrt_price).ok_or(
            MathError::LiquidityOverflow("get_next_sqrt_price_b_down: add overflow"),
        )?
    } else {
        sqrt_price.checked_sub(delta_sqrt_price).ok_or(
            MathError::LiquidityOverflow("get_next_sqrt_price_b_down: sub underflow"),
        )?
    };

    if !(MIN_SQRT_PRICE..=MAX_SQRT_PRICE).contains(&new_sqrt_price) {
        return Err(MathError::SqrtPriceOutOfBounds(new_sqrt_price));
    }
    Ok(new_sqrt_price)
}

/// Get next sqrt price from a given input amount.
pub fn get_next_sqrt_price_from_input(
    sqrt_price: u128,
    liquidity: u128,
    amount: u64,
    a_to_b: bool,
) -> Result<u128, MathError> {
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
) -> Result<u128, MathError> {
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
) -> Result<U256, MathError> {
    let sqrt_price_diff = current_sqrt_price.abs_diff(target_sqrt_price);
    if sqrt_price_diff == 0 || liquidity == 0 {
        return Ok(U256::ZERO);
    }

    if a_to_b {
        // delta_a = L * diff * 2^64 / (P_current * P_target), rounded up
        let (numerator, overflowing) = checked_shlw(full_mul_u128(liquidity, sqrt_price_diff));
        if overflowing {
            return Err(MathError::MulOverflow("get_delta_up_from_input"));
        }
        let denominator = full_mul_u128(current_sqrt_price, target_sqrt_price);
        if denominator == U256::ZERO {
            return Err(MathError::DivisionByZero);
        }
        Ok(div_round_u256(numerator, denominator, true))
    } else {
        // delta_b = L * diff / 2^64, rounded up
        let product = full_mul_u128(liquidity, sqrt_price_diff);
        let lo64_mask = U256::from(u64::MAX);
        let should_round_up = (product & lo64_mask) > U256::ZERO;
        if should_round_up {
            Ok((product >> 64) + U256::ONE)
        } else {
            Ok(product >> 64)
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
) -> Result<U256, MathError> {
    let sqrt_price_diff = current_sqrt_price.abs_diff(target_sqrt_price);
    if sqrt_price_diff == 0 || liquidity == 0 {
        return Ok(U256::ZERO);
    }

    if a_to_b {
        // delta_b = L * diff / 2^64, rounded down
        let product = full_mul_u128(liquidity, sqrt_price_diff);
        Ok(product >> 64)
    } else {
        // delta_a = L * diff * 2^64 / (P_current * P_target), rounded down
        let (numerator, overflowing) = checked_shlw(full_mul_u128(liquidity, sqrt_price_diff));
        if overflowing {
            return Err(MathError::MulOverflow("get_delta_down_from_output"));
        }
        let denominator = full_mul_u128(current_sqrt_price, target_sqrt_price);
        if denominator == U256::ZERO {
            return Err(MathError::DivisionByZero);
        }
        Ok(div_round_u256(numerator, denominator, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tick_math::tick_to_sqrt_price;

    #[test]
    fn test_get_amount_a_delta_basic() {
        let p0 = tick_to_sqrt_price(0).unwrap();
        let p1 = tick_to_sqrt_price(10).unwrap();
        let liquidity = 1_000_000_000u128;

        let delta = get_amount_a_delta(p0, p1, liquidity, true).unwrap();
        assert!(delta > 0);

        let delta_down = get_amount_a_delta(p0, p1, liquidity, false).unwrap();
        assert!(delta >= delta_down);
    }

    #[test]
    fn test_get_amount_b_delta_basic() {
        let p0 = tick_to_sqrt_price(0).unwrap();
        let p1 = tick_to_sqrt_price(10).unwrap();
        let liquidity = 1_000_000_000u128;

        let delta = get_amount_b_delta(p0, p1, liquidity, true).unwrap();
        assert!(delta > 0);

        let delta_down = get_amount_b_delta(p0, p1, liquidity, false).unwrap();
        assert!(delta >= delta_down);
    }

    #[test]
    fn test_get_amount_delta_zero_liquidity() {
        let p0 = tick_to_sqrt_price(0).unwrap();
        let p1 = tick_to_sqrt_price(10).unwrap();
        assert_eq!(get_amount_a_delta(p0, p1, 0, true).unwrap(), 0);
        assert_eq!(get_amount_b_delta(p0, p1, 0, true).unwrap(), 0);
    }

    #[test]
    fn test_get_amount_delta_same_price() {
        let p = tick_to_sqrt_price(100).unwrap();
        assert_eq!(get_amount_a_delta(p, p, 1_000_000, true).unwrap(), 0);
        assert_eq!(get_amount_b_delta(p, p, 1_000_000, true).unwrap(), 0);
    }

    #[test]
    fn test_compute_swap_step_zero_liquidity() {
        let result = compute_swap_step(
            tick_to_sqrt_price(0).unwrap(),
            tick_to_sqrt_price(-10).unwrap(),
            0,
            1_000_000,
            2500,
            true,
            true,
        )
        .unwrap();
        assert_eq!(result.amount_in, 0);
        assert_eq!(result.amount_out, 0);
        assert_eq!(result.fee_amount, 0);
    }

    #[test]
    fn test_compute_swap_step_partial_fill_a2b() {
        let current = tick_to_sqrt_price(0).unwrap();
        let target = tick_to_sqrt_price(-1000).unwrap();
        let liquidity = 10_000_000_000_000u128;

        let result = compute_swap_step(current, target, liquidity, 100, 2500, true, true).unwrap();
        assert!(result.amount_in > 0);
        assert!(result.amount_out > 0);
        assert!(result.fee_amount > 0);
        assert!(result.sqrt_price_next > target);
        assert!(result.sqrt_price_next < current);
    }

    #[test]
    fn test_compute_swap_step_full_fill_a2b() {
        let current = tick_to_sqrt_price(0).unwrap();
        let target = tick_to_sqrt_price(-10).unwrap();
        let liquidity = 1_000_000u128;

        let result = compute_swap_step(
            current,
            target,
            liquidity,
            u64::MAX / 2,
            2500,
            true,
            true,
        )
        .unwrap();
        assert_eq!(result.sqrt_price_next, target);
    }

    #[test]
    fn test_compute_swap_step_fee_calculation() {
        let current = tick_to_sqrt_price(0).unwrap();
        let target = tick_to_sqrt_price(-100).unwrap();
        let liquidity = 1_000_000_000_000u128;
        let fee_rate = 2500u64;

        let result =
            compute_swap_step(current, target, liquidity, 10_000, fee_rate, true, true).unwrap();

        assert!(result.amount_in + result.fee_amount <= 10_000);
    }

    #[test]
    fn test_compute_swap_step_b2a() {
        let current = tick_to_sqrt_price(0).unwrap();
        let target = tick_to_sqrt_price(100).unwrap();
        let liquidity = 1_000_000_000_000u128;

        let result =
            compute_swap_step(current, target, liquidity, 10_000, 2500, false, true).unwrap();
        assert!(result.amount_in > 0);
        assert!(result.amount_out > 0);
        assert!(result.sqrt_price_next > current);
    }

    #[test]
    fn test_compute_swap_step_by_amount_out() {
        let current = tick_to_sqrt_price(0).unwrap();
        let target = tick_to_sqrt_price(-100).unwrap();
        let liquidity = 1_000_000_000_000u128;

        let result =
            compute_swap_step(current, target, liquidity, 5_000, 2500, true, false).unwrap();
        assert!(result.amount_out <= 5_000);
        assert!(result.amount_in > 0);
        assert!(result.fee_amount > 0);
    }
}
