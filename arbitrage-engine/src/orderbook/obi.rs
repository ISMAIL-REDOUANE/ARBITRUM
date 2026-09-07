//! # OBI (Orderbook Imbalance) Calculations
//!
//! OBI_N = (BidVolume_N - AskVolume_N) / (BidVolume_N + AskVolume_N)
//!
//! ## Interpretation
//!
//! - OBI = +1.0: All volume on bid side (maximum buying pressure)
//! - OBI = 0.0: Balanced orderbook
//! - OBI = -1.0: All volume on ask side (maximum selling pressure)

use serde::{Deserialize, Serialize};

/// OBI calculation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObiConfig {
    /// Number of levels to use for OBI calculation
    pub obi_levels: Vec<usize>,
    /// Basis points for depth calculation (5 = 5 bps = 0.05%)
    pub depth_bps: Vec<f64>,
    /// Microprice weighting factor
    pub microprice_levels: usize,
}

impl Default for ObiConfig {
    fn default() -> Self {
        Self {
            obi_levels: vec![1, 3, 5, 10],
            depth_bps: vec![5.0, 10.0], // basis points
            microprice_levels: 1,
        }
    }
}

impl ObiConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_levels(mut self, levels: Vec<usize>) -> Self {
        self.obi_levels = levels;
        self
    }

    pub fn with_depth_bps(mut self, bps: Vec<f64>) -> Self {
        self.depth_bps = bps;
        self
    }
}

/// OBI Snapshot containing all calculated metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObiSnapshot {
    /// Exchange
    pub exchange: String,
    /// Symbol
    pub symbol: String,
    /// Exchange timestamp (ns)
    pub exchange_ts_ns: u64,
    /// Receive timestamp (ns)
    pub recv_ts_ns: u64,
    /// Mid price
    pub mid: Option<f64>,
    /// Spread
    pub spread: Option<f64>,
    /// Spread in basis points
    pub spread_bps: Option<f64>,
    /// OBI at level 1
    pub obi_1: Option<f64>,
    /// OBI at level 3
    pub obi_3: Option<f64>,
    /// OBI at level 5
    pub obi_5: Option<f64>,
    /// OBI at level 10
    pub obi_10: Option<f64>,
    /// Depth at 5 basis points
    pub depth_5bps: Option<f64>,
    /// Depth at 10 basis points
    pub depth_10bps: Option<f64>,
    /// Microprice
    pub microprice: Option<f64>,
    /// Microprice deviation from mid (bps)
    pub microprice_deviation_bps: Option<f64>,
    /// Number of bid levels
    pub num_bid_levels: usize,
    /// Number of ask levels
    pub num_ask_levels: usize,
    /// Whether book is valid (not crossed, not empty)
    pub is_valid: bool,
}

impl ObiSnapshot {
    /// Calculate OBI snapshot from bid/ask volumes
    pub fn calculate(
        exchange: &str,
        symbol: &str,
        exchange_ts_ns: u64,
        recv_ts_ns: u64,
        bids: &[(f64, f64)], // (price, quantity) sorted by price
        asks: &[(f64, f64)],
        _config: &ObiConfig,
    ) -> Self {
        let mid = calculate_mid(bids, asks);
        let spread = calculate_spread(bids, asks);
        let spread_bps = spread.and_then(|s| {
            mid.map(|m| if m > 0.0 { s / m * 10000.0 } else { 0.0 })
        });

        let _vol1 = volume_at_level(bids, 1) + volume_at_level(asks, 1);
        let _vol3 = volume_at_level(bids, 3) + volume_at_level(asks, 3);
        let _vol5 = volume_at_level(bids, 5) + volume_at_level(asks, 5);
        let _vol10 = volume_at_level(bids, 10) + volume_at_level(asks, 10);

        let bid_vol1 = volume_at_level(bids, 1);
        let ask_vol1 = volume_at_level(asks, 1);
        let bid_vol3 = volume_at_level(bids, 3);
        let ask_vol3 = volume_at_level(asks, 3);
        let bid_vol5 = volume_at_level(bids, 5);
        let ask_vol5 = volume_at_level(asks, 5);
        let bid_vol10 = volume_at_level(bids, 10);
        let ask_vol10 = volume_at_level(asks, 10);

        let obi_1 = calculate_obi(bid_vol1, ask_vol1);
        let obi_3 = calculate_obi(bid_vol3, ask_vol3);
        let obi_5 = calculate_obi(bid_vol5, ask_vol5);
        let obi_10 = calculate_obi(bid_vol10, ask_vol10);

        let depth_5bps = mid.map(|m| calculate_depth_at_bps(bids, asks, m, 0.0005));
        let depth_10bps = mid.map(|m| calculate_depth_at_bps(bids, asks, m, 0.001));

        let microprice = calculate_microprice(bids, asks);
        let microprice_deviation_bps = microprice.zip(mid).map(|(mp, m)| {
            if m > 0.0 { (mp - m) / m * 10000.0 } else { 0.0 }
        });

        let is_valid = !bids.is_empty() && !asks.is_empty() &&
            bids.first().map(|(p, _)| asks.first().map(|(ap, _)| p < ap).unwrap_or(false)).unwrap_or(false);

        Self {
            exchange: exchange.to_string(),
            symbol: symbol.to_string(),
            exchange_ts_ns,
            recv_ts_ns,
            mid,
            spread,
            spread_bps,
            obi_1,
            obi_3,
            obi_5,
            obi_10,
            depth_5bps,
            depth_10bps,
            microprice,
            microprice_deviation_bps,
            num_bid_levels: bids.len(),
            num_ask_levels: asks.len(),
            is_valid,
        }
    }

    /// Check if the OBI signal is stale (based on timestamp age)
    pub fn is_stale(&self, max_age_ms: u64) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let age_ms = now.saturating_sub(self.recv_ts_ns / 1_000_000);
        age_ms > max_age_ms
    }
}

/// Calculate OBI: (bid_vol - ask_vol) / (bid_vol + ask_vol)
#[inline]
pub fn calculate_obi(bid_vol: f64, ask_vol: f64) -> Option<f64> {
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

/// Calculate mid price
#[inline]
pub fn calculate_mid(bids: &[(f64, f64)], asks: &[(f64, f64)]) -> Option<f64> {
    match (bids.first(), asks.first()) {
        (Some((best_bid, _)), Some((best_ask, _))) => Some((*best_bid + *best_ask) / 2.0),
        _ => None,
    }
}

/// Calculate spread
#[inline]
pub fn calculate_spread(bids: &[(f64, f64)], asks: &[(f64, f64)]) -> Option<f64> {
    match (bids.first(), asks.first()) {
        (Some((best_bid, _)), Some((best_ask, _))) => Some(best_ask - best_bid),
        _ => None,
    }
}

/// Calculate total volume at top N levels
#[inline]
pub fn volume_at_level(levels: &[(f64, f64)], n: usize) -> f64 {
    levels.iter().take(n).map(|(_, q)| q).sum()
}

/// Calculate depth at a given basis point offset from mid
pub fn calculate_depth_at_bps(bids: &[(f64, f64)], asks: &[(f64, f64)], mid: f64, bps: f64) -> f64 {
    let lower = mid * (1.0 - bps);
    let upper = mid * (1.0 + bps);

    // Sum bids between lower and mid
    let bid_depth: f64 = bids.iter()
        .take_while(|(p, _)| *p >= lower)
        .take_while(|(p, _)| *p < mid)
        .map(|(_, q)| q)
        .sum();

    // Sum asks between mid and upper
    let ask_depth: f64 = asks.iter()
        .take_while(|(p, _)| *p <= upper)
        .take_while(|(p, _)| *p > mid)
        .map(|(_, q)| q)
        .sum();

    bid_depth + ask_depth
}

/// Calculate microprice: volume-weighted average price
///
/// Microprice = (bid_price * ask_vol + ask_price * bid_vol) / (bid_vol + ask_vol)
pub fn calculate_microprice(bids: &[(f64, f64)], asks: &[(f64, f64)]) -> Option<f64> {
    let bid_vol = volume_at_level(bids, 1);
    let ask_vol = volume_at_level(asks, 1);
    let total_vol = bid_vol + ask_vol;

    if total_vol <= 0.0 {
        return None;
    }

    let (Some((bid_p, _)), Some((ask_p, _))) = (bids.first(), asks.first()) else {
        return None;
    };

    let microprice = (bid_p * ask_vol + ask_p * bid_vol) / total_vol;

    if !microprice.is_finite() {
        return None;
    }

    Some(microprice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_obi_calculation() {
        // All volume on bid side
        assert_eq!(calculate_obi(100.0, 0.0), Some(1.0));

        // All volume on ask side
        assert_eq!(calculate_obi(0.0, 100.0), Some(-1.0));

        // Balanced
        assert_eq!(calculate_obi(50.0, 50.0), Some(0.0));

        // Zero total volume
        assert_eq!(calculate_obi(0.0, 0.0), None);
    }

    #[test]
    fn test_mid_calculation() {
        let bids = vec![(100.0, 1.0), (99.0, 2.0)];
        let asks = vec![(101.0, 1.0), (102.0, 2.0)];

        assert_eq!(calculate_mid(&bids, &asks), Some(100.5));
    }

    #[test]
    fn test_spread_calculation() {
        let bids = vec![(100.0, 1.0)];
        let asks = vec![(101.0, 1.0)];

        assert_eq!(calculate_spread(&bids, &asks), Some(1.0));
    }

    #[test]
    fn test_volume_at_level() {
        let levels = vec![(100.0, 1.0), (99.0, 2.0), (98.0, 3.0)];

        assert_eq!(volume_at_level(&levels, 1), 1.0);
        assert_eq!(volume_at_level(&levels, 2), 3.0);
        assert_eq!(volume_at_level(&levels, 10), 6.0); // Cap at total
    }

    #[test]
    fn test_microprice() {
        let bids = vec![(100.0, 1.0)];
        let asks = vec![(102.0, 1.0)];

        // Microprice = (100 * 1 + 102 * 1) / (1 + 1) = 101
        assert_eq!(calculate_microprice(&bids, &asks), Some(101.0));
    }

    #[test]
    fn test_depth_at_bps() {
        let bids = vec![(100.0, 1.0), (99.5, 2.0), (99.0, 3.0)];
        let asks = vec![(100.5, 1.0), (101.0, 2.0), (101.5, 3.0)];

        // Mid = 100.25, 5bps = 0.0005 (0.05%) = 0.050125 offset
        // Range = [100.199875, 100.300125] — no levels fall inside
        // (bids start at 100.0 < lower bound, asks start at 100.5 > upper)
        let depth = calculate_depth_at_bps(&bids, &asks, 100.25, 0.0005);
        assert_eq!(depth, 0.0);
    }

    #[test]
    fn test_obi_snapshot_validity() {
        let config = ObiConfig::default();
        let bids = vec![(100.0, 1.0), (99.0, 2.0)];
        let asks = vec![(101.0, 1.0), (102.0, 2.0)];

        let snapshot = ObiSnapshot::calculate(
            "binance",
            "BTCUSDT",
            1000,
            2000,
            &bids,
            &asks,
            &config,
        );

        assert!(snapshot.is_valid);
        assert!(snapshot.mid.is_some());
        assert!(snapshot.obi_1.is_some());
    }

    #[test]
    fn test_obi_snapshot_crossed_book() {
        let config = ObiConfig::default();
        let bids = vec![(102.0, 1.0)]; // Bid >= Ask = crossed
        let asks = vec![(101.0, 1.0)];

        let snapshot = ObiSnapshot::calculate(
            "binance",
            "BTCUSDT",
            1000,
            2000,
            &bids,
            &asks,
            &config,
        );

        assert!(!snapshot.is_valid);
    }
}
