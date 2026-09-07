//! # Route Generation Module
//!
//! Implements real two-leg arbitrage route discovery and validation.

use crate::pool_discovery::{EnrichedPool, PoolRegistry, TokenPair};
use crate::types::DexType;

use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct RouteLeg {
    pub dex_type: DexType,
    pub dex_name: String,
    pub pool_address: String,
    pub token_in: String,
    pub token_out: String,
    pub fee_tier: u32,
    pub expected_output: u128,
    pub min_output: u128,
    pub price_impact_bps: u32,
}

#[derive(Debug, Clone)]
pub struct ArbitrageRoute {
    pub id: String,
    pub legs: Vec<RouteLeg>,
    pub flash_loan_token: String,
    pub flash_loan_amount: u128,
    pub expected_profit: u128,
    pub min_profit: u128,
    pub total_fees: u128,
    pub price_impact_bps: u32,
    pub is_valid: bool,
    pub validation_errors: Vec<String>,
}

impl ArbitrageRoute {
    pub fn new(flash_loan_token: String, flash_loan_amount: u128) -> Self {
        Self {
            id: format!("{:x}", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64),
            legs: Vec::new(),
            flash_loan_token,
            flash_loan_amount,
            expected_profit: 0,
            min_profit: 0,
            total_fees: 0,
            price_impact_bps: 0,
            is_valid: false,
            validation_errors: Vec::new(),
        }
    }
    
    pub fn add_leg(&mut self, leg: RouteLeg) {
        self.total_fees = self.total_fees.saturating_add(calculate_swap_fee(leg.expected_output, leg.fee_tier));
        self.price_impact_bps = self.price_impact_bps.saturating_add(leg.price_impact_bps);
        self.legs.push(leg);
    }
    
    pub fn validate(&mut self) {
        self.validation_errors.clear();
        
        if self.legs.len() != 2 {
            self.validation_errors.push(format!(
                "Expected 2 legs for two-leg arbitrage, got {}",
                self.legs.len()
            ));
            return;
        }
        
        let leg0 = &self.legs[0];
        let leg1 = &self.legs[1];
        
        if leg0.token_out != leg1.token_in {
            self.validation_errors.push(format!(
                "Token continuity error: leg0 outputs {} but leg1 expects {}",
                leg0.token_out, leg1.token_in
            ));
        }
        
        if leg0.token_in != leg1.token_out {
            self.validation_errors.push(format!(
                "Route does not return to start token: starts {}, ends {}",
                leg0.token_in, leg1.token_out
            ));
        }
        
        if leg0.token_in != self.flash_loan_token {
            self.validation_errors.push(format!(
                "First leg must use flash loan token {}, got {}",
                self.flash_loan_token, leg0.token_in
            ));
        }
        
        self.validate_dex_compatibility();
        self.validate_minimum_output();
        
        self.is_valid = self.validation_errors.is_empty();
    }
    
    fn validate_dex_compatibility(&mut self) {
        for leg in &self.legs {
            match leg.dex_type {
                DexType::UniswapV3 => {
                    if leg.fee_tier == 0 || leg.fee_tier > 10000 {
                        self.validation_errors.push(format!(
                            "Invalid Uniswap V3 fee tier: {}",
                            leg.fee_tier
                        ));
                    }
                },
                DexType::UniswapV2 | DexType::SushiSwap => {
                    if leg.fee_tier != 30 {
                        self.validation_errors.push(format!(
                            "UniswapV2/SushiSwap only supports 0.3% fee, got {}",
                            leg.fee_tier
                        ));
                    }
                },
                DexType::Aerodrome => {},
            }
        }
    }
    
    fn validate_minimum_output(&mut self) {
        if let Some(last_leg) = self.legs.last() {
            let repayment = self.flash_loan_amount.saturating_add(self.total_fees);
            if last_leg.expected_output < repayment {
                self.validation_errors.push(format!(
                    "Expected output {} < repayment {}",
                    last_leg.expected_output, repayment
                ));
            }
        }
    }
    
    pub fn calculate_profit(&mut self) {
        if !self.is_valid || self.legs.is_empty() {
            return;
        }
        
        if let Some(last_leg) = self.legs.last() {
            let final_output = last_leg.expected_output;
            let repayment = self.flash_loan_amount.saturating_add(self.total_fees);
            
            if final_output > repayment {
                self.expected_profit = final_output.saturating_sub(repayment);
                self.min_profit = last_leg.min_output.saturating_sub(repayment);
            } else {
                self.expected_profit = 0;
                self.min_profit = 0;
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouteFinderConfig {
    pub max_hops: usize,
    pub max_price_impact_bps: u32,
    pub min_profit_wei: u128,
    pub max_gas_cost_wei: u128,
    pub flash_loan_fee_bps: u32,
}

impl Default for RouteFinderConfig {
    fn default() -> Self {
        Self {
            max_hops: 2,
            max_price_impact_bps: 100,
            min_profit_wei: 10_000_000_000_000_000,
            max_gas_cost_wei: 5_000_000_000_000_000,
            flash_loan_fee_bps: 0,
        }
    }
}

pub struct RouteFinder {
    config: RouteFinderConfig,
    registry: Arc<PoolRegistry>,
}

impl RouteFinder {
    pub fn new(config: RouteFinderConfig, registry: Arc<PoolRegistry>) -> Self {
        Self { config, registry }
    }
    
    pub fn find_two_leg_routes(
        &self,
        token_pair: &TokenPair,
        flash_loan_token: String,
        initial_amount: u128,
    ) -> Vec<ArbitrageRoute> {
        let mut routes = Vec::new();
        
        let pools = self.registry.get_by_token_pair(token_pair);
        
        if pools.len() < 2 {
            return routes;
        }
        
        for (i, pool_a) in pools.iter().enumerate() {
            for pool_b in pools.iter().skip(i + 1) {
                let intermediate = self.find_intermediate_token(
                    &flash_loan_token,
                    &pool_a.base,
                    &pool_b.base,
                );
                
                if let Some(intermediate) = intermediate {
                    if let Some(mut route) = self.build_route(
                        flash_loan_token.clone(),
                        intermediate,
                        pool_a,
                        pool_b,
                        initial_amount,
                    ) {
                        route.validate();
                        if route.is_valid {
                            route.calculate_profit();
                            if route.expected_profit >= self.config.min_profit_wei {
                                routes.push(route);
                            }
                        }
                    }
                }
            }
        }
        
        routes.sort_by(|a, b| b.expected_profit.cmp(&a.expected_profit));
        routes
    }
    
    fn find_intermediate_token(
        &self,
        start_token: &str,
        pool_a: &crate::types::PoolState,
        pool_b: &crate::types::PoolState,
    ) -> Option<String> {
        let pool_a_token0 = pool_a.token0.to_lowercase();
        let pool_a_token1 = pool_a.token1.to_lowercase();
        let pool_b_token0 = pool_b.token0.to_lowercase();
        let pool_b_token1 = pool_b.token1.to_lowercase();
        
        let candidates = [pool_a_token1.clone(), pool_a_token0.clone()];
        
        for candidate in candidates {
            if candidate == pool_b_token0 || candidate == pool_b_token1 {
                let pool_b_other = if candidate == pool_b_token0 {
                    pool_b_token1.clone()
                } else {
                    pool_b_token0.clone()
                };
                
                if pool_b_other == start_token.to_lowercase() {
                    return Some(candidate);
                }
            }
        }
        
        None
    }
    
    fn build_route(
        &self,
        start_token: String,
        intermediate: String,
        pool_a: &EnrichedPool,
        pool_b: &EnrichedPool,
        amount: u128,
    ) -> Option<ArbitrageRoute> {
        let mut route = ArbitrageRoute::new(start_token.clone(), amount);
        
        let leg1_output = self.estimate_swap_output(
            &start_token,
            &intermediate,
            pool_a,
            amount,
        )?;
        
        let leg1 = RouteLeg {
            dex_type: pool_a.dex_type,
            dex_name: format!("{:?}", pool_a.dex_type),
            pool_address: pool_a.base.address.clone(),
            token_in: start_token.clone(),
            token_out: intermediate.clone(),
            fee_tier: pool_a.base.fee_tier,
            expected_output: leg1_output,
            min_output: leg1_output.saturating_mul(99) / 100,
            price_impact_bps: self.estimate_price_impact_bps(pool_a, amount, leg1_output),
        };
        
        route.add_leg(leg1);
        
        let leg2_output = self.estimate_swap_output(
            &intermediate,
            &start_token,
            pool_b,
            leg1_output,
        )?;
        
        let leg2 = RouteLeg {
            dex_type: pool_b.dex_type,
            dex_name: format!("{:?}", pool_b.dex_type),
            pool_address: pool_b.base.address.clone(),
            token_in: intermediate.clone(),
            token_out: start_token.clone(),
            fee_tier: pool_b.base.fee_tier,
            expected_output: leg2_output,
            min_output: leg2_output.saturating_mul(99) / 100,
            price_impact_bps: self.estimate_price_impact_bps(pool_b, leg1_output, leg2_output),
        };
        
        route.add_leg(leg2);
        
        Some(route)
    }
    
    fn estimate_swap_output(
        &self,
        token_in: &str,
        _token_out: &str,
        pool: &EnrichedPool,
        amount_in: u128,
    ) -> Option<u128> {
        let (reserve_in, reserve_out) = if pool.base.token0.to_lowercase() == token_in.to_lowercase() {
            (pool.base.reserve0, pool.base.reserve1)
        } else {
            (pool.base.reserve1, pool.base.reserve0)
        };
        
        if reserve_in == 0 || reserve_out == 0 {
            return None;
        }
        
        let amount_in_with_fee = amount_in.saturating_mul(997);
        let numerator = amount_in_with_fee.saturating_mul(reserve_out);
        let denominator = reserve_in.saturating_mul(1000).saturating_add(amount_in_with_fee);
        
        if denominator == 0 {
            return None;
        }
        
        Some(numerator / denominator)
    }
    
    fn estimate_price_impact_bps(
        &self,
        pool: &EnrichedPool,
        amount_in: u128,
        amount_out: u128,
    ) -> u32 {
        let spot_price = if pool.base.reserve0 > 0 {
            pool.base.reserve1 as f64 / pool.base.reserve0 as f64
        } else {
            0.0
        };
        
        let executed_price = if amount_in > 0 {
            amount_out as f64 / amount_in as f64
        } else {
            0.0
        };
        
        if spot_price > 0.0 {
            ((1.0 - executed_price / spot_price) * 10000.0) as u32
        } else {
            0
        }
    }
    
    pub fn find_all_routes(
        &self,
        token_pairs: &[TokenPair],
        flash_loan_token: String,
        amount: u128,
    ) -> Vec<ArbitrageRoute> {
        let mut all_routes = Vec::new();
        
        for pair in token_pairs {
            let routes = self.find_two_leg_routes(pair, flash_loan_token.clone(), amount);
            all_routes.extend(routes);
        }
        
        all_routes.sort_by(|a, b| b.expected_profit.cmp(&a.expected_profit));
        all_routes.truncate(100);
        
        all_routes
    }
}

fn calculate_swap_fee(amount: u128, fee_tier_bps: u32) -> u128 {
    amount.saturating_mul(fee_tier_bps as u128) / 10000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route_validation() {
        let token_a = "0x0000000000000000000000000000000000000001".to_lowercase();
        let token_b = "0x0000000000000000000000000000000000000002".to_lowercase();
        
        let mut route = ArbitrageRoute::new(token_a.clone(), 1_000_000);
        
        route.add_leg(RouteLeg {
            dex_type: DexType::UniswapV2,
            dex_name: "UniswapV2".to_string(),
            pool_address: "0x0000000000000000000000000000000000000001".to_string(),
            token_in: token_a.clone(),
            token_out: token_b.clone(),
            fee_tier: 30,
            expected_output: 1_100_000,
            min_output: 1_089_000,
            price_impact_bps: 10,
        });
        
        route.add_leg(RouteLeg {
            dex_type: DexType::UniswapV3,
            dex_name: "UniswapV3".to_string(),
            pool_address: "0x0000000000000000000000000000000000000002".to_string(),
            token_in: token_b.clone(),
            token_out: token_a.clone(),
            fee_tier: 500,
            expected_output: 1_500_000,
            min_output: 1_485_000,
            price_impact_bps: 5,
        });
        
        route.validate();
        
        // Route should be valid with proper profitable data
        assert!(route.is_valid, "Validation errors: {:?}", route.validation_errors);
    }

    #[test]
    fn test_token_continuity_error() {
        let token_a = "0x0000000000000000000000000000000000000001".to_lowercase();
        let token_b = "0x0000000000000000000000000000000000000002".to_lowercase();
        let token_c = "0x0000000000000000000000000000000000000003".to_lowercase();
        
        let mut route = ArbitrageRoute::new(token_a.clone(), 1_000_000);
        
        route.add_leg(RouteLeg {
            dex_type: DexType::UniswapV2,
            dex_name: "UniswapV2".to_string(),
            pool_address: "0x0000000000000000000000000000000000000001".to_string(),
            token_in: token_a.clone(),
            token_out: token_b.clone(),
            fee_tier: 30,
            expected_output: 1_100_000,
            min_output: 1_089_000,
            price_impact_bps: 10,
        });
        
        route.add_leg(RouteLeg {
            dex_type: DexType::UniswapV3,
            dex_name: "UniswapV3".to_string(),
            pool_address: "0x0000000000000000000000000000000000000002".to_string(),
            token_in: token_c.clone(),
            token_out: token_a.clone(),
            fee_tier: 3000,
            expected_output: 900_000,
            min_output: 891_000,
            price_impact_bps: 5,
        });
        
        route.validate();
        
        assert!(!route.is_valid);
        assert!(!route.validation_errors.is_empty());
    }

    #[test]
    fn test_profit_calculation() {
        let token_a = "0x0000000000000000000000000000000000000001".to_lowercase();
        let token_b = "0x0000000000000000000000000000000000000002".to_lowercase();
        
        let mut route = ArbitrageRoute::new(token_a.clone(), 1_000_000);
        
        route.add_leg(RouteLeg {
            dex_type: DexType::UniswapV2,
            dex_name: "UniswapV2".to_string(),
            pool_address: "0x0000000000000000000000000000000000000001".to_string(),
            token_in: token_a.clone(),
            token_out: token_b.clone(),
            fee_tier: 30,
            expected_output: 1_100_000,
            min_output: 1_089_000,
            price_impact_bps: 10,
        });
        
        route.add_leg(RouteLeg {
            dex_type: DexType::UniswapV2,
            dex_name: "UniswapV2".to_string(),
            pool_address: "0x0000000000000000000000000000000000000002".to_string(),
            token_in: token_b.clone(),
            token_out: token_a.clone(),
            fee_tier: 30,
            expected_output: 1_210_000,
            min_output: 1_197_000,
            price_impact_bps: 10,
        });
        
        route.is_valid = true;
        route.calculate_profit();
        
        assert!(route.expected_profit > 0);
    }
}
