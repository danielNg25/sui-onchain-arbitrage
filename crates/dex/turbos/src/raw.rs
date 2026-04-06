// BCS raw structs must define all fields in exact Move struct order,
// even if we only read a subset. Suppress dead_code warnings.
#![allow(dead_code)]

use serde::Deserialize;

use arb_types::error::ArbError;

/// Partially parsed Turbos pool — only the fields we need.
pub(crate) struct TurbosPoolPartial {
    pub coin_a: u64,
    pub coin_b: u64,
    pub sqrt_price: u128,
    pub tick_current_index: i32,
    pub tick_spacing: u32,
    pub fee: u32,
    pub unlocked: bool,
    pub liquidity: u128,
    pub tick_map_id: [u8; 32],
}

/// Parse Turbos pool BCS bytes manually.
///
/// Turbos is closed source. Layout validated against real mainnet BCS snapshot.
/// Key difference from naive serde approach: tick_map is Table { id: UID, size: u64 },
/// not just a 32-byte UID.
pub(crate) fn parse_turbos_pool(bytes: &[u8]) -> Result<TurbosPoolPartial, ArbError> {
    let mut r = BcsReader::new(bytes);

    let _id = r.read_bytes(32)?;              // UID
    let coin_a = r.read_u64()?;               // Balance<CoinA>
    let coin_b = r.read_u64()?;               // Balance<CoinB>
    let _protocol_fees_a = r.read_u64()?;
    let _protocol_fees_b = r.read_u64()?;
    let sqrt_price = r.read_u128()?;           // Q64.64
    let tick_bits = r.read_u32()?;             // I32 { bits: u32 }
    let tick_spacing = r.read_u32()?;
    let _max_liquidity_per_tick = r.read_u128()?;
    let fee = r.read_u32()?;
    let _fee_protocol = r.read_u32()?;
    let unlocked = r.read_bool()?;
    let _fee_growth_global_a = r.read_u128()?;
    let _fee_growth_global_b = r.read_u128()?;
    let liquidity = r.read_u128()?;

    // tick_map: Table<I32, u256> = { id: UID (32 bytes), size: u64 (8 bytes) }
    let tick_map_id = r.read_object_id()?;
    let _tick_map_size = r.read_u64()?;

    // Remaining fields (deploy_time_ms, reward_infos, reward_last_updated_time_ms)
    // are not needed — stop here.

    Ok(TurbosPoolPartial {
        coin_a,
        coin_b,
        sqrt_price,
        tick_current_index: tick_bits as i32,
        tick_spacing,
        fee,
        unlocked,
        liquidity,
        tick_map_id,
    })
}

/// Turbos tick data — individual dynamic field on the pool.
#[derive(Deserialize, Debug)]
pub(crate) struct TurbosTickRaw {
    pub initialized: bool,
    pub liquidity_gross: u128,
    pub liquidity_net: TurbosI128,
    pub fee_growth_outside_a: u128,
    pub fee_growth_outside_b: u128,
    pub reward_growths_outside: Vec<u128>,
    pub tick_cumulative_outside: TurbosI64,
    pub seconds_per_liquidity_outside: u128,
    pub seconds_outside: u32,
}

#[derive(Deserialize, Debug)]
pub(crate) struct TurbosI128 {
    pub bits: u128,
}

impl TurbosI128 {
    pub fn to_i128(&self) -> i128 {
        self.bits as i128
    }
}

#[derive(Deserialize, Debug)]
pub(crate) struct TurbosI64 {
    pub bits: u64,
}

// --- Manual BCS reader (same as Cetus) ---

struct BcsReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BcsReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn ensure(&self, n: usize) -> Result<(), ArbError> {
        if self.remaining() < n {
            Err(ArbError::BcsDeserialize(format!(
                "need {} bytes at offset {}, only {} remaining",
                n, self.pos, self.remaining()
            )))
        } else {
            Ok(())
        }
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], ArbError> {
        self.ensure(n)?;
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn read_object_id(&mut self) -> Result<[u8; 32], ArbError> {
        let bytes = self.read_bytes(32)?;
        let mut id = [0u8; 32];
        id.copy_from_slice(bytes);
        Ok(id)
    }

    fn read_bool(&mut self) -> Result<bool, ArbError> {
        self.ensure(1)?;
        let v = self.data[self.pos] != 0;
        self.pos += 1;
        Ok(v)
    }

    fn read_u32(&mut self) -> Result<u32, ArbError> {
        self.ensure(4)?;
        let v = u32::from_le_bytes(self.data[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }

    fn read_u64(&mut self) -> Result<u64, ArbError> {
        self.ensure(8)?;
        let v = u64::from_le_bytes(self.data[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        Ok(v)
    }

    fn read_u128(&mut self) -> Result<u128, ArbError> {
        self.ensure(16)?;
        let v = u128::from_le_bytes(self.data[self.pos..self.pos + 16].try_into().unwrap());
        self.pos += 16;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_i32_positive() {
        assert_eq!(100u32 as i32, 100);
    }

    #[test]
    fn test_i32_negative() {
        assert_eq!((-100i32) as u32 as i32, -100);
    }
}
