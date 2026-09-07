#![allow(dead_code)]
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::type_complexity)]

//! # Lead-Lag Arbitrage Engine Library
//!
//! High-performance, sub-millisecond CEX-DEX arbitrage engine targeting Ethereum L2s.
//!
//! ## Architecture
//!
//! - **Lead Signal**: Binance WebSocket `aggTrade` stream
//! - **Execution**: On-chain DEX pools via Balancer V2 Flash Loans
//! - **Simulation**: REVM in-memory EVM for zero-gas failed trades
//!
//! ## Threading Model
//!
//! - `WebSocketThread`: Binance data ingestion (SPSC Producer)
//! - `SimulationThread`: REVM execution engine (SPSC Consumer)
//! - `SenderThread`: Transaction construction & broadcast
//!
//! ## Key Features
//!
//! - Zero-capital via Balancer flash loans (0% fee)
//! - Zero-copy JSON parsing with `simd-json`
//! - Lock-free SPSC ring buffers for inter-thread communication
//! - In-memory `CacheDB` pre-synced via WebSocket events

pub mod config;
pub mod error;
pub mod types;
pub mod engine;
pub mod cache_db;
pub mod websocket;
pub mod simulation;
pub mod math;
pub mod sender;
pub mod hydration;
pub mod broadcaster;
pub mod tsc;
pub mod event_loop;
pub mod pool_discovery;
pub mod pool_scoring;
pub mod lead_lag;
pub mod route_gen;
pub mod revmsim;
pub mod two_leg_route;
pub mod executor_abi;
pub mod orderbook;

pub use config::Config;
pub use error::ArbitrageError;
pub use types::*;
pub use engine::ArbitrageEngine;
