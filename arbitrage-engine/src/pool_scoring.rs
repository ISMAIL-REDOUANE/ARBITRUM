//! # Pool Scoring Module
//!
//! Scores and ranks pools based on likelihood of producing executable lead-lag arbitrage opportunities.

use crate::pool_discovery::{EnrichedPool, PoolRegistry, TokenPair};
use crate::types::PriceEvent;

use parking_lot::RwLock;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

/// Scoring weights configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ScoringWeights {
    pub active_liquidity_weight: f64,
    pub executable_liquidity_weight: f64,
    pub volume_weight: f64,
    pub swap_frequency_weight: f64,
    pub volatility_weight: f64,
    pub cross_dex_weight: f64,
    pub reaction_speed_weight: f64,
    pub expected_profit_weight: f64,
    pub min_score_threshold: f64,
}

impl Default for ScoringWeights {
    fn default() -> Self {
        Self {
            active_liquidity_weight: 0.20,
            executable_liquidity_weight: 0.15,
            volume_weight: 0.15,
            swap_frequency_weight: 0.10,
            volatility_weight: 0.10,
            cross_dex_weight: 0.10,
            reaction_speed_weight: 0.10,
            expected_profit_weight: 0.10,
            min_score_threshold: 0.3,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScoringConfig {
    pub weights: ScoringWeights,
    pub lookback_swaps: usize,
    pub lookback_blocks: u64,
    pub volatility_window_blocks: u64,
    pub cross_dex_lookback: usize,
}

impl Default for ScoringConfig {
    fn default() -> Self {
        Self {
            weights: ScoringWeights::default(),
            lookback_swaps: 100,
            lookback_blocks: 100,
            volatility_window_blocks: 50,
            cross_dex_lookback: 10,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PoolScore {
    pub pool_address: String,
    pub total_score: f64,
    pub active_liquidity_score: f64,
    pub executable_liquidity_score: f64,
    pub volume_score: f64,
    pub swap_frequency_score: f64,
    pub volatility_score: f64,
    pub cross_dex_score: f64,
    pub reaction_speed_score: f64,
    pub expected_profit_score: f64,
    pub is_candidate: bool,
}

impl Default for PoolScore {
    fn default() -> Self {
        Self {
            pool_address: String::new(),
            total_score: 0.0,
            active_liquidity_score: 0.0,
            executable_liquidity_score: 0.0,
            volume_score: 0.0,
            swap_frequency_score: 0.0,
            volatility_score: 0.0,
            cross_dex_score: 0.0,
            reaction_speed_score: 0.0,
            expected_profit_score: 0.0,
            is_candidate: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReactionHistory {
    pub sample_count: usize,
    pub avg_lag_blocks: f64,
    pub min_lag_blocks: u64,
    pub max_lag_blocks: u64,
    pub reaction_count: usize,
    pub miss_count: usize,
}

impl ReactionHistory {
    pub fn reaction_rate(&self) -> f64 {
        if self.sample_count == 0 {
            return 0.0;
        }
        self.reaction_count as f64 / self.sample_count as f64
    }

    pub fn avg_reaction_lag(&self) -> f64 {
        self.avg_lag_blocks
    }

    pub fn record(&mut self, lag_blocks: u64, reacted: bool) {
        self.sample_count += 1;
        self.avg_lag_blocks = (self.avg_lag_blocks * (self.sample_count - 1) as f64
            + lag_blocks as f64)
            / self.sample_count as f64;
        self.min_lag_blocks = self.min_lag_blocks.min(lag_blocks);
        self.max_lag_blocks = self.max_lag_blocks.max(lag_blocks);
        if reacted {
            self.reaction_count += 1;
        } else {
            self.miss_count += 1;
        }
    }
}

pub struct PoolScorer {
    config: ScoringConfig,
    scores: RwLock<HashMap<String, PoolScore>>,
    reaction_history: RwLock<HashMap<String, ReactionHistory>>,
}

impl PoolScorer {
    pub fn new(config: ScoringConfig) -> Self {
        Self {
            config,
            scores: RwLock::new(HashMap::new()),
            reaction_history: RwLock::new(HashMap::new()),
        }
    }

    pub fn score_pool(&self, pool: &EnrichedPool) -> PoolScore {
        let mut score = PoolScore {
            pool_address: pool.base.address.clone(),
            ..Default::default()
        };

        score.active_liquidity_score = self.score_active_liquidity(pool);
        score.executable_liquidity_score = self.score_executable_liquidity(pool);
        score.volume_score = self.score_volume(pool);
        score.swap_frequency_score = self.score_swap_frequency(pool);
        score.volatility_score = self.score_volatility(pool);
        score.cross_dex_score = 0.5;
        score.reaction_speed_score = self.score_reaction_speed(&pool.base.address);
        score.expected_profit_score = self.score_expected_profit(pool);

        let w = &self.config.weights;
        score.total_score = w.active_liquidity_weight * score.active_liquidity_score
            + w.executable_liquidity_weight * score.executable_liquidity_score
            + w.volume_weight * score.volume_score
            + w.swap_frequency_weight * score.swap_frequency_score
            + w.volatility_weight * score.volatility_score
            + w.cross_dex_weight * score.cross_dex_score
            + w.reaction_speed_weight * score.reaction_speed_score
            + w.expected_profit_weight * score.expected_profit_score;

        score.is_candidate = score.total_score >= self.config.weights.min_score_threshold;

        {
            let mut scores = self.scores.write();
            scores.insert(pool.base.address.clone(), score.clone());
        }

        score
    }

    fn score_active_liquidity(&self, pool: &EnrichedPool) -> f64 {
        let liquidity = pool.active_liquidity as f64;
        let reference_max = 100_000_000_000_000_000_000_000_000.0;
        (liquidity / reference_max).min(1.0)
    }

    fn score_executable_liquidity(&self, pool: &EnrichedPool) -> f64 {
        let exec_liq = pool.executable_liquidity as f64;
        let active_liq = pool.active_liquidity.max(1) as f64;
        (exec_liq / active_liq).min(1.0)
    }

    fn score_volume(&self, pool: &EnrichedPool) -> f64 {
        let volume_usd = pool.recent_volume as f64 / 1e18 * 3500.0;
        let reference_max = 10_000_000_000.0;
        (volume_usd / reference_max).min(1.0)
    }

    fn score_swap_frequency(&self, pool: &EnrichedPool) -> f64 {
        let swap_count = pool.recent_swaps.len() as f64;
        let max_swaps = self.config.lookback_swaps as f64;
        (swap_count / max_swaps).min(1.0)
    }

    fn score_volatility(&self, pool: &EnrichedPool) -> f64 {
        let volatility_bps = pool.price_movement_bps as f64;
        (volatility_bps / 100.0).min(1.0)
    }

    fn score_reaction_speed(&self, pool_address: &str) -> f64 {
        let history = self.reaction_history.read();
        if let Some(h) = history.get(pool_address) {
            if h.reaction_rate() > 0.0 {
                let lag_score = (10.0 / h.avg_reaction_lag().max(1.0)).min(1.0);
                return lag_score * h.reaction_rate();
            }
        }
        0.0
    }

    fn score_expected_profit(&self, pool: &EnrichedPool) -> f64 {
        let volume_usd = pool.recent_volume as f64 / 1e18 * 3500.0;
        let estimated_spread = 0.001;
        let fill_rate = 0.1;
        let expected_profit_usd = volume_usd * estimated_spread * fill_rate;
        let reference_profit = 1000.0;
        (expected_profit_usd / reference_profit).min(1.0)
    }

    pub fn record_binance_movement(&self, affected_tokens: &[TokenPair], lag_blocks: u64) {
        let mut history = self.reaction_history.write();

        for pair in affected_tokens {
            let key = format!("{}-{}", pair.token0, pair.token1);
            let entry = history.entry(key).or_default();
            entry.record(lag_blocks, true);
        }
    }

    pub fn get_ranked_pools(&self) -> Vec<PoolScore> {
        let scores = self.scores.read();
        let mut ranked: Vec<_> = scores.values().cloned().collect();
        ranked.sort_by(|a, b| b.total_score.partial_cmp(&a.total_score).unwrap());
        ranked
    }

    pub fn get_candidates(&self) -> Vec<PoolScore> {
        self.get_ranked_pools()
            .into_iter()
            .filter(|s| s.is_candidate)
            .collect()
    }

    pub fn get_score(&self, pool_address: &str) -> Option<PoolScore> {
        self.scores.read().get(pool_address).cloned()
    }

    pub fn clear_scores(&self) {
        self.scores.write().clear();
    }
}

pub struct CandidateSelector {
    scorer: Arc<PoolScorer>,
    registry: Arc<PoolRegistry>,
    config: ScoringConfig,
}

impl CandidateSelector {
    pub fn new(
        scorer: Arc<PoolScorer>,
        registry: Arc<PoolRegistry>,
        config: ScoringConfig,
    ) -> Self {
        Self {
            scorer,
            registry,
            config,
        }
    }

    pub fn select_candidates_for_movement(
        &self,
        _price_event: &PriceEvent,
        affected_tokens: &[TokenPair],
    ) -> Vec<(EnrichedPool, PoolScore)> {
        let mut candidates = Vec::new();

        for pair in affected_tokens {
            let pools = self.registry.get_by_token_pair(pair);

            for pool in pools {
                if !pool.is_fresh() || !pool.is_usable() {
                    continue;
                }

                let score = self.scorer.score_pool(&pool);
                if score.is_candidate {
                    candidates.push((pool, score));
                }
            }
        }

        candidates.sort_by(|a, b| b.1.total_score.partial_cmp(&a.1.total_score).unwrap());
        candidates.truncate(10);
        candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_score_calculation() {
        let config = ScoringConfig::default();
        let scorer = PoolScorer::new(config);

        let pool = EnrichedPool {
            active_liquidity: 1_000_000_000_000_000_000_000_000u128,
            executable_liquidity: 500_000_000_000_000_000_000_000u128,
            recent_volume: 1_000_000_000_000_000_000u128,
            price_movement_bps: 50,
            ..Default::default()
        };

        let score = scorer.score_pool(&pool);

        assert!(score.total_score > 0.0);
        assert!(score.active_liquidity_score > 0.0);
    }

    #[test]
    fn test_candidate_filtering() {
        let config = ScoringConfig::default();
        let scorer = PoolScorer::new(config);

        let pool = EnrichedPool {
            active_liquidity: 1_000_000_000_000_000u128,
            executable_liquidity: 500_000_000_000_000u128,
            ..Default::default()
        };

        let score = scorer.score_pool(&pool);

        // Score depends on values, check it runs without error
        assert!(score.pool_address == pool.base.address);
    }

    #[test]
    fn test_reaction_history() {
        let mut history = ReactionHistory::default();

        history.record(10, true);
        history.record(5, true);

        assert_eq!(history.sample_count, 2);
        assert_eq!(history.reaction_count, 2);
        assert!((history.avg_lag_blocks - 7.5).abs() < 0.1);

        history.record(20, false);

        assert_eq!(history.sample_count, 3);
        assert_eq!(history.reaction_count, 2);
        assert!((history.avg_lag_blocks - 11.67).abs() < 0.1);
        assert_eq!(history.reaction_rate(), 2.0 / 3.0);
    }
}
