//! # Lead-Lag Detection Module
//!
//! Implements Binance lead detection and Arbitrum lag detection for arbitrage opportunities.

use crate::pool_discovery::TokenPair;
use crate::types::PriceEvent;

use parking_lot::RwLock;
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone)]
pub struct TokenMapping {
    pub binance_symbol: String,
    pub arbitrum_token: String,
    pub token_symbol: String,
}

#[derive(Debug, Clone)]
pub struct MovementDetection {
    pub symbol: String,
    pub direction: MovementDirection,
    pub velocity_bps_per_sec: f64,
    pub acceleration: Option<f64>,
    pub volume_spike: bool,
    pub confidence: f64,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovementDirection {
    Up,
    Down,
    Sideways,
}

impl MovementDirection {
    pub fn is_significant(&self, _threshold_bps: f64) -> bool {
        match self {
            MovementDirection::Up | MovementDirection::Down => true,
            MovementDirection::Sideways => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LeadLagSignal {
    pub id: String,
    pub binance_movement: MovementDetection,
    pub affected_tokens: Vec<TokenMapping>,
    pub candidate_pools: Vec<String>,
    pub estimated_opportunity_count: usize,
    pub timestamp: u64,
    pub t0_receive: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MovementConfig {
    pub price_change_threshold_bps: f64,
    pub velocity_threshold_bps_per_sec: f64,
    pub acceleration_threshold_bps_per_sec2: f64,
    pub volume_spike_multiplier: f64,
    pub min_confidence: f64,
    pub lookback_window_ms: u64,
    pub cooldown_ms: u64,
}

impl Default for MovementConfig {
    fn default() -> Self {
        Self {
            price_change_threshold_bps: 10.0,
            velocity_threshold_bps_per_sec: 5.0,
            acceleration_threshold_bps_per_sec2: 2.0,
            volume_spike_multiplier: 2.0,
            min_confidence: 0.5,
            lookback_window_ms: 1000,
            cooldown_ms: 100,
        }
    }
}

#[derive(Debug, Clone)]
struct PricePoint {
    price: f64,
    timestamp: u64,
    volume: f64,
}

impl PricePoint {
    fn new(price: f64, timestamp: u64, volume: f64) -> Self {
        Self {
            price,
            timestamp,
            volume,
        }
    }
}

pub struct LeadLagDetector {
    config: MovementConfig,
    price_history: RwLock<HashMap<String, VecDeque<PricePoint>>>,
    token_mappings: RwLock<Vec<TokenMapping>>,
    last_signal_time: RwLock<HashMap<String, u64>>,
}

impl LeadLagDetector {
    pub fn new(config: MovementConfig) -> Self {
        Self {
            config,
            price_history: RwLock::new(HashMap::new()),
            token_mappings: RwLock::new(Self::default_mappings()),
            last_signal_time: RwLock::new(HashMap::new()),
        }
    }

    fn default_mappings() -> Vec<TokenMapping> {
        vec![
            TokenMapping {
                binance_symbol: "ETHUSDT".to_string(),
                arbitrum_token: "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1".to_lowercase(),
                token_symbol: "WETH".to_string(),
            },
            TokenMapping {
                binance_symbol: "WBTCUSDT".to_string(),
                arbitrum_token: "0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f".to_lowercase(),
                token_symbol: "WBTC".to_string(),
            },
            TokenMapping {
                binance_symbol: "LINKUSDT".to_string(),
                arbitrum_token: "0xf97f4df05117a5ab3f3b5b0d4d7b1c0d4e9f7a6b".to_lowercase(),
                token_symbol: "LINK".to_string(),
            },
            TokenMapping {
                binance_symbol: "UNIUSDT".to_string(),
                arbitrum_token: "0xFa7F8980b0f1E64A2062791cc3b4e149cF6F6C0C".to_lowercase(),
                token_symbol: "UNI".to_string(),
            },
        ]
    }

    pub fn add_mapping(&self, mapping: TokenMapping) {
        let mut mappings = self.token_mappings.write();
        mappings.retain(|m| m.binance_symbol != mapping.binance_symbol);
        mappings.push(mapping);
    }

    pub fn detect_movement(&self, event: &PriceEvent) -> Option<MovementDetection> {
        let symbol = &event.symbol;
        let price = event.price.parse::<f64>().ok()?;
        let timestamp = event.trade_time;
        let volume = event.quantity.parse::<f64>().ok()?;

        {
            let mut history = self.price_history.write();
            let points = history.entry(symbol.clone()).or_default();
            points.push_back(PricePoint::new(price, timestamp, volume));

            let cutoff = timestamp.saturating_sub(self.config.lookback_window_ms);
            while points
                .front()
                .map(|p| p.timestamp < cutoff)
                .unwrap_or(false)
            {
                points.pop_front();
            }
        }

        {
            let last_time = self.last_signal_time.read();
            if let Some(&last) = last_time.get(symbol) {
                if timestamp.saturating_sub(last) < self.config.cooldown_ms {
                    return None;
                }
            }
        }

        let history = self.price_history.read();
        let points = history.get(symbol)?;

        if points.len() < 2 {
            return None;
        }

        let first = &points[0];
        let last_pt = points.back()?;

        let price_change_ratio = (last_pt.price - first.price) / first.price;
        let price_change_bps = price_change_ratio * 10000.0;

        let time_delta_sec = (last_pt.timestamp - first.timestamp) as f64 / 1000.0;
        let velocity_bps_per_sec = if time_delta_sec > 0.0 {
            price_change_bps / time_delta_sec
        } else {
            0.0
        };

        let acceleration = if points.len() >= 4 {
            let mid_idx = points.len() / 2;
            let p1 = points.front()?;
            let p2 = points.get(mid_idx)?;
            let p3 = points.get(mid_idx)?;
            let p4 = points.back()?;

            let delta1 = p2.timestamp.saturating_sub(p1.timestamp);
            let delta2 = p4.timestamp.saturating_sub(p3.timestamp);

            if delta1 > 0 && delta2 > 0 {
                let change1 = (p2.price - p1.price) / p1.price * 10000.0;
                let change2 = (p4.price - p3.price) / p3.price * 10000.0;
                Some((change2 / (delta2 as f64 / 1000.0)) - (change1 / (delta1 as f64 / 1000.0)))
            } else {
                None
            }
        } else {
            None
        };

        let avg_volume: f64 = points.iter().map(|p| p.volume).sum::<f64>() / points.len() as f64;
        let volume_spike = last_pt.volume > avg_volume * self.config.volume_spike_multiplier;

        let direction = if price_change_bps > self.config.price_change_threshold_bps {
            MovementDirection::Up
        } else if price_change_bps < -self.config.price_change_threshold_bps {
            MovementDirection::Down
        } else {
            MovementDirection::Sideways
        };

        let mut confidence = 0.5;
        confidence += (price_change_bps.abs() / 100.0).min(0.3);

        if velocity_bps_per_sec.abs() > self.config.velocity_threshold_bps_per_sec {
            confidence += 0.1;
        }

        if volume_spike {
            confidence += 0.1;
        }

        if let Some(acc) = acceleration {
            if acc.abs() > self.config.acceleration_threshold_bps_per_sec2 {
                confidence += 0.1;
            }
        }

        confidence = confidence.min(1.0);

        if confidence < self.config.min_confidence {
            return None;
        }

        if !direction.is_significant(self.config.price_change_threshold_bps) {
            return None;
        }

        {
            let mut last_time = self.last_signal_time.write();
            last_time.insert(symbol.clone(), timestamp);
        }

        Some(MovementDetection {
            symbol: symbol.clone(),
            direction,
            velocity_bps_per_sec,
            acceleration,
            volume_spike,
            confidence,
            timestamp,
        })
    }

    pub fn get_affected_tokens(&self, movement: &MovementDetection) -> Vec<TokenMapping> {
        let mappings = self.token_mappings.read();
        mappings
            .iter()
            .filter(|m| m.binance_symbol.to_uppercase() == movement.symbol.to_uppercase())
            .cloned()
            .collect()
    }

    pub fn get_token_pair(&self, mapping: &TokenMapping) -> TokenPair {
        TokenPair::new_hex(
            &mapping.arbitrum_token,
            "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
        )
    }

    pub fn generate_signal(
        &self,
        movement: &MovementDetection,
        candidate_pools: Vec<String>,
    ) -> LeadLagSignal {
        let opportunity_count = candidate_pools.len();
        LeadLagSignal {
            id: format!("{:x}-{:x}", movement.timestamp, rand_simple()),
            binance_movement: movement.clone(),
            affected_tokens: self.get_affected_tokens(movement),
            candidate_pools,
            estimated_opportunity_count: opportunity_count,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            t0_receive: movement.timestamp,
        }
    }
}

fn hex_to_addr(hex: &str) -> [u8; 20] {
    let bytes = hex::decode(hex.trim_start_matches("0x")).unwrap_or_default();
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&bytes[..20]);
    addr
}

fn rand_simple() -> u64 {
    use std::time::Instant;
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_movement_detection() {
        let config = MovementConfig::default();
        let detector = LeadLagDetector::new(config);

        let event = PriceEvent::new(
            "ETHUSDT".to_string(),
            "3500.50".to_string(),
            "10.5".to_string(),
            1699999999000,
            false,
        );

        let detection = detector.detect_movement(&event);
        assert!(detection.is_none());
    }

    #[test]
    fn test_token_mappings() {
        let config = MovementConfig::default();
        let detector = LeadLagDetector::new(config);

        let mappings = detector.token_mappings.read();
        assert!(!mappings.is_empty());

        let eth_mapping = mappings.iter().find(|m| m.binance_symbol == "ETHUSDT");
        assert!(eth_mapping.is_some());
    }

    #[test]
    fn test_movement_direction_significance() {
        assert!(MovementDirection::Up.is_significant(10.0));
        assert!(MovementDirection::Down.is_significant(10.0));
        assert!(!MovementDirection::Sideways.is_significant(10.0));
    }
}
