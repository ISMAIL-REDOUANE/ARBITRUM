//! # Health Monitoring
//!
//! Independent L1 and L2 health monitoring with staleness detection.
//!
//! ## Health State Model
//!
//! ```text
//! L1: Connected/Stale/Disconnected
//! L2: Connected/Stale/Disconnected
//!
//! Combined: Healthy / Degraded / Critical
//! ```
//!
//! ## Staleness Rules
//!
//! - L1 stale: No new blocks within expected interval
//! - L2 stale: No new orderbook updates within expected interval
//! - L1 disconnected: WebSocket connection lost
//! - L2 disconnected: WebSocket connection lost

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Health state for a data feed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FeedHealth {
    /// Fully healthy
    Healthy,
    /// Connected but data is stale
    Stale,
    /// Connection lost
    Disconnected,
    /// Never received data
    #[default]
    Unknown,
}

impl FeedHealth {
    pub fn is_usable(&self) -> bool {
        matches!(self, FeedHealth::Healthy | FeedHealth::Stale)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            FeedHealth::Healthy => "healthy",
            FeedHealth::Stale => "stale",
            FeedHealth::Disconnected => "disconnected",
            FeedHealth::Unknown => "unknown",
        }
    }
}

/// Connection health state for an exchange
#[derive(Debug)]
pub struct ConnectionHealth {
    /// Exchange name
    pub exchange: String,
    /// L1 feed health
    l1_health: FeedHealth,
    /// L2 feed health
    l2_health: FeedHealth,
    /// Last L1 message timestamp (ns)
    last_l1_ts_ns: AtomicU64,
    /// Last L2 message timestamp (ns)
    last_l2_ts_ns: AtomicU64,
    /// Connection established timestamp (ns)
    connected_at_ns: AtomicU64,
    /// Total messages received
    messages_received: AtomicU64,
    /// Total messages dropped
    messages_dropped: AtomicU64,
}

impl ConnectionHealth {
    pub fn new(exchange: &str) -> Self {
        Self {
            exchange: exchange.to_string(),
            l1_health: FeedHealth::default(),
            l2_health: FeedHealth::default(),
            last_l1_ts_ns: AtomicU64::new(0),
            last_l2_ts_ns: AtomicU64::new(0),
            connected_at_ns: AtomicU64::new(0),
            messages_received: AtomicU64::new(0),
            messages_dropped: AtomicU64::new(0),
        }
    }

    pub fn l1_health(&self) -> FeedHealth {
        self.l1_health
    }

    pub fn l2_health(&self) -> FeedHealth {
        self.l2_health
    }

    /// Record an L1 message received
    pub fn record_l1_message(&self, timestamp_ns: u64) {
        self.last_l1_ts_ns.store(timestamp_ns, Ordering::Relaxed);
        self.messages_received.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an L2 message received
    pub fn record_l2_message(&self, timestamp_ns: u64) {
        self.last_l2_ts_ns.store(timestamp_ns, Ordering::Relaxed);
        self.messages_received.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a dropped message (due to backpressure)
    pub fn record_dropped(&self) {
        self.messages_dropped.fetch_add(1, Ordering::Relaxed);
    }

    /// Mark as connected
    pub fn mark_connected(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        self.connected_at_ns.store(now, Ordering::Relaxed);
    }

    /// Mark as disconnected
    pub fn mark_disconnected(&mut self) {
        self.l1_health = FeedHealth::Disconnected;
        self.l2_health = FeedHealth::Disconnected;
    }

    /// Update L1 health based on staleness
    pub fn update_l1_health(&mut self, max_staleness: Duration) {
        let last_ts = self.last_l1_ts_ns.load(Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        if last_ts == 0 {
            self.l1_health = FeedHealth::Unknown;
        } else if now.saturating_sub(last_ts) > max_staleness.as_nanos() as u64 {
            self.l1_health = FeedHealth::Stale;
        } else {
            self.l1_health = FeedHealth::Healthy;
        }
    }

    /// Update L2 health based on staleness
    pub fn update_l2_health(&mut self, max_staleness: Duration) {
        let last_ts = self.last_l2_ts_ns.load(Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        if last_ts == 0 {
            self.l2_health = FeedHealth::Unknown;
        } else if now.saturating_sub(last_ts) > max_staleness.as_nanos() as u64 {
            self.l2_health = FeedHealth::Stale;
        } else {
            self.l2_health = FeedHealth::Healthy;
        }
    }

    /// Get combined health status
    pub fn combined_health(&self) -> &'static str {
        match (&self.l1_health, &self.l2_health) {
            (FeedHealth::Healthy, FeedHealth::Healthy) => "healthy",
            (FeedHealth::Healthy | FeedHealth::Stale, FeedHealth::Healthy | FeedHealth::Stale) => {
                "degraded"
            }
            _ => "critical",
        }
    }

    /// Check if L1 is stale
    pub fn is_l1_stale(&self, max_staleness: Duration) -> bool {
        let last_ts = self.last_l1_ts_ns.load(Ordering::Relaxed);
        if last_ts == 0 {
            return true;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        now.saturating_sub(last_ts) > max_staleness.as_nanos() as u64
    }

    /// Check if L2 is stale
    pub fn is_l2_stale(&self, max_staleness: Duration) -> bool {
        let last_ts = self.last_l2_ts_ns.load(Ordering::Relaxed);
        if last_ts == 0 {
            return true;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        now.saturating_sub(last_ts) > max_staleness.as_nanos() as u64
    }

    /// Get time since last L1 message
    pub fn time_since_l1(&self) -> Duration {
        let last_ts = self.last_l1_ts_ns.load(Ordering::Relaxed);
        if last_ts == 0 {
            return Duration::MAX;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        Duration::from_nanos(now.saturating_sub(last_ts))
    }

    /// Get time since last L2 message
    pub fn time_since_l2(&self) -> Duration {
        let last_ts = self.last_l2_ts_ns.load(Ordering::Relaxed);
        if last_ts == 0 {
            return Duration::MAX;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        Duration::from_nanos(now.saturating_sub(last_ts))
    }

    /// Get last L1 timestamp
    pub fn last_l1_timestamp(&self) -> u64 {
        self.last_l1_ts_ns.load(Ordering::Relaxed)
    }

    /// Get last L2 timestamp
    pub fn last_l2_timestamp(&self) -> u64 {
        self.last_l2_ts_ns.load(Ordering::Relaxed)
    }
}

/// Health monitor managing all exchange connections
#[derive(Debug)]
pub struct HealthMonitor {
    /// Per-exchange health states
    exchanges: std::collections::HashMap<String, ConnectionHealth>,
    /// L1 staleness threshold
    l1_staleness: Duration,
    /// L2 staleness threshold
    l2_staleness: Duration,
}

impl HealthMonitor {
    pub fn new(l1_staleness: Duration, l2_staleness: Duration) -> Self {
        Self {
            exchanges: std::collections::HashMap::new(),
            l1_staleness,
            l2_staleness,
        }
    }

    /// Register an exchange
    pub fn register_exchange(&mut self, exchange: &str) {
        self.exchanges
            .insert(exchange.to_string(), ConnectionHealth::new(exchange));
    }

    /// Get health for an exchange
    pub fn get_health(&self, exchange: &str) -> Option<&ConnectionHealth> {
        self.exchanges.get(exchange)
    }

    /// Get mutable health for an exchange
    pub fn get_health_mut(&mut self, exchange: &str) -> Option<&mut ConnectionHealth> {
        self.exchanges.get_mut(exchange)
    }

    /// Update all health states
    pub fn update_all(&mut self) {
        for health in self.exchanges.values_mut() {
            health.update_l1_health(self.l1_staleness);
            health.update_l2_health(self.l2_staleness);
        }
    }

    /// Check if any L2 is stale (blocks signal generation)
    pub fn any_l2_stale(&self) -> bool {
        self.exchanges
            .values()
            .any(|h| h.is_l2_stale(self.l2_staleness))
    }

    /// Check if any L1 is stale
    pub fn any_l1_stale(&self) -> bool {
        self.exchanges
            .values()
            .any(|h| h.is_l1_stale(self.l1_staleness))
    }

    /// Get all exchanges with their health status
    pub fn status(&self) -> Vec<(&str, &str, &str)> {
        self.exchanges
            .iter()
            .map(|(name, health)| {
                (
                    name.as_str(),
                    health.l1_health.as_str(),
                    health.l2_health.as_str(),
                )
            })
            .collect()
    }
}

impl Default for HealthMonitor {
    fn default() -> Self {
        Self::new(
            Duration::from_secs(5), // L1: 5 seconds
            Duration::from_secs(2), // L2: 2 seconds
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_health_creation() {
        let health = ConnectionHealth::new("binance");
        assert_eq!(health.exchange, "binance");
        assert_eq!(health.l1_health(), FeedHealth::Unknown);
        assert_eq!(health.l2_health(), FeedHealth::Unknown);
    }

    #[test]
    fn test_record_l2_message() {
        let health = ConnectionHealth::new("binance");
        let now = 1000000000u64; // 1 second after epoch

        health.record_l2_message(now);
        assert_eq!(health.last_l2_timestamp(), now);
        assert_eq!(health.messages_received.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_staleness_detection() {
        let health = ConnectionHealth::new("binance");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        // Record message just now
        health.record_l2_message(now);
        assert!(!health.is_l2_stale(Duration::from_secs(1)));

        // Record message 5 seconds ago
        health.record_l2_message(now - 5_000_000_000);
        assert!(health.is_l2_stale(Duration::from_secs(1)));
    }

    #[test]
    fn test_combined_health() {
        let mut health = ConnectionHealth::new("binance");

        health.l1_health = FeedHealth::Healthy;
        health.l2_health = FeedHealth::Healthy;
        assert_eq!(health.combined_health(), "healthy");

        health.l1_health = FeedHealth::Stale;
        health.l2_health = FeedHealth::Healthy;
        assert_eq!(health.combined_health(), "degraded");

        health.l1_health = FeedHealth::Disconnected;
        health.l2_health = FeedHealth::Healthy;
        assert_eq!(health.combined_health(), "critical");
    }

    #[test]
    fn test_health_monitor() {
        let mut monitor = HealthMonitor::default();
        monitor.register_exchange("binance");
        monitor.register_exchange("bybit");

        assert!(monitor.get_health("binance").is_some());
        assert!(monitor.get_health("bitget").is_none());
    }

    #[test]
    fn test_feed_health_is_usable() {
        assert!(FeedHealth::Healthy.is_usable());
        assert!(FeedHealth::Stale.is_usable()); // Stale is still usable
        assert!(!FeedHealth::Disconnected.is_usable());
        assert!(!FeedHealth::Unknown.is_usable());
    }

    #[test]
    fn test_l1_healthy_l2_stale() {
        let mut monitor = HealthMonitor::default();
        monitor.register_exchange("binance");

        let health = monitor.get_health_mut("binance").unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        // L1 healthy
        health.record_l1_message(now);
        // L2 stale (5 seconds ago)
        health.record_l2_message(now - 5_000_000_000);

        monitor.update_all();

        let h = monitor.get_health("binance").unwrap();
        assert_eq!(h.l1_health(), FeedHealth::Healthy);
        assert_eq!(h.l2_health(), FeedHealth::Stale);
        assert!(monitor.any_l2_stale());
    }

    #[test]
    fn test_l1_stale_l2_healthy() {
        let mut monitor = HealthMonitor::default();
        monitor.register_exchange("binance");

        let health = monitor.get_health_mut("binance").unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        // L1 stale (5 seconds ago)
        health.record_l1_message(now - 5_000_000_000);
        // L2 healthy
        health.record_l2_message(now);

        monitor.update_all();

        let h = monitor.get_health("binance").unwrap();
        assert_eq!(h.l1_health(), FeedHealth::Stale);
        assert_eq!(h.l2_health(), FeedHealth::Healthy);
        assert!(monitor.any_l1_stale());
    }

    #[test]
    fn test_both_healthy() {
        let mut monitor = HealthMonitor::default();
        monitor.register_exchange("binance");

        let health = monitor.get_health_mut("binance").unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        health.record_l1_message(now);
        health.record_l2_message(now);

        monitor.update_all();

        let h = monitor.get_health("binance").unwrap();
        assert_eq!(h.l1_health(), FeedHealth::Healthy);
        assert_eq!(h.l2_health(), FeedHealth::Healthy);
        assert!(!monitor.any_l1_stale());
        assert!(!monitor.any_l2_stale());
    }
}
