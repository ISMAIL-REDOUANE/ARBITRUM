//! # Error Types Module
//!
//! Centralized error types for the arbitrage engine.

use thiserror::Error;

/// Main error enum for arbitrage engine
#[derive(Debug, Error)]
pub enum ArbitrageError {
    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("JSON parse error: {0}")]
    JsonParse(String),

    #[error("Simulation error: {0}")]
    Simulation(String),

    #[error("CacheDB error: {0}")]
    CacheDb(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Channel error: {0}")]
    Channel(String),

    #[error("Transaction error: {0}")]
    Transaction(String),

    #[error("Contract error: {0}")]
    Contract(String),

    #[error("Insufficient profit: {0} < {1}")]
    InsufficientProfit(u64, u64),

    #[error("Pool not found: {0}")]
    PoolNotFound(String),

    #[error("Invalid config: {0}")]
    InvalidConfig(String),

    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("Encoding error: {0}")]
    Encoding(String),

    #[error("Shutdown error: {0}")]
    Shutdown(String),

    #[error("System error: {0}")]
    System(String),

    #[error("Crypto error: {0}")]
    Crypto(String),
}

impl serde::Serialize for ArbitrageError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ArbitrageError>;
