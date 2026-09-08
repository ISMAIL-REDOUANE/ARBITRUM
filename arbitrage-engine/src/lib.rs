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

pub mod broadcaster;
pub mod cache_db;
pub mod config;
pub mod engine;
pub mod error;
pub mod event_loop;
pub mod executor_abi;
pub mod hydration;
pub mod lead_lag;
pub mod math;
pub mod orderbook;
pub mod pool_discovery;
pub mod pool_scoring;
pub mod revmsim;
pub mod route_gen;
pub mod sender;
pub mod simulation;
pub mod tsc;
pub mod two_leg_route;
pub mod types;
pub mod websocket;

pub use config::Config;
pub use engine::ArbitrageEngine;
pub use error::ArbitrageError;
pub use types::*;
