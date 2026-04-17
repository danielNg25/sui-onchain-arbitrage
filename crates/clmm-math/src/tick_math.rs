use crate::error::MathError;
use crate::math_u256::mul_shr_u128;
use crate::{MAX_SQRT_PRICE, MAX_TICK, MIN_SQRT_PRICE, MIN_TICK};

/// Convert tick index to Q64.64 sqrt price.
///
/// Ported from CetusProtocol/cetus-clmm-interface tick_math.move.
/// Uses binary exponentiation with precomputed ratio constants.
pub fn tick_to_sqrt_price(tick: i32) -> Result<u128, MathError> {
    if !(MIN_TICK..=MAX_TICK).contains(&tick) {
        return Err(MathError::TickOutOfBounds(tick));
    }
    if tick < 0 {
        Ok(sqrt_price_at_negative_tick(tick))
    } else {
        Ok(sqrt_price_at_positive_tick(tick))
    }
}

/// Convert Q64.64 sqrt price to tick index (floor).
///
/// Ported from CetusProtocol/cetus-clmm-interface tick_math.move.
/// Uses MSB detection + log2 approximation with 14 bits of precision.
pub fn sqrt_price_to_tick(sqrt_price: u128) -> Result<i32, MathError> {
    if !(MIN_SQRT_PRICE..=MAX_SQRT_PRICE).contains(&sqrt_price) {
        return Err(MathError::SqrtPriceOutOfBounds(sqrt_price));
    }

    // Find MSB (most significant bit)
    let mut r = sqrt_price;
    let mut msb: u8 = 0;

    let f = if r >= 0x1_0000_0000_0000_0000 { 64u8 } else { 0u8 };
    msb |= f;
    r >>= f;

    let f = if r >= 0x1_0000_0000 { 32u8 } else { 0u8 };
    msb |= f;
    r >>= f;

    let f = if r >= 0x1_0000 { 16u8 } else { 0u8 };
    msb |= f;
    r >>= f;

    let f = if r >= 0x100 { 8u8 } else { 0u8 };
    msb |= f;
    r >>= f;

    let f = if r >= 0x10 { 4u8 } else { 0u8 };
    msb |= f;
    r >>= f;

    let f = if r >= 0x4 { 2u8 } else { 0u8 };
    msb |= f;
    r >>= f;

    let f = if r >= 0x2 { 1u8 } else { 0u8 };
    msb |= f;

    // log_2_x32 = (msb - 64) << 32  (as signed i128)
    let log_2_x32: i128 = ((msb as i128) - 64) << 32;

    // Normalize r to have msb at bit 63
    r = if msb >= 64 {
        sqrt_price >> (msb - 63)
    } else {
        sqrt_price << (63 - msb)
    };

    // Refine: 14 iterations (shift 31 down to 18)
    let mut log_2_x32 = log_2_x32;
    let mut shift: i32 = 31;
    while shift >= 18 {
        r = (r * r) >> 63;
        let f = (r >> 64) as i128;
        log_2_x32 |= f << shift;
        r >>= f as u128;
        shift -= 1;
    }

    // log_sqrt_10001 = log_2_x32 * 59543866431366
    let log_sqrt_10001: i128 = log_2_x32 * 59_543_866_431_366i128;

    // tick_low and tick_high
    let tick_low = ((log_sqrt_10001 - 184_467_440_737_095_516i128) >> 64) as i32;
    let tick_high = ((log_sqrt_10001 + 15_793_534_762_490_258_745i128) >> 64) as i32;

    if tick_low == tick_high {
        Ok(tick_low)
    } else if tick_to_sqrt_price(tick_high)? <= sqrt_price {
        Ok(tick_high)
    } else {
        Ok(tick_low)
    }
}

/// Compute sqrt price for negative ticks using Q64.64 ratio constants.
fn sqrt_price_at_negative_tick(tick: i32) -> u128 {
    let abs_tick = tick.unsigned_abs();

    let mut ratio: u128 = if abs_tick & 0x1 != 0 {
        18_445_821_805_675_392_311
    } else {
        18_446_744_073_709_551_616 // 1 << 64
    };

    if abs_tick & 0x2 != 0 {
        ratio = mul_shr_u128(ratio, 18_444_899_583_751_176_498, 64);
    }
    if abs_tick & 0x4 != 0 {
        ratio = mul_shr_u128(ratio, 18_443_055_278_223_354_162, 64);
    }
    if abs_tick & 0x8 != 0 {
        ratio = mul_shr_u128(ratio, 18_439_367_220_385_604_838, 64);
    }
    if abs_tick & 0x10 != 0 {
        ratio = mul_shr_u128(ratio, 18_431_993_317_065_449_817, 64);
    }
    if abs_tick & 0x20 != 0 {
        ratio = mul_shr_u128(ratio, 18_417_254_355_718_160_513, 64);
    }
    if abs_tick & 0x40 != 0 {
        ratio = mul_shr_u128(ratio, 18_387_811_781_193_591_352, 64);
    }
    if abs_tick & 0x80 != 0 {
        ratio = mul_shr_u128(ratio, 18_329_067_761_203_520_168, 64);
    }
    if abs_tick & 0x100 != 0 {
        ratio = mul_shr_u128(ratio, 18_212_142_134_806_087_854, 64);
    }
    if abs_tick & 0x200 != 0 {
        ratio = mul_shr_u128(ratio, 17_980_523_815_641_551_639, 64);
    }
    if abs_tick & 0x400 != 0 {
        ratio = mul_shr_u128(ratio, 17_526_086_738_831_147_013, 64);
    }
    if abs_tick & 0x800 != 0 {
        ratio = mul_shr_u128(ratio, 16_651_378_430_235_024_244, 64);
    }
    if abs_tick & 0x1000 != 0 {
        ratio = mul_shr_u128(ratio, 15_030_750_278_693_429_944, 64);
    }
    if abs_tick & 0x2000 != 0 {
        ratio = mul_shr_u128(ratio, 12_247_334_978_882_834_399, 64);
    }
    if abs_tick & 0x4000 != 0 {
        ratio = mul_shr_u128(ratio, 8_131_365_268_884_726_200, 64);
    }
    if abs_tick & 0x8000 != 0 {
        ratio = mul_shr_u128(ratio, 3_584_323_654_723_342_297, 64);
    }
    if abs_tick & 0x10000 != 0 {
        ratio = mul_shr_u128(ratio, 696_457_651_847_595_233, 64);
    }
    if abs_tick & 0x20000 != 0 {
        ratio = mul_shr_u128(ratio, 26_294_789_957_452_057, 64);
    }
    if abs_tick & 0x40000 != 0 {
        ratio = mul_shr_u128(ratio, 37_481_735_321_082, 64);
    }

    ratio
}

/// Compute sqrt price for positive ticks using Q96.96 ratio constants,
/// then right-shift by 32 to get Q64.64.
fn sqrt_price_at_positive_tick(tick: i32) -> u128 {
    let abs_tick = tick as u32;

    let mut ratio: u128 = if abs_tick & 0x1 != 0 {
        79_232_123_823_359_799_118_286_999_567
    } else {
        79_228_162_514_264_337_593_543_950_336 // 1 << 96
    };

    if abs_tick & 0x2 != 0 {
        ratio = mul_shr_u128(ratio, 79_236_085_330_515_764_027_303_304_731, 96);
    }
    if abs_tick & 0x4 != 0 {
        ratio = mul_shr_u128(ratio, 79_244_008_939_048_815_603_706_035_061, 96);
    }
    if abs_tick & 0x8 != 0 {
        ratio = mul_shr_u128(ratio, 79_259_858_533_276_714_757_314_932_305, 96);
    }
    if abs_tick & 0x10 != 0 {
        ratio = mul_shr_u128(ratio, 79_291_567_232_598_584_799_939_703_904, 96);
    }
    if abs_tick & 0x20 != 0 {
        ratio = mul_shr_u128(ratio, 79_355_022_692_464_371_645_785_046_466, 96);
    }
    if abs_tick & 0x40 != 0 {
        ratio = mul_shr_u128(ratio, 79_482_085_999_252_804_386_437_311_141, 96);
    }
    if abs_tick & 0x80 != 0 {
        ratio = mul_shr_u128(ratio, 79_736_823_300_114_093_921_829_183_326, 96);
    }
    if abs_tick & 0x100 != 0 {
        ratio = mul_shr_u128(ratio, 80_248_749_790_819_932_309_965_073_892, 96);
    }
    if abs_tick & 0x200 != 0 {
        ratio = mul_shr_u128(ratio, 81_282_483_887_344_747_381_513_967_011, 96);
    }
    if abs_tick & 0x400 != 0 {
        ratio = mul_shr_u128(ratio, 83_390_072_131_320_151_908_154_831_281, 96);
    }
    if abs_tick & 0x800 != 0 {
        ratio = mul_shr_u128(ratio, 87_770_609_709_833_776_024_991_924_138, 96);
    }
    if abs_tick & 0x1000 != 0 {
        ratio = mul_shr_u128(ratio, 97_234_110_755_111_693_312_479_820_773, 96);
    }
    if abs_tick & 0x2000 != 0 {
        ratio = mul_shr_u128(ratio, 119_332_217_159_966_728_226_237_229_890, 96);
    }
    if abs_tick & 0x4000 != 0 {
        ratio = mul_shr_u128(ratio, 179_736_315_981_702_064_433_883_588_727, 96);
    }
    if abs_tick & 0x8000 != 0 {
        ratio = mul_shr_u128(ratio, 407_748_233_172_238_350_107_850_275_304, 96);
    }
    if abs_tick & 0x10000 != 0 {
        ratio = mul_shr_u128(ratio, 2_098_478_828_474_011_932_436_660_412_517, 96);
    }
    if abs_tick & 0x20000 != 0 {
        ratio = mul_shr_u128(ratio, 55_581_415_166_113_811_149_459_800_483_533, 96);
    }
    if abs_tick & 0x40000 != 0 {
        ratio = mul_shr_u128(ratio, 38_992_368_544_603_139_932_233_054_999_993_551, 96);
    }

    ratio >> 32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tick_0() {
        // tick 0 = price 1.0 = 2^64 in Q64.64
        let sqrt_price = tick_to_sqrt_price(0).unwrap();
        assert_eq!(sqrt_price, 1u128 << 64);
    }

    #[test]
    fn test_min_tick() {
        assert_eq!(tick_to_sqrt_price(MIN_TICK).unwrap(), MIN_SQRT_PRICE);
    }

    #[test]
    fn test_max_tick() {
        assert_eq!(tick_to_sqrt_price(MAX_TICK).unwrap(), MAX_SQRT_PRICE);
    }

    #[test]
    fn test_known_negative_tick() {
        assert_eq!(tick_to_sqrt_price(-435_444).unwrap(), 6_469_134_034);
    }

    #[test]
    fn test_known_positive_tick() {
        assert_eq!(
            tick_to_sqrt_price(408_332).unwrap(),
            13_561_044_167_458_152_057_771_544_136
        );
    }

    #[test]
    fn test_round_trip_zero() {
        assert_eq!(sqrt_price_to_tick(tick_to_sqrt_price(0).unwrap()).unwrap(), 0);
    }

    #[test]
    fn test_round_trip_positive() {
        for t in [1, 10, 100, 1000, 10000, 100000, 443636] {
            let sp = tick_to_sqrt_price(t).unwrap();
            let recovered = sqrt_price_to_tick(sp).unwrap();
            assert_eq!(recovered, t, "round-trip failed for tick {t}");
        }
    }

    #[test]
    fn test_round_trip_negative() {
        for t in [-1, -10, -100, -1000, -10000, -100000, -443636] {
            let sp = tick_to_sqrt_price(t).unwrap();
            let recovered = sqrt_price_to_tick(sp).unwrap();
            assert_eq!(recovered, t, "round-trip failed for tick {t}");
        }
    }

    #[test]
    fn test_sqrt_price_to_tick_known() {
        assert_eq!(sqrt_price_to_tick(6_469_134_034).unwrap(), -435_444);
        assert_eq!(
            sqrt_price_to_tick(13_561_044_167_458_152_057_771_544_136).unwrap(),
            408_332
        );
    }

    #[test]
    fn test_tick_spacing_multiples() {
        for spacing in [2, 10, 20, 60, 200] {
            let tick = spacing * 100;
            let sp = tick_to_sqrt_price(tick).unwrap();
            let recovered = sqrt_price_to_tick(sp).unwrap();
            assert_eq!(recovered, tick);
        }
    }

    #[test]
    fn test_tick_out_of_bounds_high() {
        assert!(matches!(
            tick_to_sqrt_price(MAX_TICK + 1),
            Err(MathError::TickOutOfBounds(_))
        ));
    }

    #[test]
    fn test_tick_out_of_bounds_low() {
        assert!(matches!(
            tick_to_sqrt_price(MIN_TICK - 1),
            Err(MathError::TickOutOfBounds(_))
        ));
    }

    #[test]
    fn test_sqrt_price_out_of_bounds_high() {
        assert!(matches!(
            sqrt_price_to_tick(MAX_SQRT_PRICE + 1),
            Err(MathError::SqrtPriceOutOfBounds(_))
        ));
    }

    #[test]
    fn test_sqrt_price_out_of_bounds_low() {
        assert!(matches!(
            sqrt_price_to_tick(MIN_SQRT_PRICE - 1),
            Err(MathError::SqrtPriceOutOfBounds(_))
        ));
    }
}
