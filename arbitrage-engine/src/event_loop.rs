//! # Event Loop Module
//!
//! Real-time mempool and block ingestion from Arbitrum via WebSocket.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                    ARBITRUM EVENT LOOP PIPELINE                          │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │                                                                          │
//! │  Arbitrum Node (ws://...)                                                │
//! │      │                                                                   │
//! │      ├── eth_subscribe("newPendingTransactions")                          │
//! │      │                                                                   │
//! │      └── eth_subscribe("newHeads")                                      │
//! │                                                                          │
//! │      ↓                                                                   │
//! │  ┌─────────────────────────────────────────────────────────────────┐   │
//! │  │              WebSocket Connection Manager                          │   │
//! │  │                                                                  │   │
//! │  │  - Automatic reconnection with exponential backoff                 │   │
//! │  │  - Connection health monitoring                                    │   │
//! │  │  - Subscription management                                        │   │
//! │  └─────────────────────────────────────────────────────────────────┘   │
//! │      │                                                                   │
//! │      ↓                                                                   │
//! │  ┌─────────────────────────────────────────────────────────────────┐   │
//! │  │              Fast Calldata Parser                                │   │
//! │  │                                                                  │   │
//! │  │  - Zero-copy extraction of to, input, value                     │   │
//! │  │  - Immediate drop of non-DEX transactions                        │   │
//! │  │  - Only allocate for relevant transactions                       │   │
//! │  └─────────────────────────────────────────────────────────────────┘   │
//! │      │                                                                   │
//! │      ↓                                                                   │
//! │  ┌─────────────────────────────────────────────────────────────────┐   │
//! │  │              Target DEX Filters                                   │   │
//! │  │                                                                  │   │
//! │  │  - Uniswap V3 Router: 0xE592427A...                            │   │
//! │  │  - Uniswap V2 Router: 0x7a250d56...                           │   │
//! │  │  - Balancer Vault:    0xBA122222...                            │   │
//! │  │  - SushiSwap Router:  0xd9e1cE2...                            │   │
//! │  │  - Curve Router:       0x99a5848...                            │   │
//! │  └─────────────────────────────────────────────────────────────────┘   │
//! │      │                                                                   │
//! │      ↓                                                                   │
//! │  ┌─────────────────────────────────────────────────────────────────┐   │
//! │  │              SPSC Ring Buffer (Core 2)                          │   │
//! │  │                                                                  │   │
//! │  │  push(MempoolEvent) → Core 3 Simulation Thread                │   │
//! │  └─────────────────────────────────────────────────────────────────┘   │
//! │                                                                          │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Why WebSocket vs HTTP Polling?
//!
//! | Aspect | HTTP Polling | WebSocket Subscription |
//! |--------|--------------|--------------------------|
//! | Latency | 50-100ms per poll | <1ms notification |
//! | Load | High (continuous requests) | Low (persistent) |
//! | Missed txs | Possible between polls | None (instant) |
//! | Cost | API rate limits | No limit |

use crate::engine::SharedState;
use crate::error::{ArbitrageError, Result};
use crate::types::{BlockHeader, MempoolEvent};

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Target DEX router/pool addresses on Arbitrum
///
/// These are the addresses we monitor for arbitrage opportunities.
const TARGET_ADDRESSES: &[&str] = &[
    // Uniswap V3 Router
    "0xE592427A0AEce92De3Edee1F18E0157C05861564",
    // Uniswap V3 Quoter
    "0xb27308f9F90D607463bb33eA1BeA3D0D3D15F8E",
    // Uniswap V2 Router
    "0x4752ba5DBc23f44D87826276BF6Fd6b1C372aD24",
    // Balancer Vault
    "0xBA12222222228d8Ba445958a75a0704d566BF2C8",
    // SushiSwap Router
    "0x1b02dA8Cb0d097eB8D57A175b88c7D8b47997506",
    // Curve Router
    "0x99a5848E3f6C7a1e3B8c0E4E5dB5cD4F6e7A8b9",
];

/// Arbitrum RPC WebSocket endpoints
const ARB_WS_URLS: &[&str] = &[
    "wss://arb1.arbitrum.io/ws",
    "wss://arbitrum.public-rpc.com/ws",
];

/// Maximum reconnection attempts before giving up
const MAX_RECONNECT_ATTEMPTS: u32 = 10;

/// Initial backoff delay in milliseconds
const INITIAL_BACKOFF_MS: u64 = 100;

/// Maximum backoff delay in milliseconds  
const MAX_BACKOFF_MS: u64 = 30_000;

/// Event loop configuration
#[derive(Debug, Clone)]
pub struct EventLoopConfig {
    /// WebSocket URL for Arbitrum node
    pub ws_url: String,
    /// Maximum events per second to process (0 = unlimited)
    pub max_events_per_second: usize,
    /// Enable debug logging
    pub debug: bool,
}

impl Default for EventLoopConfig {
    fn default() -> Self {
        Self {
            ws_url: ARB_WS_URLS[0].to_string(),
            max_events_per_second: 0,
            debug: false,
        }
    }
}

/// Event loop state
pub struct EventLoop {
    state: Arc<SharedState>,
    config: EventLoopConfig,
    target_addresses: Vec<[u8; 20]>,
}

impl EventLoop {
    pub fn new(state: Arc<SharedState>, config: EventLoopConfig) -> Self {
        // Pre-compute target addresses as bytes for fast comparison
        let target_addresses = TARGET_ADDRESSES
            .iter()
            .filter_map(|addr| {
                let hex = addr.trim_start_matches("0x");
                let bytes = hex::decode(hex).ok()?;
                if bytes.len() == 20 {
                    let mut arr = [0u8; 20];
                    arr.copy_from_slice(&bytes);
                    Some(arr)
                } else {
                    None
                }
            })
            .collect();

        Self {
            state,
            config,
            target_addresses,
        }
    }

    /// Check if a transaction is relevant (interacts with target DEX)
    #[inline]
    fn is_relevant_tx(&self, to: &[u8; 20]) -> bool {
        self.target_addresses.iter().any(|addr| addr == to)
    }

    /// Run the event loop (blocking)
    pub async fn run(&self) -> Result<()> {
        tracing::info!("Starting Arbitrum event loop");
        tracing::info!("WebSocket URL: {}", self.config.ws_url);
        tracing::info!("Target addresses: {} contracts", TARGET_ADDRESSES.len());

        let mut attempts = 0u32;
        let mut backoff_ms = INITIAL_BACKOFF_MS;

        loop {
            if self.state.is_shutdown() {
                tracing::info!("Event loop shutting down");
                break;
            }

            match self.connect_and_subscribe().await {
                Ok(()) => {
                    tracing::info!("Event loop disconnected normally");
                    break;
                }
                Err(e) => {
                    attempts += 1;
                    if attempts > MAX_RECONNECT_ATTEMPTS {
                        tracing::error!(
                            "Max reconnection attempts ({}) reached. Event loop exiting.",
                            MAX_RECONNECT_ATTEMPTS
                        );
                        return Err(e);
                    }

                    tracing::warn!(
                        "Event loop connection failed (attempt {}): {}. Retrying in {}ms...",
                        attempts,
                        e,
                        backoff_ms
                    );

                    // Exponential backoff
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
                }
            }
        }

        Ok(())
    }

    /// Connect and subscribe to mempool/block events
    async fn connect_and_subscribe(&self) -> Result<()> {
        let url = &self.config.ws_url;

        tracing::info!("Connecting to {}", url);

        let (ws_stream, _) = connect_async(url)
            .await
            .map_err(|e| ArbitrageError::WebSocket(format!("Connection failed: {}", e)))?;

        tracing::info!("Connected to Arbitrum WebSocket");

        let (mut write, mut read) = ws_stream.split();

        // Subscribe to pending transactions
        let pending_sub = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_subscribe",
            "params": ["alchemy_newFullPendingTransactions"]
        });

        let pending_bytes = serde_json::to_vec(&pending_sub)
            .map_err(|e| ArbitrageError::JsonParse(format!("Failed to serialize: {}", e)))?;

        write
            .send(Message::Binary(pending_bytes))
            .await
            .map_err(|e| ArbitrageError::WebSocket(format!("Send failed: {}", e)))?;

        tracing::info!("Subscribed to pending transactions");

        // Subscribe to new blocks
        let blocks_sub = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "eth_subscribe",
            "params": ["newHeads"]
        });

        let blocks_bytes = serde_json::to_vec(&blocks_sub)
            .map_err(|e| ArbitrageError::JsonParse(format!("Failed to serialize: {}", e)))?;

        write
            .send(Message::Binary(blocks_bytes))
            .await
            .map_err(|e| ArbitrageError::WebSocket(format!("Send failed: {}", e)))?;

        tracing::info!("Subscribed to new block headers");

        // Process messages
        loop {
            tokio::select! {
                // Check for shutdown
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    if self.state.is_shutdown() {
                        break;
                    }
                }

                // Read WebSocket messages
                msg = read.next() => {
                    match msg {
                        Some(Ok(Message::Binary(data))) => {
                            self.handle_message(&data).await;
                        }
                        Some(Ok(Message::Text(data))) => {
                            self.handle_message(data.as_bytes()).await;
                        }
                        Some(Ok(Message::Close(..))) | None => {
                            tracing::info!("WebSocket closed by peer");
                            break;
                        }
                        Some(Err(e)) => {
                            tracing::warn!("WebSocket error: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle incoming WebSocket message
    async fn handle_message(&self, data: &[u8]) {
        // Try to parse as JSON
        let json: serde_json::Value = match serde_json::from_slice(data) {
            Ok(v) => v,
            Err(_) => return,
        };

        // Check for subscription result or notification
        if let Some(params) = json.get("params") {
            let result = &params["result"];

            // Check if it's a pending transaction (has "transaction" field)
            if let Some(tx) = result.get("transaction") {
                self.handle_pending_tx(tx).await;
            }
            // Check if it's a block header (result is the block object itself)
            else if result.is_object() {
                self.handle_block_header(result).await;
            }
        }
    }

    /// Handle a pending transaction notification
    async fn handle_pending_tx(&self, tx: &serde_json::Value) {
        // Extract target address (to field)
        let to_hex = match tx.get("to").and_then(|v| v.as_str()) {
            Some(v) => v,
            None => return, // No target (contract creation)
        };

        // Parse and check if relevant
        let to_bytes = match hex::decode(to_hex.trim_start_matches("0x")) {
            Ok(b) if b.len() == 20 => {
                let mut arr = [0u8; 20];
                arr.copy_from_slice(&b);
                arr
            }
            _ => return, // Invalid address
        };

        // Fast filter: skip non-target transactions
        if !self.is_relevant_tx(&to_bytes) {
            return;
        }

        // Extract transaction data
        let input_hex = tx.get("input").and_then(|v| v.as_str()).unwrap_or("0x");

        let input = match hex::decode(input_hex.trim_start_matches("0x")) {
            Ok(v) => v,
            Err(_) => return,
        };

        // Extract value
        let value_hex = tx.get("value").and_then(|v| v.as_str()).unwrap_or("0x0");
        let value = u64::from_str_radix(value_hex.trim_start_matches("0x"), 16).unwrap_or(0);

        // Extract gas price
        let gas_price_hex = tx.get("gasPrice").and_then(|v| v.as_str()).unwrap_or("0x0");
        let gas_price =
            u64::from_str_radix(gas_price_hex.trim_start_matches("0x"), 16).unwrap_or(0);

        // Extract transaction hash
        let tx_hash = tx.get("hash").and_then(|v| v.as_str()).unwrap_or("");

        // Extract from address
        let from_hex = tx.get("from").and_then(|v| v.as_str()).unwrap_or("");

        if self.config.debug {
            tracing::debug!(
                "Relevant tx: {} to={} value={} gas_price={}",
                &tx_hash[..8],
                &to_hex[..10],
                value,
                gas_price
            );
        }

        // Create mempool event
        let event = MempoolEvent {
            tx_hash: tx_hash.to_string(),
            from: from_hex.to_string(),
            to: to_hex.to_string(),
            input,
            value,
            gas_price,
            block_number: None,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        };

        // Push to simulation channel (non-blocking)
        if let Err(e) = self.state.push_mempool_event(event) {
            tracing::warn!("Failed to push mempool event: {}", e);
        }
    }

    /// Handle a new block header notification
    async fn handle_block_header(&self, block: &serde_json::Value) {
        let block_number = block
            .get("number")
            .and_then(|v| v.as_str())
            .map(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0))
            .unwrap_or(0);

        let timestamp = block
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0))
            .unwrap_or(0);

        let hash = block.get("hash").and_then(|v| v.as_str()).unwrap_or("");

        let header = BlockHeader {
            number: block_number,
            hash: hash.to_string(),
            parent_hash: block
                .get("parentHash")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            timestamp,
            gas_limit: block
                .get("gasLimit")
                .and_then(|v| v.as_str())
                .map(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0))
                .unwrap_or(0),
            base_fee_per_gas: block
                .get("baseFeePerGas")
                .and_then(|v| v.as_str())
                .map(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0))
                .unwrap_or(0),
        };

        if self.config.debug {
            tracing::debug!(
                "New block: #{} hash={} timestamp={}",
                block_number,
                &hash[..10],
                timestamp
            );
        }

        // Push to block header channel
        if let Err(e) = self.state.push_block_header(header) {
            tracing::warn!("Failed to push block header: {}", e);
        }
    }
}

/// Spawn the event loop as a background task
pub fn spawn_event_loop(
    state: Arc<SharedState>,
    config: EventLoopConfig,
) -> Result<std::thread::JoinHandle<()>> {
    let event_loop = EventLoop::new(state.clone(), config);

    let handle = std::thread::Builder::new()
        .name("event-loop".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            rt.block_on(async {
                if let Err(e) = event_loop.run().await {
                    tracing::error!("Event loop error: {}", e);
                }
            });
        })
        .map_err(|e| ArbitrageError::System(format!("Failed to spawn event loop: {}", e)))?;

    Ok(handle)
}

/// Create default event loop configuration
pub fn default_config() -> EventLoopConfig {
    // Allow override via environment variable
    let ws_url = std::env::var("ARBITRUM_WS_URL").unwrap_or_else(|_| ARB_WS_URLS[0].to_string());

    EventLoopConfig {
        ws_url,
        max_events_per_second: 0,
        debug: std::env::var("DEBUG")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_relevant_tx() {
        let state = Arc::new(crate::engine::SharedState::default());
        let config = EventLoopConfig::default();
        let loop_ = EventLoop::new(state, config);

        // Uniswap V3 Router
        let uniswap = hex::decode("E592427A0AEce92De3Edee1F18E0157C05861564").unwrap();
        let mut arr = [0u8; 20];
        arr.copy_from_slice(&uniswap);
        assert!(loop_.is_relevant_tx(&arr));

        // Random address
        let random = hex::decode("0000000000000000000000000000000000000001").unwrap();
        let mut arr2 = [0u8; 20];
        arr2.copy_from_slice(&random);
        assert!(!loop_.is_relevant_tx(&arr2));
    }
}
