//! # Two-Leg Arbitrage Route Module
//!
//! Explicit two-leg route structure for arbitrage execution.
//! Route: TokenA -> DEX1 -> TokenB -> DEX2 -> TokenA

use serde::{Deserialize, Serialize};

/// DEX type for swap execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DexType {
    UniswapV2,
    UniswapV3,
    SushiSwap,
    Aerodrome,
}

impl DexType {
    pub fn router_address(&self, chain_id: u64) -> Option<[u8; 20]> {
        match self {
            DexType::UniswapV3 => Some(
                hex::decode("68b3465833fb72A70ecDF485E0e4C7bD8665Fc45")
                    .unwrap()
                    .try_into()
                    .unwrap(),
            ),
            DexType::UniswapV2 => {
                // Uniswap V2 Router address varies by chain
                match chain_id {
                    1 => Some(
                        hex::decode("7a250d5630B4cF539739dF2C5dAcb4c659F2488D")
                            .unwrap()
                            .try_into()
                            .unwrap(),
                    ), // Mainnet
                    42161 => Some(
                        hex::decode("0x34342370221487471869603929487e349A137c75")
                            .unwrap()
                            .try_into()
                            .unwrap(),
                    ), // Arbitrum
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// Single swap leg parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapLeg {
    /// DEX type for this leg
    pub dex_type: DexType,
    /// Pool address (for Uniswap V3)
    pub pool_address: String,
    /// Input token address
    pub token_in: String,
    /// Output token address
    pub token_out: String,
    /// Fee tier in basis points (e.g., 500 = 0.05%)
    pub fee_tier: u32,
    /// Minimum output amount (slippage protection)
    pub min_output: u128,
    /// Amount in (for exact input, use 0 for exact output)
    pub amount_in: u128,
}

/// Two-leg arbitrage route
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwoLegRoute {
    /// Loan token address
    pub loan_token: String,
    /// Flash loan amount
    pub loan_amount: u128,
    /// First swap leg (TokenA -> TokenB)
    pub leg1: SwapLeg,
    /// Second swap leg (TokenB -> TokenA)
    pub leg2: SwapLeg,
    /// Minimum profit threshold
    pub min_profit: u128,
    /// Validation errors
    pub validation_errors: Vec<String>,
}

impl TwoLegRoute {
    /// Create a new two-leg route
    pub fn new(loan_token: String, loan_amount: u128) -> Self {
        Self {
            loan_token,
            loan_amount,
            leg1: SwapLeg::default(),
            leg2: SwapLeg::default(),
            min_profit: 0,
            validation_errors: Vec::new(),
        }
    }

    /// Set leg 1
    pub fn with_leg1(mut self, leg: SwapLeg) -> Self {
        self.leg1 = leg;
        self
    }

    /// Set leg 2
    pub fn with_leg2(mut self, leg: SwapLeg) -> Self {
        self.leg2 = leg;
        self
    }

    /// Set minimum profit
    pub fn with_min_profit(mut self, min_profit: u128) -> Self {
        self.min_profit = min_profit;
        self
    }

    /// Validate the route and return errors
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();

        // Check leg1 token_out == leg2 token_in (token continuity)
        if self.leg1.token_out.to_lowercase() != self.leg2.token_in.to_lowercase() {
            errors.push(format!(
                "Token continuity error: leg1 outputs {} but leg2 expects {}",
                self.leg1.token_out, self.leg2.token_in
            ));
        }

        // Check leg2 token_out == loan_token (returns to start)
        if self.leg2.token_out.to_lowercase() != self.loan_token.to_lowercase() {
            errors.push(format!(
                "Route does not return to start: ends {} but started {}",
                self.leg2.token_out, self.loan_token
            ));
        }

        // Check leg1 token_in == loan_token
        if self.leg1.token_in.to_lowercase() != self.loan_token.to_lowercase() {
            errors.push(format!(
                "First leg must use loan token {} as input, got {}",
                self.loan_token, self.leg1.token_in
            ));
        }

        // Check pool addresses are valid
        if self.leg1.pool_address == "0x0000000000000000000000000000000000000000" {
            errors.push("Leg1 pool address is zero".to_string());
        }
        if self.leg2.pool_address == "0x0000000000000000000000000000000000000000" {
            errors.push("Leg2 pool address is zero".to_string());
        }

        // Check min_output > 0
        if self.leg1.min_output == 0 {
            errors.push("Leg1 min_output is zero - no slippage protection".to_string());
        }
        if self.leg2.min_output == 0 {
            errors.push("Leg2 min_output is zero - no slippage protection".to_string());
        }

        errors
    }

    /// Check if route is valid
    pub fn is_valid(&self) -> bool {
        self.validate().is_empty()
    }
}

impl Default for SwapLeg {
    fn default() -> Self {
        Self {
            dex_type: DexType::UniswapV3,
            pool_address: String::new(),
            token_in: String::new(),
            token_out: String::new(),
            fee_tier: 0,
            min_output: 0,
            amount_in: 0,
        }
    }
}

/// Executor configuration
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Executor contract address (local deployment for REVM)
    pub executor_address: [u8; 20],
    /// Balancer Vault address
    pub balancer_vault: [u8; 20],
    /// Uniswap V3 Router address
    pub uniswap_v3_router: [u8; 20],
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            executor_address: hex::decode("DEADBEEF00000000000000000000000000000001")
                .unwrap()
                .try_into()
                .unwrap(),
            balancer_vault: hex::decode("BA12222222228d8Ba445958a75a0704d566BF2C8")
                .unwrap()
                .try_into()
                .unwrap(),
            uniswap_v3_router: hex::decode("68b3465833fb72A70ecDF485E0e4C7bD8665Fc45")
                .unwrap()
                .try_into()
                .unwrap(),
        }
    }
}
