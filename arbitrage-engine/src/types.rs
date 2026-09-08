//! # Types Module
//!
//! Core type definitions for the arbitrage engine.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// Price event from Binance aggTrade stream
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceEvent {
    /// Event type (aggTrade)
    pub event_type: String,

    /// Trading symbol (e.g., "ETHUSDT")
    pub symbol: String,

    /// Trade price
    pub price: String,

    /// Trade quantity
    pub quantity: String,

    /// Trade timestamp (milliseconds)
    pub trade_time: u64,

    /// Is buyer maker?
    pub is_buyer_maker: bool,

    /// Received timestamp (for latency measurement)
    #[serde(skip)]
    pub received_at: u64,
}

impl PriceEvent {
    /// Create a new price event
    pub fn new(
        symbol: String,
        price: String,
        quantity: String,
        trade_time: u64,
        is_buyer_maker: bool,
    ) -> Self {
        Self {
            event_type: "aggTrade".to_string(),
            symbol,
            price,
            quantity,
            trade_time,
            is_buyer_maker,
            received_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        }
    }

    /// Parse price as f64
    pub fn price_f64(&self) -> Option<f64> {
        self.price.parse().ok()
    }

    /// Parse quantity as f64
    pub fn quantity_f64(&self) -> Option<f64> {
        self.quantity.parse().ok()
    }
}

/// Arbitrage opportunity detected
#[derive(Debug, Clone)]
pub struct ArbitrageOpportunity {
    /// Source exchange (lead)
    pub lead_exchange: String,

    /// Target DEX (lag)
    pub lag_exchange: String,

    /// Buy price (CEX)
    pub buy_price: f64,

    /// Sell price (DEX)
    pub sell_price: f64,

    /// Price deviation percentage
    pub deviation_pct: f64,

    /// Estimated profit in wei
    pub estimated_profit_wei: u64,

    /// Token pair
    pub token_pair: (String, String),

    /// Chain ID
    pub chain_id: u64,

    /// Timestamp
    pub timestamp: u64,
}

/// Arbitrage transaction to be sent
#[derive(Debug, Clone)]
pub struct ArbitrageTx {
    /// Encoded transaction data
    pub to: String,

    /// Calldata
    pub data: Vec<u8>,

    /// Value in wei
    pub value: u64,

    /// Gas limit
    pub gas_limit: u64,

    /// Nonce
    pub nonce: u64,

    /// Chain ID
    pub chain_id: u64,

    /// Max priority fee per gas
    pub max_priority_fee: u64,

    /// Max fee per gas
    pub max_fee: u64,
}

/// Pool state for simulation
#[derive(Debug, Clone)]
pub struct PoolState {
    /// Pool address
    pub address: String,

    /// Token0 address
    pub token0: String,

    /// Token1 address
    pub token1: String,

    /// Reserve0
    pub reserve0: u128,

    /// Reserve1
    pub reserve1: u128,

    /// Fee tier (basis points)
    pub fee_tier: u32,

    /// Liquidity
    pub liquidity: u128,

    /// SqrtPriceX96
    pub sqrt_price_x96: u128,

    /// Current tick (for V3)
    pub current_tick: Option<i32>,

    /// Last update time
    pub last_update: u64,
}

/// DEX type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DexType {
    UniswapV2 = 0,
    UniswapV3 = 1,
    Aerodrome = 2,
    SushiSwap = 3,
}

impl DexType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::UniswapV2),
            1 => Some(Self::UniswapV3),
            2 => Some(Self::Aerodrome),
            3 => Some(Self::SushiSwap),
            _ => None,
        }
    }
}

/// Mempool transaction event from WebSocket subscription
#[derive(Debug, Clone)]
pub struct MempoolEvent {
    /// Transaction hash
    pub tx_hash: String,

    /// Sender address
    pub from: String,

    /// Target contract address
    pub to: String,

    /// Calldata (input bytes)
    pub input: Vec<u8>,

    /// ETH value in wei
    pub value: u64,

    /// Gas price in wei
    pub gas_price: u64,

    /// Block number (None for pending)
    pub block_number: Option<u64>,

    /// Event timestamp (unix ms)
    pub timestamp: u64,
}

/// Block header from newHeads subscription
#[derive(Debug, Clone)]
pub struct BlockHeader {
    /// Block number
    pub number: u64,

    /// Block hash
    pub hash: String,

    /// Parent block hash
    pub parent_hash: String,

    /// Block timestamp
    pub timestamp: u64,

    /// Block gas limit
    pub gas_limit: u64,

    /// Base fee per gas (EIP-1559)
    pub base_fee_per_gas: u64,
}

/// Simulation result
#[derive(Debug, Clone)]
pub struct SimulationResult {
    /// Whether simulation succeeded
    pub success: bool,

    /// Profit in wei (0 if failed)
    pub profit_wei: u64,

    /// Gas used
    pub gas_used: u64,

    /// Revert reason if failed
    pub revert_reason: Option<String>,

    /// Execution time in microseconds
    pub execution_time_us: u64,
}

/// Performance statistics
#[derive(Debug, Default)]
pub struct Stats {
    /// Total signals received
    pub signals_received: AtomicU64,

    /// Total opportunities found
    pub opportunities_found: AtomicU64,

    /// Total transactions sent
    pub txs_sent: AtomicU64,

    /// Total transactions confirmed
    pub txs_confirmed: AtomicU64,

    /// Total transactions failed
    pub txs_failed: AtomicU64,

    /// Total profit in wei
    pub total_profit_wei: AtomicU64,

    /// Average latency in microseconds
    pub avg_latency_us: AtomicU64,
}

impl Stats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_signal(&self) {
        self.signals_received.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_opportunity(&self) {
        self.opportunities_found.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_tx_sent(&self) {
        self.txs_sent.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_tx_confirmed(&self) {
        self.txs_confirmed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_tx_failed(&self) {
        self.txs_failed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_profit(&self, profit_wei: u64) {
        self.total_profit_wei
            .fetch_add(profit_wei, Ordering::Relaxed);
    }

    pub fn record_latency(&self, latency_us: u64) {
        let current = self.avg_latency_us.load(Ordering::Relaxed);
        let count = self.signals_received.load(Ordering::Relaxed);
        if count > 0 {
            let new_avg = (current * (count - 1) + latency_us) / count;
            self.avg_latency_us.store(new_avg, Ordering::Relaxed);
        }
    }

    pub fn signals(&self) -> u64 {
        self.signals_received.load(Ordering::Relaxed)
    }

    pub fn opportunities(&self) -> u64 {
        self.opportunities_found.load(Ordering::Relaxed)
    }

    pub fn txs_sent(&self) -> u64 {
        self.txs_sent.load(Ordering::Relaxed)
    }

    pub fn avg_latency(&self) -> u64 {
        self.avg_latency_us.load(Ordering::Relaxed)
    }
}
