//! # Symbol Actor Module
//!
//! Single-writer actor for each (exchange, symbol) pair.
//!
//! ## Architecture
//!
//! ```text
//! L2Update channel → SymbolActor → L2Book → ObiSnapshot → Signal
//!                         ↓
//!                    L2Persister
//! ```
//!
//! ## One Writer Guarantee
//!
//! Each SymbolActor owns exactly one L2Book instance.
//! Updates are processed sequentially, ensuring deterministic state.

use crate::orderbook::{L2Book, ObiConfig, ObiSnapshot, SymbolSnapshot};
use crate::orderbook::l2_update::L2Update;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Message to SymbolActor
#[derive(Debug)]
pub enum ActorMessage {
    /// L2 update from exchange
    Update(L2Update),
    /// Get current snapshot (request)
    GetSnapshot(mpsc::Sender<Option<SymbolSnapshot>>),
    /// Health check
    HealthCheck,
    /// Shutdown signal
    Shutdown,
}

/// SymbolActor processing state for one (exchange, symbol) pair
pub struct SymbolActor {
    /// Exchange
    exchange: String,
    /// Symbol
    symbol: String,
    /// Orderbook state
    book: L2Book,
    /// OBI configuration
    obi_config: ObiConfig,
    /// Last update timestamp
    last_update: Instant,
    /// Messages processed
    messages_processed: u64,
}

impl SymbolActor {
    pub fn new(exchange: &str, symbol: &str) -> Self {
        Self {
            exchange: exchange.to_string(),
            symbol: symbol.to_string(),
            book: L2Book::new(symbol, exchange),
            obi_config: ObiConfig::default(),
            last_update: Instant::now(),
            messages_processed: 0,
        }
    }

    pub fn new_with_config(exchange: &str, symbol: &str, obi_config: ObiConfig) -> Self {
        Self {
            exchange: exchange.to_string(),
            symbol: symbol.to_string(),
            book: L2Book::new(symbol, exchange),
            obi_config,
            last_update: Instant::now(),
            messages_processed: 0,
        }
    }

    /// Process an L2 update
    pub fn process_update(&mut self, update: &L2Update) -> Result<(), ActorError> {
        // Validate update
        update.validate().map_err(ActorError::InvalidUpdate)?;

        // Apply snapshot to book
        let snapshot = crate::orderbook::OrderBookSnapshot::new(
            update.exchange.as_str(),
            &update.symbol,
            update.bids.clone(),
            update.asks.clone(),
            update.exchange_ts_ns,
            update.recv_ts_ns,
            update.update_id,
        );

        self.book.apply_snapshot(&snapshot)
            .map_err(ActorError::BookError)?;

        self.last_update = Instant::now();
        self.messages_processed += 1;

        Ok(())
    }

    /// Get current OBI snapshot
    pub fn get_obi_snapshot(&self) -> ObiSnapshot {
        let scale = self.book.price_scale() as f64;
        let bids_f64: Vec<(f64, f64)> = self.book.top_n_bids(10)
            .into_iter()
            .map(|(p, q)| (p as f64 / scale, q))
            .collect();
        let asks_f64: Vec<(f64, f64)> = self.book.top_n_asks(10)
            .into_iter()
            .map(|(p, q)| (p as f64 / scale, q))
            .collect();
        ObiSnapshot::calculate(
            &self.exchange,
            &self.symbol,
            self.book.exchange_ts_ns,
            self.book.recv_ts_ns,
            &bids_f64,
            &asks_f64,
            &self.obi_config,
        )
    }

    /// Get current symbol snapshot
    pub fn get_symbol_snapshot(&self) -> SymbolSnapshot {
        SymbolSnapshot::from_book(&self.book)
    }

    /// Check if actor is stale (no updates for duration)
    pub fn is_stale(&self, max_age: Duration) -> bool {
        self.last_update.elapsed() > max_age
    }

    /// Get time since last update
    pub fn time_since_update(&self) -> Duration {
        self.last_update.elapsed()
    }

    /// Get messages processed count
    pub fn messages_processed(&self) -> u64 {
        self.messages_processed
    }

    /// Get reference to the orderbook
    pub fn book(&self) -> &L2Book {
        &self.book
    }
}

/// Actor error types
#[derive(Debug)]
pub enum ActorError {
    InvalidUpdate(crate::orderbook::l2_update::L2UpdateError),
    BookError(crate::orderbook::l2_book::BookError),
    ChannelError,
    Shutdown,
}

impl std::fmt::Display for ActorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActorError::InvalidUpdate(e) => write!(f, "Invalid update: {}", e),
            ActorError::BookError(e) => write!(f, "Book error: {}", e),
            ActorError::ChannelError => write!(f, "Channel error"),
            ActorError::Shutdown => write!(f, "Actor shutdown"),
        }
    }
}

impl std::error::Error for ActorError {}

/// Actor handle for sending messages
pub struct SymbolActorHandle {
    tx: mpsc::Sender<ActorMessage>,
}

impl SymbolActorHandle {
    pub fn new(exchange: &str, symbol: &str) -> (Self, mpsc::Receiver<ActorMessage>) {
        let (tx, rx) = mpsc::channel();
        let _actor = SymbolActor::new(exchange, symbol);
        (Self { tx }, rx)
    }

    pub fn new_with_config(exchange: &str, symbol: &str, obi_config: ObiConfig) -> (Self, mpsc::Receiver<ActorMessage>, SymbolActor) {
        let (tx, rx) = mpsc::channel();
        let actor = SymbolActor::new_with_config(exchange, symbol, obi_config);
        (Self { tx }, rx, actor)
    }

    /// Send an L2 update
    pub fn send_update(&self, update: L2Update) -> Result<(), ActorError> {
        self.tx
            .send(ActorMessage::Update(update))
            .map_err(|_| ActorError::ChannelError)
    }

    /// Request current snapshot
    pub fn get_snapshot(&self) -> Result<mpsc::Receiver<Option<SymbolSnapshot>>, ActorError> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(ActorMessage::GetSnapshot(tx))
            .map_err(|_| ActorError::ChannelError)?;
        Ok(rx)
    }

    /// Send shutdown signal
    pub fn shutdown(&self) -> Result<(), ActorError> {
        self.tx
            .send(ActorMessage::Shutdown)
            .map_err(|_| ActorError::ChannelError)
    }
}

/// Run the actor loop
pub fn run_actor(actor: &mut SymbolActor, rx: &mut mpsc::Receiver<ActorMessage>) -> Result<(), ActorError> {
    loop {
        match rx.recv() {
            Ok(ActorMessage::Update(update)) => {
                actor.process_update(&update)?;
            }
            Ok(ActorMessage::GetSnapshot(tx)) => {
                let snapshot = actor.get_symbol_snapshot();
                let _ = tx.send(Some(snapshot));
            }
            Ok(ActorMessage::HealthCheck) => {
                // Health check handled externally
            }
            Ok(ActorMessage::Shutdown) | Err(_) => {
                return Err(ActorError::Shutdown);
            }
        }
    }
}

/// Actor registry managing all symbol actors
pub struct ActorRegistry {
    actors: std::collections::HashMap<(String, String), SymbolActorHandle>,
}

impl ActorRegistry {
    pub fn new() -> Self {
        Self {
            actors: std::collections::HashMap::new(),
        }
    }

    /// Register a new (exchange, symbol) pair
    pub fn register(&mut self, exchange: &str, symbol: &str) -> &mut SymbolActorHandle {
        let key = (exchange.to_string(), symbol.to_string());
        if !self.actors.contains_key(&key) {
            let (handle, _rx) = SymbolActorHandle::new(exchange, symbol);
            self.actors.insert(key.clone(), handle);
        }
        self.actors.get_mut(&key).unwrap()
    }

    /// Get handle for (exchange, symbol)
    pub fn get(&mut self, exchange: &str, symbol: &str) -> Option<&mut SymbolActorHandle> {
        let key = (exchange.to_string(), symbol.to_string());
        self.actors.get_mut(&key)
    }

    /// Check if (exchange, symbol) is registered
    pub fn contains(&self, exchange: &str, symbol: &str) -> bool {
        let key = (exchange.to_string(), symbol.to_string());
        self.actors.contains_key(&key)
    }

    /// Get all registered pairs
    pub fn registered_pairs(&self) -> Vec<(String, String)> {
        self.actors.keys().cloned().collect()
    }
}

impl Default for ActorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orderbook::l2_update::Exchange;

    #[test]
    fn test_actor_creation() {
        let actor = SymbolActor::new("binance", "BTCUSDT");
        assert_eq!(actor.exchange, "binance");
        assert_eq!(actor.symbol, "BTCUSDT");
        assert!(actor.book.is_empty());
    }

    #[test]
    fn test_process_valid_update() {
        let mut actor = SymbolActor::new("binance", "BTCUSDT");

        let update = L2Update::snapshot(
            Exchange::Binance,
            "BTCUSDT",
            1000,
            2000,
            1,
            vec![(100.0, 1.0), (99.0, 2.0)],
            vec![(101.0, 1.0), (102.0, 2.0)],
            "depth@100ms",
        );

        actor.process_update(&update).unwrap();
        assert!(!actor.book.is_empty());
        assert!(actor.book.is_valid);
        assert_eq!(actor.messages_processed(), 1);
    }

    #[test]
    fn test_get_obi_snapshot() {
        let mut actor = SymbolActor::new("binance", "BTCUSDT");

        let update = L2Update::snapshot(
            Exchange::Binance,
            "BTCUSDT",
            1000,
            2000,
            1,
            vec![(100.0, 1.0)],
            vec![(101.0, 1.0)],
            "depth@100ms",
        );

        actor.process_update(&update).unwrap();
        let snapshot = actor.get_obi_snapshot();

        assert!(snapshot.is_valid);
        assert!(snapshot.mid.is_some());
    }

    #[test]
    fn test_staleness() {
        let actor = SymbolActor::new("binance", "BTCUSDT");
        // Fresh actor should not be stale
        assert!(!actor.is_stale(Duration::from_secs(1)));
    }

    #[test]
    fn test_actor_registry() {
        let mut registry = ActorRegistry::new();
        registry.register("binance", "BTCUSDT");
        registry.register("bybit", "ETHUSDT");

        assert!(registry.contains("binance", "BTCUSDT"));
        assert!(registry.contains("bybit", "ETHUSDT"));
        assert!(!registry.contains("binance", "ETHUSDT"));

        let pairs = registry.registered_pairs();
        assert_eq!(pairs.len(), 2);
    }
}
