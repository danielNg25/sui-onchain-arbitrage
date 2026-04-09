use arb_types::error::ArbError;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("no cycles found for pool {0}")]
    NoCycles(String),

    #[error("simulation failed: {0}")]
    Simulation(String),

    #[error("pool error: {0}")]
    Pool(#[from] ArbError),

    #[error("price fetch failed: {0}")]
    PriceFetch(String),

    #[error("config error: {0}")]
    Config(String),
}
