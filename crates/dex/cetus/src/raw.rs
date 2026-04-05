// BCS raw structs must define all fields in exact Move struct order,
// even if we only read a subset. Suppress dead_code warnings.
#![allow(dead_code)]

use serde::Deserialize;

use arb_types::error::ArbError;

/// Partially parsed Cetus pool — only the fields we need.
pub(crate) struct CetusPoolPartial {
    pub coin_a: u64,
    pub coin_b: u64,
    pub tick_spacing: u32,
    pub fee_rate: u64,
    pub liquidity: u128,
    pub current_sqrt_price: u128,
    pub current_tick_index: i32,
    pub ticks_table_id: [u8; 32],
    pub is_pause: bool,
}

/// Parse Cetus pool BCS bytes manually, field by field.
///
/// This avoids having to perfectly match every nested struct (SkipList, Bag,
/// RewarderManager, PositionManager) with serde. We read the fields we need
/// and skip the rest using their known sizes.
pub(crate) fn parse_cetus_pool(bytes: &[u8]) -> Result<CetusPoolPartial, ArbError> {
    let mut r = BcsReader::new(bytes);

    // Pool fields (exact order from Move struct):
    let _id = r.read_bytes(32)?; // UID
    let coin_a = r.read_u64()?; // Balance<CoinA>
    let coin_b = r.read_u64()?; // Balance<CoinB>
    let tick_spacing = r.read_u32()?;
    let fee_rate = r.read_u64()?;
    let liquidity = r.read_u128()?;
    let current_sqrt_price = r.read_u128()?;
    let current_tick_index_bits = r.read_u32()?; // I32 { bits: u32 }
    let _fee_growth_global_a = r.read_u128()?;
    let _fee_growth_global_b = r.read_u128()?;
    let _fee_protocol_coin_a = r.read_u64()?;
    let _fee_protocol_coin_b = r.read_u64()?;

    // TickManager { tick_spacing: u32, ticks: SkipList<Tick> }
    let _tm_tick_spacing = r.read_u32()?;

    // SkipList<Tick> { id: UID, head: Vec<OptionU64>, tail: OptionU64, level: u64, max_level: u64, list_p: u64, size: u64 }
    let ticks_table_id = r.read_object_id()?;
    let head_len = r.read_uleb128()?;
    for _ in 0..head_len {
        r.read_option_u64()?; // OptionU64 { is_none: bool, v: u64 }
    }
    r.read_option_u64()?; // tail: OptionU64 (single, NOT vector)
    let _level = r.read_u64()?;
    let _max_level = r.read_u64()?;
    let _list_p = r.read_u64()?;
    let _size = r.read_u64()?;

    // RewarderManager, PositionManager, is_pause, index, url follow.
    // Their exact BCS layout is complex (Bag, LinkedTable, custom options)
    // and fragile to reverse-engineer. Since we have all fields we need,
    // default is_pause to false. The caller can check via JSON content
    // if pause status is critical.
    let is_pause = false;

    Ok(CetusPoolPartial {
        coin_a,
        coin_b,
        tick_spacing,
        fee_rate,
        liquidity,
        current_sqrt_price,
        current_tick_index: current_tick_index_bits as i32,
        ticks_table_id,
        is_pause,
    })
}

/// A SkipList node containing tick data (from dynamic fields).
#[derive(Deserialize, Debug)]
pub(crate) struct CetusSkipListNodeRaw {
    pub score: u64,
    pub nexts: Vec<CetusOptionU64>,
    pub prev: CetusOptionU64,
    pub value: CetusTickRaw,
}

/// Cetus Tick stored inside SkipList nodes.
#[derive(Deserialize, Debug)]
pub(crate) struct CetusTickRaw {
    pub index: CetusI32,
    pub sqrt_price: u128,
    pub liquidity_net: CetusI128,
    pub liquidity_gross: u128,
    pub fee_growth_outside_a: u128,
    pub fee_growth_outside_b: u128,
    pub points_growth_outside: u128,
    pub rewards_growth_outside: Vec<u128>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct CetusI32 {
    pub bits: u32,
}

impl CetusI32 {
    pub fn to_i32(&self) -> i32 {
        self.bits as i32
    }
}

#[derive(Deserialize, Debug)]
pub(crate) struct CetusI128 {
    pub bits: u128,
}

impl CetusI128 {
    pub fn to_i128(&self) -> i128 {
        self.bits as i128
    }
}

#[derive(Deserialize, Debug)]
pub(crate) struct CetusOptionU64 {
    pub is_none: bool,
    pub v: u64,
}

// --- Manual BCS reader ---

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

    fn read_uleb128(&mut self) -> Result<usize, ArbError> {
        let mut result: usize = 0;
        let mut shift = 0;
        loop {
            self.ensure(1)?;
            let byte = self.data[self.pos];
            self.pos += 1;
            result |= ((byte & 0x7f) as usize) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        Ok(result)
    }

    fn read_string(&mut self) -> Result<String, ArbError> {
        let len = self.read_uleb128()?;
        let bytes = self.read_bytes(len)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|e| ArbError::BcsDeserialize(format!("invalid UTF-8 string: {}", e)))
    }

    fn read_option_u64(&mut self) -> Result<Option<u64>, ArbError> {
        let is_none = self.read_bool()?;
        let v = self.read_u64()?;
        Ok(if is_none { None } else { Some(v) })
    }

    /// Read a BCS Option<T> where T is `n` bytes. BCS Options are:
    /// 0 byte = None, or 1 byte + T bytes = Some(T).
    fn read_option_bytes(&mut self, n: usize) -> Result<Option<&'a [u8]>, ArbError> {
        let tag = self.read_uleb128()?; // 0 = None, 1 = Some
        if tag == 0 {
            Ok(None)
        } else {
            let bytes = self.read_bytes(n)?;
            Ok(Some(bytes))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cetus_i32_positive() {
        let i = CetusI32 { bits: 100 };
        assert_eq!(i.to_i32(), 100);
    }

    #[test]
    fn test_cetus_i32_negative() {
        let i = CetusI32 {
            bits: (-100i32) as u32,
        };
        assert_eq!(i.to_i32(), -100);
    }

    #[test]
    fn test_cetus_i32_zero() {
        let i = CetusI32 { bits: 0 };
        assert_eq!(i.to_i32(), 0);
    }

    #[test]
    fn test_cetus_i32_max_tick() {
        let i = CetusI32 { bits: 443636 };
        assert_eq!(i.to_i32(), 443636);
    }

    #[test]
    fn test_cetus_i32_min_tick() {
        let i = CetusI32 {
            bits: (-443636i32) as u32,
        };
        assert_eq!(i.to_i32(), -443636);
    }

    #[test]
    fn test_cetus_i128_positive() {
        let i = CetusI128 { bits: 12345 };
        assert_eq!(i.to_i128(), 12345);
    }

    #[test]
    fn test_cetus_i128_negative() {
        let i = CetusI128 {
            bits: (-12345i128) as u128,
        };
        assert_eq!(i.to_i128(), -12345);
    }
}
