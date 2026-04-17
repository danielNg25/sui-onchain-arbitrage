use arb_types::tick::Tick;

use crate::error::MathError;
use crate::swap_math::compute_swap_step;
use crate::tick_math::tick_to_sqrt_price;
use crate::{MAX_SQRT_PRICE, MIN_SQRT_PRICE};

/// Result of a full multi-tick swap simulation.
#[derive(Debug, Clone)]
pub struct SwapResult {
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_total: u64,
    pub sqrt_price_after: u128,
    pub tick_after: i32,
    pub liquidity_after: u128,
    pub steps: u32,
    pub is_exceed: bool,
}

/// Apply a signed liquidity delta, checking for over/underflow.
fn apply_liquidity_delta(liquidity: u128, net: i128) -> Result<u128, MathError> {
    if net >= 0 {
        liquidity
            .checked_add(net as u128)
            .ok_or(MathError::LiquidityOverflow("liquidity add"))
    } else {
        let abs = net.unsigned_abs();
        liquidity
            .checked_sub(abs)
            .ok_or(MathError::LiquidityOverflow("liquidity sub"))
    }
}

/// Simulate a full swap across multiple ticks.
///
/// `ticks` must be sorted by index ascending with sqrt_price populated.
/// `a_to_b`: true = sell token A (price decreases), false = sell token B (price increases).
/// `amount`: input amount to swap (by_amount_in = true).
#[allow(clippy::too_many_arguments)]
pub fn simulate_swap(
    sqrt_price: u128,
    tick_current: i32,
    liquidity: u128,
    fee_rate: u64,
    _tick_spacing: u32,
    ticks: &[Tick],
    a_to_b: bool,
    amount: u64,
) -> Result<SwapResult, MathError> {
    let mut current_sqrt_price = sqrt_price;
    let mut current_liquidity = liquidity;
    let mut amount_remaining = amount;
    let mut total_amount_in: u64 = 0;
    let mut total_amount_out: u64 = 0;
    let mut total_fee: u64 = 0;
    let mut steps: u32 = 0;
    let mut current_tick = tick_current;

    let sqrt_price_limit = if a_to_b {
        MIN_SQRT_PRICE
    } else {
        MAX_SQRT_PRICE
    };

    while amount_remaining > 0 && current_sqrt_price != sqrt_price_limit {
        let next_tick = find_next_initialized_tick(ticks, current_tick, a_to_b);

        let (_next_tick_index, next_tick_sqrt_price) = match next_tick {
            Some(tick) => {
                let sp = if tick.sqrt_price != 0 {
                    tick.sqrt_price
                } else {
                    tick_to_sqrt_price(tick.index)?
                };
                (tick.index, sp)
            }
            None => {
                if a_to_b {
                    (crate::MIN_TICK, MIN_SQRT_PRICE)
                } else {
                    (crate::MAX_TICK, MAX_SQRT_PRICE)
                }
            }
        };

        let target_sqrt_price = if a_to_b {
            next_tick_sqrt_price.max(sqrt_price_limit)
        } else {
            next_tick_sqrt_price.min(sqrt_price_limit)
        };

        let step = compute_swap_step(
            current_sqrt_price,
            target_sqrt_price,
            current_liquidity,
            amount_remaining,
            fee_rate,
            a_to_b,
            true,
        )?;

        total_amount_in = total_amount_in.saturating_add(step.amount_in);
        total_amount_out = total_amount_out.saturating_add(step.amount_out);
        total_fee = total_fee.saturating_add(step.fee_amount);
        amount_remaining = amount_remaining
            .saturating_sub(step.amount_in)
            .saturating_sub(step.fee_amount);

        // If the step made no progress (stale state / zero-output), stop to avoid infinite loop.
        if step.amount_in == 0 && step.amount_out == 0 && step.sqrt_price_next == current_sqrt_price {
            break;
        }

        current_sqrt_price = step.sqrt_price_next;

        if let Some(tick) = next_tick.filter(|_| step.sqrt_price_next == target_sqrt_price) {
            if a_to_b {
                current_liquidity = apply_liquidity_delta(current_liquidity, -tick.liquidity_net)?;
                current_tick = tick.index - 1;
            } else {
                current_liquidity = apply_liquidity_delta(current_liquidity, tick.liquidity_net)?;
                current_tick = tick.index;
            }
            steps += 1;
        } else {
            current_tick = crate::tick_math::sqrt_price_to_tick(current_sqrt_price)?;
        }
    }

    let is_exceed = amount_remaining > 0;

    Ok(SwapResult {
        amount_in: total_amount_in,
        amount_out: total_amount_out,
        fee_total: total_fee,
        sqrt_price_after: current_sqrt_price,
        tick_after: current_tick,
        liquidity_after: current_liquidity,
        steps,
        is_exceed,
    })
}

/// Find the next initialized tick in direction of travel.
/// For a_to_b: find the largest tick index <= current_tick (price going down).
/// For b_to_a: find the smallest tick index > current_tick (price going up).
fn find_next_initialized_tick(ticks: &[Tick], current_tick: i32, a_to_b: bool) -> Option<&Tick> {
    if ticks.is_empty() {
        return None;
    }

    if a_to_b {
        let pos = ticks.partition_point(|t| t.index <= current_tick);
        if pos > 0 {
            Some(&ticks[pos - 1])
        } else {
            None
        }
    } else {
        let pos = ticks.partition_point(|t| t.index <= current_tick);
        if pos < ticks.len() {
            Some(&ticks[pos])
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tick_math::tick_to_sqrt_price;

    fn make_simple_ticks() -> Vec<Tick> {
        vec![
            Tick {
                index: -200,
                liquidity_net: 1_000_000_000_000,
                liquidity_gross: 1_000_000_000_000,
                sqrt_price: tick_to_sqrt_price(-200).unwrap(),
            },
            Tick {
                index: 200,
                liquidity_net: -1_000_000_000_000,
                liquidity_gross: 1_000_000_000_000,
                sqrt_price: tick_to_sqrt_price(200).unwrap(),
            },
        ]
    }

    #[test]
    fn test_simulate_single_tick_a2b() {
        let ticks = make_simple_ticks();
        let result = simulate_swap(
            tick_to_sqrt_price(0).unwrap(),
            0,
            1_000_000_000_000,
            2500,
            60,
            &ticks,
            true,
            1_000,
        )
        .unwrap();

        assert!(result.amount_in > 0);
        assert!(result.amount_out > 0);
        assert!(result.fee_total > 0);
        assert!(!result.is_exceed);
        assert!(result.sqrt_price_after < tick_to_sqrt_price(0).unwrap());
    }

    #[test]
    fn test_simulate_single_tick_b2a() {
        let ticks = make_simple_ticks();
        let result = simulate_swap(
            tick_to_sqrt_price(0).unwrap(),
            0,
            1_000_000_000_000,
            2500,
            60,
            &ticks,
            false,
            1_000,
        )
        .unwrap();

        assert!(result.amount_in > 0);
        assert!(result.amount_out > 0);
        assert!(result.sqrt_price_after > tick_to_sqrt_price(0).unwrap());
    }

    #[test]
    fn test_simulate_multi_tick_crossing() {
        let ticks = vec![
            Tick {
                index: -600,
                liquidity_net: 1_000_000,
                liquidity_gross: 1_000_000,
                sqrt_price: tick_to_sqrt_price(-600).unwrap(),
            },
            Tick {
                index: -200,
                liquidity_net: 1_000_000,
                liquidity_gross: 1_000_000,
                sqrt_price: tick_to_sqrt_price(-200).unwrap(),
            },
            Tick {
                index: 200,
                liquidity_net: -1_000_000,
                liquidity_gross: 1_000_000,
                sqrt_price: tick_to_sqrt_price(200).unwrap(),
            },
            Tick {
                index: 600,
                liquidity_net: -1_000_000,
                liquidity_gross: 1_000_000,
                sqrt_price: tick_to_sqrt_price(600).unwrap(),
            },
        ];

        let result = simulate_swap(
            tick_to_sqrt_price(0).unwrap(),
            0,
            2_000_000,
            2500,
            60,
            &ticks,
            true,
            1_000_000_000,
        )
        .unwrap();

        assert!(result.amount_out > 0);
        assert!(result.steps >= 1);
    }

    #[test]
    fn test_simulate_exhausts_liquidity() {
        let ticks = vec![
            Tick {
                index: -200,
                liquidity_net: 100,
                liquidity_gross: 100,
                sqrt_price: tick_to_sqrt_price(-200).unwrap(),
            },
            Tick {
                index: 200,
                liquidity_net: -100,
                liquidity_gross: 100,
                sqrt_price: tick_to_sqrt_price(200).unwrap(),
            },
        ];

        let result = simulate_swap(
            tick_to_sqrt_price(0).unwrap(),
            0,
            100,
            2500,
            60,
            &ticks,
            true,
            u64::MAX / 2,
        )
        .unwrap();

        assert!(result.is_exceed);
    }

    #[test]
    fn test_simulate_zero_amount() {
        let ticks = make_simple_ticks();
        let result = simulate_swap(
            tick_to_sqrt_price(0).unwrap(),
            0,
            1_000_000_000_000,
            2500,
            60,
            &ticks,
            true,
            0,
        )
        .unwrap();

        assert_eq!(result.amount_in, 0);
        assert_eq!(result.amount_out, 0);
        assert_eq!(result.fee_total, 0);
    }

    #[test]
    fn test_find_next_tick_a2b() {
        let ticks = make_simple_ticks();
        let next = find_next_initialized_tick(&ticks, 0, true);
        assert_eq!(next.unwrap().index, -200);
    }

    #[test]
    fn test_find_next_tick_b2a() {
        let ticks = make_simple_ticks();
        let next = find_next_initialized_tick(&ticks, 0, false);
        assert_eq!(next.unwrap().index, 200);
    }

    #[test]
    fn test_find_next_tick_none() {
        let ticks = make_simple_ticks();
        assert!(find_next_initialized_tick(&ticks, -201, true).is_none());
        assert!(find_next_initialized_tick(&ticks, 200, false).is_none());
    }
}
