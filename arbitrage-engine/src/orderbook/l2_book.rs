//! # L2 Book Implementation
//!
//! Thread-safe L2 orderbook with BTreeMap for price levels.
//!
//! ## Ordering Invariants
//!
//! - Bids: sorted by price descending (highest first)
//! - Asks: sorted by price ascending (lowest first)
//!
//! ## No Arc<Mutex> in Hot Path
//!
//! The L2Book is designed to be used by a single writer (SymbolActor).
//! External access goes through channels, not shared memory.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::orderbook::l2_update::Side;

/// Price type - scaled integer for deterministic ordering
pub type Price = u64;
/// Quantity type
pub type Quantity = f64;

/// A single price level
#[derive(Debug, Clone)]
pub struct Level {
    pub price: Price,
    pub quantity: Quantity,
}

impl Level {
    pub fn new(price: Price, quantity: Quantity) -> Option<Self> {
        if !quantity.is_finite() || quantity < 0.0 {
            return None;
        }
        Some(Self { price, quantity })
    }
}

/// Wrapper for f64 that implements Ord for price sorting
/// Prices are stored as scaled integers (e.g., tick_size precision)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SortedPrice(u64);

impl SortedPrice {
    pub fn new(price: u64) -> Self {
        Self(price)
    }

    pub fn from_f64(price: f64, scale: u32) -> Option<Self> {
        if !price.is_finite() || price <= 0.0 {
            return None;
        }
        let scaled = (price * (10u64.pow(scale) as f64)) as u64;
        Some(Self(scaled))
    }

    pub fn to_f64(self, scale: u32) -> f64 {
        self.0 as f64 / (10u64.pow(scale) as f64)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl Eq for SortedPrice {}

impl PartialOrd for SortedPrice {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SortedPrice {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

/// L2 Orderbook state
///
/// # Invariants
///
/// 1. No crossed book (best_bid < best_ask always)
/// 2. All prices are positive and finite
/// 3. All quantities are non-negative and finite
/// 4. Bids sorted by price descending
/// 5. Asks sorted by price ascending
#[derive(Debug, Clone)]
pub struct L2Book {
    /// Bids: SortedPrice -> quantity (sorted descending by price)
    bids: BTreeMap<SortedPrice, Quantity>,
    /// Asks: SortedPrice -> quantity (sorted ascending by price)
    asks: BTreeMap<SortedPrice, Quantity>,
    /// Exchange timestamp (nanoseconds)
    pub exchange_ts_ns: u64,
    /// Receive timestamp (nanoseconds)
    pub recv_ts_ns: u64,
    /// Last update ID (exchange-specific sequence)
    pub last_update_id: u64,
    /// Whether book is valid (not crossed, not empty)
    pub is_valid: bool,
    /// Symbol for this book
    pub symbol: String,
    /// Exchange for this book
    pub exchange: String,
    /// Price scale (number of decimal places)
    price_scale: u32,
}

impl L2Book {
    /// Create a new empty L2Book
    pub fn new(symbol: &str, exchange: &str) -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            exchange_ts_ns: 0,
            recv_ts_ns: 0,
            last_update_id: 0,
            is_valid: false,
            symbol: symbol.to_string(),
            exchange: exchange.to_string(),
            price_scale: 8, // default scale
        }
    }

    /// Create with specific price scale
    pub fn with_scale(symbol: &str, exchange: &str, price_scale: u32) -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            exchange_ts_ns: 0,
            recv_ts_ns: 0,
            last_update_id: 0,
            is_valid: false,
            symbol: symbol.to_string(),
            exchange: exchange.to_string(),
            price_scale,
        }
    }

    /// Get price scale
    pub fn price_scale(&self) -> u32 {
        self.price_scale
    }

    /// Apply a snapshot update (replaces all levels)
    pub fn apply_snapshot(&mut self, snapshot: &OrderBookSnapshot) -> Result<(), BookError> {
        // Clear existing state
        self.bids.clear();
        self.asks.clear();

        // Validate and insert bids (sorted descending by price)
        let mut bid_prices: Vec<u64> = Vec::new();
        for &(price, qty) in &snapshot.bids {
            if !price.is_finite() || price <= 0.0 {
                return Err(BookError::InvalidPrice(price));
            }
            if !qty.is_finite() || qty < 0.0 {
                return Err(BookError::InvalidQuantity(qty));
            }
            let scaled = (price * (10u64.pow(self.price_scale) as f64)) as u64;
            if bid_prices.contains(&scaled) {
                return Err(BookError::DuplicatePrice(scaled));
            }
            bid_prices.push(scaled);
            self.bids.insert(SortedPrice(scaled), qty);
        }

        // Validate and insert asks (sorted ascending by price)
        let mut ask_prices: Vec<u64> = Vec::new();
        for &(price, qty) in &snapshot.asks {
            if !price.is_finite() || price <= 0.0 {
                return Err(BookError::InvalidPrice(price));
            }
            if !qty.is_finite() || qty < 0.0 {
                return Err(BookError::InvalidQuantity(qty));
            }
            let scaled = (price * (10u64.pow(self.price_scale) as f64)) as u64;
            if ask_prices.contains(&scaled) {
                return Err(BookError::DuplicatePrice(scaled));
            }
            ask_prices.push(scaled);
            self.asks.insert(SortedPrice(scaled), qty);
        }

        // Update metadata
        self.exchange_ts_ns = snapshot.exchange_ts_ns;
        self.recv_ts_ns = snapshot.recv_ts_ns;
        self.last_update_id = snapshot.update_id;

        // Validate book is not crossed
        self.is_valid = self.validate_book_state();
        Ok(())
    }

    /// Apply an incremental update
    pub fn apply_update(
        &mut self,
        price: f64,
        quantity: Quantity,
        side: Side,
    ) -> Result<(), BookError> {
        // Validate inputs
        if !price.is_finite() || price <= 0.0 {
            return Err(BookError::InvalidPrice(price));
        }
        if !quantity.is_finite() || quantity < 0.0 {
            return Err(BookError::InvalidQuantity(quantity));
        }

        let scaled = (price * (10u64.pow(self.price_scale) as f64)) as u64;

        match side {
            Side::Bid => {
                if quantity == 0.0 {
                    self.bids.remove(&SortedPrice(scaled));
                } else {
                    self.bids.insert(SortedPrice(scaled), quantity);
                }
            }
            Side::Ask => {
                if quantity == 0.0 {
                    self.asks.remove(&SortedPrice(scaled));
                } else {
                    self.asks.insert(SortedPrice(scaled), quantity);
                }
            }
        }

        self.is_valid = self.validate_book_state();
        Ok(())
    }

    /// Validate book state (not crossed, not empty)
    fn validate_book_state(&self) -> bool {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => bid.0 < ask.0,
            _ => false, // Empty book is invalid
        }
    }

    /// Get best bid price and quantity (highest bid first)
    pub fn best_bid(&self) -> Option<(Price, Quantity)> {
        self.bids
            .iter()
            .next_back() // Highest price (descending order in BTreeMap)
            .map(|(p, q)| (p.as_u64(), *q))
    }

    /// Get best ask price and quantity (lowest ask first)
    pub fn best_ask(&self) -> Option<(Price, Quantity)> {
        self.asks
            .iter()
            .next() // Lowest price (ascending order in BTreeMap)
            .map(|(p, q)| (p.as_u64(), *q))
    }

    /// Get top N bids (highest first)
    pub fn top_n_bids(&self, n: usize) -> Vec<(Price, Quantity)> {
        self.bids
            .iter()
            .rev() // Descending order
            .take(n)
            .map(|(p, q)| (p.as_u64(), *q))
            .collect()
    }

    /// Get top N asks (lowest first)
    pub fn top_n_asks(&self, n: usize) -> Vec<(Price, Quantity)> {
        self.asks
            .iter()
            .take(n)
            .map(|(p, q)| (p.as_u64(), *q))
            .collect()
    }

    /// Get mid price (as scaled integer)
    pub fn mid_price(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => {
                let bid_f = bid.0 as f64 / (10u64.pow(self.price_scale) as f64);
                let ask_f = ask.0 as f64 / (10u64.pow(self.price_scale) as f64);
                Some((bid_f + ask_f) / 2.0)
            }
            _ => None,
        }
    }

    /// Get spread (as scaled integer)
    pub fn spread(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => {
                let bid_f = bid.0 as f64 / (10u64.pow(self.price_scale) as f64);
                let ask_f = ask.0 as f64 / (10u64.pow(self.price_scale) as f64);
                Some(ask_f - bid_f)
            }
            _ => None,
        }
    }

    /// Get spread as basis points
    pub fn spread_bps(&self) -> Option<f64> {
        match self.mid_price() {
            Some(mid) if mid > 0.0 => self.spread().map(|s| s / mid * 10000.0),
            _ => None,
        }
    }

    /// Check if book is crossed (bid >= ask)
    pub fn is_crossed(&self) -> bool {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => bid.0 >= ask.0,
            _ => false,
        }
    }

    /// Check if book is empty
    pub fn is_empty(&self) -> bool {
        self.bids.is_empty() && self.asks.is_empty()
    }

    /// Total bid volume at top N levels
    pub fn bid_volume_top_n(&self, n: usize) -> f64 {
        self.top_n_bids(n).iter().map(|(_, q)| q).sum()
    }

    /// Total ask volume at top N levels
    pub fn ask_volume_top_n(&self, n: usize) -> f64 {
        self.top_n_asks(n).iter().map(|(_, q)| q).sum()
    }

    /// Total bid volume
    pub fn total_bid_volume(&self) -> f64 {
        self.bids.values().sum()
    }

    /// Total ask volume
    pub fn total_ask_volume(&self) -> f64 {
        self.asks.values().sum()
    }

    /// Get number of bid levels
    pub fn num_bid_levels(&self) -> usize {
        self.bids.len()
    }

    /// Get number of ask levels
    pub fn num_ask_levels(&self) -> usize {
        self.asks.len()
    }
}

/// Order book snapshot from exchange
#[derive(Debug, Clone)]
pub struct OrderBookSnapshot {
    pub exchange: String,
    pub symbol: String,
    pub bids: Vec<(f64, f64)>,
    pub asks: Vec<(f64, f64)>,
    pub exchange_ts_ns: u64,
    pub recv_ts_ns: u64,
    pub update_id: u64,
}

impl OrderBookSnapshot {
    /// Create a new snapshot
    pub fn new(
        exchange: &str,
        symbol: &str,
        bids: Vec<(f64, f64)>,
        asks: Vec<(f64, f64)>,
        exchange_ts_ns: u64,
        recv_ts_ns: u64,
        update_id: u64,
    ) -> Self {
        Self {
            exchange: exchange.to_string(),
            symbol: symbol.to_string(),
            bids,
            asks,
            exchange_ts_ns,
            recv_ts_ns,
            update_id,
        }
    }
}

/// Symbol snapshot for OBI calculation
#[derive(Debug, Clone)]
pub struct SymbolSnapshot {
    pub exchange: String,
    pub symbol: String,
    pub exchange_ts_ns: u64,
    pub recv_ts_ns: u64,
    pub mid: Option<f64>,
    pub obi_1: Option<f64>,
    pub obi_3: Option<f64>,
    pub obi_5: Option<f64>,
    pub obi_10: Option<f64>,
    pub depth_5bps: Option<f64>,
    pub depth_10bps: Option<f64>,
    pub microprice: Option<f64>,
    pub spread: Option<f64>,
    pub spread_bps: Option<f64>,
    pub num_bid_levels: usize,
    pub num_ask_levels: usize,
    pub is_valid: bool,
}

impl SymbolSnapshot {
    /// Create from L2Book
    pub fn from_book(book: &L2Book) -> Self {
        let mid = book.mid_price();
        let bid_vol1 = book.bid_volume_top_n(1);
        let ask_vol1 = book.ask_volume_top_n(1);
        let bid_vol3 = book.bid_volume_top_n(3);
        let ask_vol3 = book.ask_volume_top_n(3);
        let bid_vol5 = book.bid_volume_top_n(5);
        let ask_vol5 = book.ask_volume_top_n(5);
        let bid_vol10 = book.bid_volume_top_n(10);
        let ask_vol10 = book.ask_volume_top_n(10);

        let obi_1 = calculate_obi(bid_vol1, ask_vol1);
        let obi_3 = calculate_obi(bid_vol3, ask_vol3);
        let obi_5 = calculate_obi(bid_vol5, ask_vol5);
        let obi_10 = calculate_obi(bid_vol10, ask_vol10);

        let depth_5bps = book
            .mid_price()
            .map(|m| calculate_depth_at_bps(book, m, 0.0005));
        let depth_10bps = book
            .mid_price()
            .map(|m| calculate_depth_at_bps(book, m, 0.001));

        let microprice = calculate_microprice(book);
        let spread = book.spread();
        let spread_bps = book.spread_bps();

        Self {
            exchange: book.exchange.clone(),
            symbol: book.symbol.clone(),
            exchange_ts_ns: book.exchange_ts_ns,
            recv_ts_ns: book.recv_ts_ns,
            mid,
            obi_1,
            obi_3,
            obi_5,
            obi_10,
            depth_5bps,
            depth_10bps,
            microprice,
            spread,
            spread_bps,
            num_bid_levels: book.num_bid_levels(),
            num_ask_levels: book.num_ask_levels(),
            is_valid: book.is_valid && !book.is_empty(),
        }
    }
}

/// Calculate OBI: (bid_vol - ask_vol) / (bid_vol + ask_vol)
fn calculate_obi(bid_vol: f64, ask_vol: f64) -> Option<f64> {
    let sum = bid_vol + ask_vol;
    if sum <= 0.0 {
        return None;
    }
    let obi = (bid_vol - ask_vol) / sum;
    if !obi.is_finite() {
        return None;
    }
    Some(obi)
}

/// Calculate depth at a given basis point offset from mid
fn calculate_depth_at_bps(book: &L2Book, mid: f64, bps: f64) -> f64 {
    let scale = book.price_scale();
    let lower = mid * (1.0 - bps);
    let upper = mid * (1.0 + bps);

    let lower_scaled = (lower * (10u64.pow(scale) as f64)) as u64;
    let mid_scaled = (mid * (10u64.pow(scale) as f64)) as u64;
    let upper_scaled = (upper * (10u64.pow(scale) as f64)) as u64;

    let bid_depth: f64 = book
        .bids
        .range(SortedPrice(lower_scaled)..SortedPrice(mid_scaled))
        .map(|(_, q)| q)
        .sum();

    let ask_depth: f64 = book
        .asks
        .range(SortedPrice(mid_scaled)..SortedPrice(upper_scaled))
        .map(|(_, q)| q)
        .sum();

    bid_depth + ask_depth
}

/// Calculate microprice: weighted average of bid/ask prices by volume
fn calculate_microprice(book: &L2Book) -> Option<f64> {
    let scale = book.price_scale();
    let bid_vol1 = book.bid_volume_top_n(1);
    let ask_vol1 = book.ask_volume_top_n(1);
    let total_vol = bid_vol1 + ask_vol1;

    if total_vol <= 0.0 {
        return None;
    }

    let bid_p = book.best_bid()?.0;
    let ask_p = book.best_ask()?.0;

    let bid_f = bid_p as f64 / (10u64.pow(scale) as f64);
    let ask_f = ask_p as f64 / (10u64.pow(scale) as f64);

    // Microprice = (bid_p * ask_vol + ask_p * bid_vol) / (bid_vol + ask_vol)
    let microprice = (bid_f * ask_vol1 + ask_f * bid_vol1) / total_vol;

    if !microprice.is_finite() {
        return None;
    }

    Some(microprice)
}

/// Book error types
#[derive(Debug, Clone)]
pub enum BookError {
    InvalidPrice(f64),
    InvalidQuantity(f64),
    CrossedBook,
    EmptyBook,
    DuplicatePrice(u64),
}

impl std::fmt::Display for BookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BookError::InvalidPrice(p) => write!(f, "Invalid price: {}", p),
            BookError::InvalidQuantity(q) => write!(f, "Invalid quantity: {}", q),
            BookError::CrossedBook => write!(f, "Book is crossed (bid >= ask)"),
            BookError::EmptyBook => write!(f, "Book is empty"),
            BookError::DuplicatePrice(p) => write!(f, "Duplicate price level: {}", p),
        }
    }
}

impl std::error::Error for BookError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orderbook::l2_update::Side;

    #[test]
    fn test_empty_book() {
        let book = L2Book::new("BTCUSDT", "binance");
        assert!(book.is_empty());
        assert!(!book.is_valid);
        assert!(book.best_bid().is_none());
        assert!(book.best_ask().is_none());
    }

    #[test]
    fn test_single_level() {
        let mut book = L2Book::with_scale("BTCUSDT", "binance", 2);
        // prices as f64: 100.0, 101.0 -> scaled: 10000, 10100
        book.apply_update(100.0, 1.0, Side::Bid).unwrap();
        book.apply_update(101.0, 1.0, Side::Ask).unwrap();
        assert!(!book.is_empty());
        assert!(book.is_valid);

        let (bid_p, bid_q) = book.best_bid().unwrap();
        assert_eq!(bid_p, 10000); // scaled
        assert_eq!(bid_q, 1.0);

        let (ask_p, ask_q) = book.best_ask().unwrap();
        assert_eq!(ask_p, 10100); // scaled
        assert_eq!(ask_q, 1.0);

        assert_eq!(book.mid_price(), Some(100.5));
        assert_eq!(book.spread(), Some(1.0));
    }

    #[test]
    fn test_multiple_levels() {
        let mut book = L2Book::with_scale("BTCUSDT", "binance", 2);

        // Add bids: 100.0, 99.0, 98.0
        book.apply_update(100.0, 1.0, Side::Bid).unwrap();
        book.apply_update(99.0, 2.0, Side::Bid).unwrap();
        book.apply_update(98.0, 3.0, Side::Bid).unwrap();

        // Add asks: 101.0, 102.0
        book.apply_update(101.0, 1.5, Side::Ask).unwrap();
        book.apply_update(102.0, 2.5, Side::Ask).unwrap();

        // Top 3 bids should be 100, 99, 98 (highest first)
        let top3 = book.top_n_bids(3);
        assert_eq!(top3.len(), 3);
        assert_eq!(top3[0].0, 10000); // 100.0 scaled
        assert_eq!(top3[1].0, 9900); // 99.0 scaled
        assert_eq!(top3[2].0, 9800); // 98.0 scaled

        // Top 3 asks should be 101, 102 (lowest first)
        let top3_asks = book.top_n_asks(3);
        assert_eq!(top3_asks.len(), 2); // Only 2 ask levels
        assert_eq!(top3_asks[0].0, 10100); // 101.0 scaled
        assert_eq!(top3_asks[1].0, 10200); // 102.0 scaled
    }

    #[test]
    fn test_crossed_book_rejected() {
        let mut book = L2Book::with_scale("BTCUSDT", "binance", 2);
        book.apply_update(100.0, 1.0, Side::Bid).unwrap();
        book.apply_update(99.0, 1.0, Side::Ask).unwrap(); // crossed!

        // Bid >= Ask means crossed book
        assert!(!book.is_valid);
    }

    #[test]
    fn test_remove_level() {
        let mut book = L2Book::with_scale("BTCUSDT", "binance", 2);
        book.apply_update(100.0, 1.0, Side::Bid).unwrap();
        assert!(book.best_bid().is_some());

        // Remove by setting quantity to 0
        book.apply_update(100.0, 0.0, Side::Bid).unwrap();
        assert!(book.best_bid().is_none());
    }

    #[test]
    fn test_invalid_price_rejected() {
        let mut book = L2Book::with_scale("BTCUSDT", "binance", 2);
        assert!(book.apply_update(-100.0, 1.0, Side::Bid).is_err());
        assert!(book.apply_update(0.0, 1.0, Side::Bid).is_err());
        assert!(book.apply_update(f64::NAN, 1.0, Side::Bid).is_err());
        assert!(book.apply_update(f64::INFINITY, 1.0, Side::Bid).is_err());
    }

    #[test]
    fn test_invalid_quantity_rejected() {
        let mut book = L2Book::with_scale("BTCUSDT", "binance", 2);
        assert!(book.apply_update(100.0, -1.0, Side::Bid).is_err());
        assert!(book.apply_update(100.0, f64::NAN, Side::Bid).is_err());
        assert!(book.apply_update(100.0, f64::INFINITY, Side::Bid).is_err());
    }

    #[test]
    fn test_volume_calculation() {
        let mut book = L2Book::with_scale("BTCUSDT", "binance", 2);
        book.apply_update(100.0, 1.0, Side::Bid).unwrap();
        book.apply_update(99.0, 2.0, Side::Bid).unwrap();
        book.apply_update(101.0, 1.5, Side::Ask).unwrap();
        book.apply_update(102.0, 2.5, Side::Ask).unwrap();

        assert_eq!(book.bid_volume_top_n(2), 3.0); // 1 + 2
        assert_eq!(book.ask_volume_top_n(2), 4.0); // 1.5 + 2.5
        assert_eq!(book.total_bid_volume(), 3.0);
        assert_eq!(book.total_ask_volume(), 4.0);
    }

    #[test]
    fn test_spread_bps() {
        let mut book = L2Book::with_scale("BTCUSDT", "binance", 2);
        book.apply_update(100.0, 1.0, Side::Bid).unwrap();
        book.apply_update(100.01, 1.0, Side::Ask).unwrap();

        let spread_bps = book.spread_bps().unwrap();
        // Spread is 0.01 on 100.005 mid = ~1 bps
        assert!((spread_bps - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_snapshot() {
        let mut book = L2Book::with_scale("BTCUSDT", "binance", 2);
        let snapshot = OrderBookSnapshot::new(
            "binance",
            "BTCUSDT",
            vec![(100.0, 1.0), (99.0, 2.0)],
            vec![(101.0, 1.0), (102.0, 2.0)],
            1000,
            2000,
            42,
        );

        book.apply_snapshot(&snapshot).unwrap();
        assert!(book.is_valid);
        assert_eq!(book.exchange_ts_ns, 1000);
        assert_eq!(book.recv_ts_ns, 2000);
        assert_eq!(book.last_update_id, 42);
    }

    #[test]
    fn test_obi_top_levels_different() {
        let mut book = L2Book::with_scale("BTCUSDT", "binance", 2);

        // Top bid: 100.0 qty=10
        book.apply_update(100.0, 10.0, Side::Bid).unwrap();
        // Bid 99.0 qty=1
        book.apply_update(99.0, 1.0, Side::Bid).unwrap();
        // Bid 98.0 qty=1
        book.apply_update(98.0, 1.0, Side::Bid).unwrap();

        // Top ask: 101.0 qty=1
        book.apply_update(101.0, 1.0, Side::Ask).unwrap();
        // Ask 102.0 qty=1
        book.apply_update(102.0, 1.0, Side::Ask).unwrap();
        // Ask 103.0 qty=10
        book.apply_update(103.0, 10.0, Side::Ask).unwrap();

        let snap = SymbolSnapshot::from_book(&book);

        // OBI1: (10-1)/(10+1) = 0.818
        // OBI3: (12-3)/(12+3) = 0.6
        // OBI5+: same as OBI3 since only 3 bids and 3 asks
        assert!(snap.obi_1.is_some());
        assert!(snap.obi_3.is_some());

        // They should be different due to volume distribution
        let obi1 = snap.obi_1.unwrap();
        let obi3 = snap.obi_3.unwrap();
        assert!(
            (obi1 - obi3).abs() > 0.01,
            "OBI1 {} should differ from OBI3 {}",
            obi1,
            obi3
        );
    }

    #[test]
    fn test_deterministic_price_encoding_different_scales() {
        // BTCUSDT with scale 8: 50000.12345678 -> scaled = 5000012345678
        let mut btc_book = L2Book::with_scale("BTCUSDT", "binance", 8);
        btc_book
            .apply_update(50000.12345678, 1.0, Side::Bid)
            .unwrap();

        // SHIBUSDT with scale 10: 0.00001234567 -> scaled = 1234567
        let mut shib_book = L2Book::with_scale("SHIBUSDT", "binance", 10);
        shib_book
            .apply_update(0.00001234567, 1.0, Side::Bid)
            .unwrap();

        // Verify internal representation is correct
        let (btc_price, _) = btc_book.best_bid().unwrap();
        let (shib_price, _) = shib_book.best_bid().unwrap();

        // BTC: 50000.12345678 * 10^8 = 5000012345678
        assert_eq!(btc_price, 5000012345678u64);
        // SHIB: 0.00001234567 * 10^10 = 123456.7 -> truncated to 123456
        assert_eq!(shib_price, 123456u64);

        // Verify price roundtrip through the scaled representation
        // (mid_price() requires both sides; these books hold a single bid)
        let btc_roundtrip = SortedPrice::new(btc_price).to_f64(8);
        let shib_roundtrip = SortedPrice::new(shib_price).to_f64(10);
        assert!((btc_roundtrip - 50000.12345678).abs() < 1e-6);
        assert!((shib_roundtrip - 0.00001234567).abs() < 1e-8);
    }

    #[test]
    fn test_same_price_different_scales_gives_different_internal() {
        // Price 100.0 with scale 2: 100.00 -> scaled = 10000
        let mut book1 = L2Book::with_scale("PAIR1", "binance", 2);
        book1.apply_update(100.0, 1.0, Side::Bid).unwrap();

        // Price 100.0 with scale 4: 100.0000 -> scaled = 1000000
        let mut book2 = L2Book::with_scale("PAIR2", "binance", 4);
        book2.apply_update(100.0, 1.0, Side::Bid).unwrap();

        let (p1, _) = book1.best_bid().unwrap();
        let (p2, _) = book2.best_bid().unwrap();

        // Internal representations must differ
        assert_ne!(p1, p2);
        // But they maintain correct ordering when sorted
        assert!(p1 < p2);
    }

    #[test]
    fn test_obi_zero_denominator() {
        let book = L2Book::with_scale("BTCUSDT", "binance", 8);
        // Empty book has no OBI
        let snap = SymbolSnapshot::from_book(&book);
        assert!(snap.obi_1.is_none());
        assert!(snap.obi_3.is_none());
    }
}
