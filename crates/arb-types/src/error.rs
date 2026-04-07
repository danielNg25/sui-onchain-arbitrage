#[derive(Debug, thiserror::Error)]
pub enum ArbError {
    #[error("BCS deserialization failed: {0}")]
    BcsDeserialize(String),

    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("Pool not found: {0}")]
    PoolNotFound(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Invalid data: {0}")]
    InvalidData(String),
}
