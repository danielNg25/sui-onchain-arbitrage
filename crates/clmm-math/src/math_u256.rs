use ethnum::U256;

/// Multiply two u128 values and return a U256 result.
#[inline(always)]
pub fn full_mul_u128(a: u128, b: u128) -> U256 {
    U256::from(a) * U256::from(b)
}

/// (a * b) / denom, rounded down.
#[inline(always)]
pub fn mul_div_floor_u128(a: u128, b: u128, denom: u128) -> u128 {
    let r = full_mul_u128(a, b) / U256::from(denom);
    r.as_u128()
}

/// (a * b) / denom, rounded up.
#[inline(always)]
pub fn mul_div_ceil_u128(a: u128, b: u128, denom: u128) -> u128 {
    let r = (full_mul_u128(a, b) + U256::from(denom - 1)) / U256::from(denom);
    r.as_u128()
}

/// (a * b) >> shift
#[inline(always)]
pub fn mul_shr_u128(a: u128, b: u128, shift: u8) -> u128 {
    let product = full_mul_u128(a, b) >> shift;
    product.as_u128()
}

/// (a * b) / denom, rounded down (u64 version).
#[inline(always)]
pub fn mul_div_floor_u64(a: u64, b: u64, denom: u64) -> u64 {
    let r = (a as u128) * (b as u128) / (denom as u128);
    r as u64
}

/// (a * b) / denom, rounded up (u64 version).
#[inline(always)]
pub fn mul_div_ceil_u64(a: u64, b: u64, denom: u64) -> u64 {
    let r = ((a as u128) * (b as u128)).div_ceil(denom as u128);
    r as u64
}

/// Left-shift a U256 by 64 bits, returning (result, overflowed).
/// Overflow means the value >= 2^192 before shifting.
#[inline(always)]
pub fn checked_shlw(val: U256) -> (U256, bool) {
    let overflowing = val >= (U256::ONE << 192);
    (val << 64, overflowing)
}

/// num / denom, with optional round_up.
#[inline(always)]
pub fn div_round_u256(num: U256, denom: U256, round_up: bool) -> U256 {
    let quotient = num / denom;
    if round_up && (quotient * denom != num) {
        quotient + 1
    } else {
        quotient
    }
}

/// num / denom, with optional round_up (u128 version).
#[inline(always)]
pub fn checked_div_round_u128(num: u128, denom: u128, round_up: bool) -> u128 {
    let quotient = num / denom;
    let remainder = num % denom;
    if round_up && remainder > 0 {
        quotient + 1
    } else {
        quotient
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_mul() {
        assert_eq!(full_mul_u128(0, 100), U256::ZERO);
        assert_eq!(full_mul_u128(1, 1), U256::ONE);
        let max = u128::MAX;
        let result = full_mul_u128(max, max);
        assert!(result > U256::from(max));
    }

    #[test]
    fn test_mul_div_floor() {
        assert_eq!(mul_div_floor_u128(10, 20, 3), 66);
        assert_eq!(mul_div_floor_u128(7, 3, 2), 10);
    }

    #[test]
    fn test_mul_div_ceil() {
        assert_eq!(mul_div_ceil_u128(10, 20, 3), 67);
        assert_eq!(mul_div_ceil_u128(7, 3, 2), 11);
    }

    #[test]
    fn test_checked_shlw() {
        let (result, overflow) = checked_shlw(U256::ONE);
        assert_eq!(result, U256::from(1u128 << 64));
        assert!(!overflow);

        let big = U256::ONE << 192;
        let (_, overflow) = checked_shlw(big);
        assert!(overflow);
    }

    #[test]
    fn test_div_round() {
        assert_eq!(div_round_u256(U256::from(10u64), U256::from(3u64), false), U256::from(3u64));
        assert_eq!(div_round_u256(U256::from(10u64), U256::from(3u64), true), U256::from(4u64));
        assert_eq!(div_round_u256(U256::from(9u64), U256::from(3u64), true), U256::from(3u64));
    }
}
