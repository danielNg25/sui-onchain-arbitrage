use arb_types::error::ArbError;

#[derive(Debug, Clone, thiserror::Error)]
pub enum MathError {
    #[error("multiplication overflow in {0}")]
    MulOverflow(&'static str),

    #[error("sqrt_price out of bounds: {0}")]
    SqrtPriceOutOfBounds(u128),

    #[error("tick out of bounds: {0}")]
    TickOutOfBounds(i32),

    #[error("division by zero")]
    DivisionByZero,

    #[error("value truncation in {0}")]
    Truncation(&'static str),

    #[error("liquidity overflow: {0}")]
    LiquidityOverflow(&'static str),
}

impl From<MathError> for ArbError {
    fn from(e: MathError) -> Self {
        ArbError::InvalidData(e.to_string())
    }
}
