//! # L2 Orderbook Module
//!
//! High-performance L2 orderbook implementation with OBI calculations.
//!
//! ## Architecture
//!
//! ```text
//! Exchange WS → L2Update → SymbolActor → L2Book → ObiSnapshot
//!                                            ↓
//!                                      Persistent L2
//! ```
//!
//! ## Key Design Decisions
//!
//! 1. **One writer per (exchange, symbol)**: Ensures deterministic state
//! 2. **No Arc<Mutex<L2Book>> in hot path**: Single-threaded updates via actor
//! 3. **BTreeMap for price levels**: O(log n) insertion/deletion, sorted by price
//! 4. **Bids: highest price first, Asks: lowest price first**
//!
//! ## OBI Calculation
//!
//! OBI_N = (BidVolume_N - AskVolume_N) / (BidVolume_N + AskVolume_N)
//!
//! Where N is the top N price levels.

pub mod l2_book;
pub mod l2_update;
pub mod obi;
pub mod health;
pub mod persistence;
pub mod symbol_actor;

pub use l2_book::{L2Book, Level, OrderBookSnapshot, SymbolSnapshot};
pub use l2_update::{Exchange, L2Update, Side};
pub use obi::{ObiSnapshot, ObiConfig};
pub use health::{ConnectionHealth, FeedHealth};
pub use persistence::{L2Persister, ReplaySource};

use serde::{Deserialize, Serialize};

/// Symbol metadata for price representation
///
/// Contains authoritative information for deterministic price encoding
/// per exchange and symbol pair.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SymbolMetadata {
    pub exchange: String,
    pub symbol: String,
    pub price_scale: u32,
    pub quantity_decimals: u32,
}

impl SymbolMetadata {
    pub fn new(exchange: &str, symbol: &str, price_scale: u32, quantity_decimals: u32) -> Self {
        Self {
            exchange: exchange.to_string(),
            symbol: symbol.to_string(),
            price_scale,
            quantity_decimals,
        }
    }

    pub fn btc_usdt() -> Self {
        Self::new("binance", "BTCUSDT", 8, 8)
    }

    pub fn eth_usdt() -> Self {
        Self::new("binance", "ETHUSDT", 8, 8)
    }

    pub fn from_exchange_symbol(exchange: &str, symbol: &str) -> Option<Self> {
        match (exchange.to_lowercase().as_str(), symbol.to_uppercase().as_str()) {
            ("binance", s) if s.ends_with("USDT") => Some(Self::new(exchange, symbol, 8, 8)),
            ("binance", s) if s.ends_with("BUSD") => Some(Self::new(exchange, symbol, 8, 8)),
            ("bybit", s) if s.ends_with("USDT") => Some(Self::new(exchange, symbol, 8, 8)),
            ("bitget", s) if s.ends_with("USDT") => Some(Self::new(exchange, symbol, 8, 8)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod symbol_metadata_tests {
    use super::*;

    #[test]
    fn test_btc_usdt_metadata() {
        let meta = SymbolMetadata::btc_usdt();
        assert_eq!(meta.exchange, "binance");
        assert_eq!(meta.symbol, "BTCUSDT");
        assert_eq!(meta.price_scale, 8);
        assert_eq!(meta.quantity_decimals, 8);
    }

    #[test]
    fn test_different_scales() {
        let btc = SymbolMetadata::new("binance", "BTCUSDT", 8, 8);
        let shib = SymbolMetadata::new("binance", "SHIBUSDT", 10, 8);
        assert_ne!(btc.price_scale, shib.price_scale);
    }

    #[test]
    fn test_unknown_symbol_returns_none() {
        assert!(SymbolMetadata::from_exchange_symbol("binance", "UNKNOWNPAIR").is_none());
        assert!(SymbolMetadata::from_exchange_symbol("unknown_exchange", "BTCUSDT").is_none());
    }

    #[test]
    fn test_deterministic_encoding() {
        let meta1 = SymbolMetadata::new("binance", "BTCUSDT", 8, 8);
        let meta2 = SymbolMetadata::new("binance", "BTCUSDT", 8, 8);
        assert_eq!(meta1, meta2);
    }
}

/// Price level with price and quantity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevel {
    /// Price (in quote currency, e.g., USD)
    pub price: f64,
    /// Quantity (in base currency, e.g., ETH)
    pub quantity: f64,
}

impl PriceLevel {
    pub fn new(price: f64, quantity: f64) -> Option<Self> {
        if price <= 0.0 || quantity < 0.0 {
            return None;
        }
        Some(Self { price, quantity })
    }
}

/// Top N orderbook depths
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepthLevels {
    pub top1_bid: Option<PriceLevel>,
    pub top1_ask: Option<PriceLevel>,
    pub top3_bid: Vec<PriceLevel>,
    pub top3_ask: Vec<PriceLevel>,
    pub top5_bid: Vec<PriceLevel>,
    pub top5_ask: Vec<PriceLevel>,
    pub top10_bid: Vec<PriceLevel>,
    pub top10_ask: Vec<PriceLevel>,
}

impl DepthLevels {
    pub fn from_book(book: &L2Book) -> Self {
        let scale = book.price_scale() as f64;
        let divisor = 10f64.powf(scale);
        Self {
            top1_bid: book.best_bid().map(|(p, q)| PriceLevel { price: p as f64 / divisor, quantity: q }),
            top1_ask: book.best_ask().map(|(p, q)| PriceLevel { price: p as f64 / divisor, quantity: q }),
            top3_bid: book.top_n_bids(3).into_iter().map(|(p, q)| PriceLevel { price: p as f64 / divisor, quantity: q }).collect(),
            top3_ask: book.top_n_asks(3).into_iter().map(|(p, q)| PriceLevel { price: p as f64 / divisor, quantity: q }).collect(),
            top5_bid: book.top_n_bids(5).into_iter().map(|(p, q)| PriceLevel { price: p as f64 / divisor, quantity: q }).collect(),
            top5_ask: book.top_n_asks(5).into_iter().map(|(p, q)| PriceLevel { price: p as f64 / divisor, quantity: q }).collect(),
            top10_bid: book.top_n_bids(10).into_iter().map(|(p, q)| PriceLevel { price: p as f64 / divisor, quantity: q }).collect(),
            top10_ask: book.top_n_asks(10).into_iter().map(|(p, q)| PriceLevel { price: p as f64 / divisor, quantity: q }).collect(),
        }
    }
}
