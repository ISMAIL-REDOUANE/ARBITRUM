//! # Mathematical Arbitrage Engine
//!
//! Dynamic trade size calculation and friction accounting for optimal arbitrage execution.
//!
//! ## Key Formulas
//!
//! ### Maximum Optimal Input Amount (Q_in)
//! For Uniswap V3, given sqrtPriceX96 and tick liquidity L:
//! $$Q_{in}^{max} = \frac{L \cdot \Delta\sqrt{P}}{sqrt{P}_{in} \cdot sqrt{P}_{out}}$$
//!
//! ### Price Impact Constraint
//! $$Slippage = \frac{\Delta P}{P} \leq 0.10\%$$
//!
//! ### Total Friction
//! $$F_{total} = F_{flash} + F_{dex} + F_{gas}$$
//! - Flash Loan Fee (Aave V3): 0.05% = 5 bps
//! - DEX Swap Fee (Uniswap V3 0.30% pool): 30 bps
//! - L2 Gas Cost: gas_price * est_gas
//!
//! ### Zero-Loss Execution Gate
//! $$Profit_{gross} > F_{total} + SafetyBuffer$$
//! where SafetyBuffer ≈ $1-2 USD to capture micro-spreads

use crate::types::PoolState;

pub const AAVE_V3_FLASH_LOAN_FEE_BPS: u64 = 5;
pub const SAFETY_BUFFER_USD: f64 = 1.50;

#[derive(Debug, Clone)]
pub struct FrictionBreakdown {
    pub flash_loan_fee_wei: u64,
    pub dex_swap_fee_wei: u64,
    pub gas_cost_wei: u64,
    pub total_friction_wei: u64,
}

#[derive(Debug, Clone)]
pub struct OptimalTradeSize {
    pub max_input_amount_wei: u64,
    pub estimated_output_wei: u64,
    pub price_impact_bps: u64,
    pub slippage_bps: u64,
}

#[derive(Debug, Clone)]
pub struct ArbitrageMath {
    pub sqrt_price_x96: u128,
    pub tick_liquidity: u128,
    pub current_tick: i32,
    pub fee_tier_bps: u32,
}

impl ArbitrageMath {
    pub fn from_pool_state(pool: &PoolState) -> Self {
        Self {
            sqrt_price_x96: pool.liquidity,
            tick_liquidity: pool.liquidity,
            current_tick: pool.current_tick.unwrap_or(0),
            fee_tier_bps: pool.fee_tier,
        }
    }

    pub fn with_tick_data(sqrt_price_x96: u128, tick_liquidity: u128, current_tick: i32, fee_tier_bps: u32) -> Self {
        Self {
            sqrt_price_x96,
            tick_liquidity,
            current_tick,
            fee_tier_bps,
        }
    }

    pub fn calc_sqrt_price_from_tick(tick: i32) -> u128 {
        let ratio = if tick >= 0 {
            1.0001_f64.powi(tick).sqrt() * (1u128 << 96) as f64
        } else {
            1.0 / 1.0001_f64.powi(-tick).sqrt() * (1u128 << 96) as f64
        };
        ratio as u128
    }

    pub fn calc_tick_from_sqrt_price(sqrt_price_x96: u128) -> i32 {
        let price = (sqrt_price_x96 as f64) / (1u128 << 96) as f64;
        let log_price = price.log10() / 0.0001_f64.log10();
        log_price as i32
    }

    #[inline(always)]
    pub fn get_sqrt_price(&self) -> u128 {
        self.sqrt_price_x96
    }

    #[inline(always)]
    pub fn get_liquidity(&self) -> u128 {
        self.tick_liquidity
    }

    pub fn max_input_for_slippage(&self, output_amount_wei: u64, max_slippage_bps: u64) -> u64 {
        if self.tick_liquidity == 0 || self.sqrt_price_x96 == 0 {
            return 0;
        }

        let price = (self.sqrt_price_x96 as f64) / (1u128 << 96) as f64;
        let slippage_factor = 1.0 - (max_slippage_bps as f64) / 10000.0;
        let adjusted_price = price * slippage_factor;

        let _delta_price = price - adjusted_price;
        let delta_sqrt_p = (price.sqrt() - adjusted_price.sqrt()).abs();

        let numerator = self.tick_liquidity * (delta_sqrt_p as u128);
        let denominator = (price.sqrt() * adjusted_price.sqrt()) as u128;

        if denominator == 0 {
            return 0;
        }

        let max_input = numerator * 1_000_000_000 / denominator;

        max_input.min(output_amount_wei as u128) as u64
    }

    pub fn calculate_optimal_trade_size(
        &self,
        input_token_decimals: u8,
        output_token_decimals: u8,
        max_slippage_bps: u64,
    ) -> OptimalTradeSize {
        if self.tick_liquidity == 0 {
            return OptimalTradeSize {
                max_input_amount_wei: 0,
                estimated_output_wei: 0,
                price_impact_bps: 0,
                slippage_bps: 0,
            };
        }

        let price_impact_bps: u64 = self.fee_tier_bps as u64;
        let slippage_bps: u64 = max_slippage_bps.min(price_impact_bps + 10);

        let max_input_raw = self.max_input_for_slippage(1_000_000_000_000_000_000u64, slippage_bps);

        let input_scale = 10u64.pow(input_token_decimals as u32);
        let _output_scale = 10u64.pow(output_token_decimals as u32);

        let max_input_wei = max_input_raw / input_scale * input_scale;
        let estimated_output = self.apply_swap(max_input_wei, input_token_decimals, output_token_decimals);

        OptimalTradeSize {
            max_input_amount_wei: max_input_wei,
            estimated_output_wei: estimated_output,
            price_impact_bps,
            slippage_bps,
        }
    }

    #[inline(always)]
    pub fn apply_swap(&self, amount_in_wei: u64, in_decimals: u8, out_decimals: u8) -> u64 {
        if amount_in_wei == 0 || self.tick_liquidity == 0 {
            return 0;
        }

        let amount_in_scaled = amount_in_wei as u128;

        // Uniswap V3 fee tiers are in hundredths of a bip (1,000,000 = 100%)
        let fee_multiplier = 1_000_000_u128.saturating_sub(self.fee_tier_bps as u128);
        let amount_in_after_fee = amount_in_scaled * fee_multiplier / 1_000_000;

        let sqrt_p = (self.sqrt_price_x96 as f64) / (1u128 << 96) as f64;
        let price = sqrt_p * sqrt_p; // Price ratio P = (sqrtPriceX96 / 2^96)^2
        let amount_out = amount_in_after_fee as f64 * price;

        let scale_diff = if out_decimals > in_decimals {
            10u64.pow((out_decimals - in_decimals) as u32) as f64
        } else {
            1.0 / 10u64.pow((in_decimals - out_decimals) as u32) as f64
        };

        (amount_out * scale_diff) as u64
    }

    pub fn calculate_friction(
        &self,
        input_amount_wei: u64,
        gas_price_gwei: u64,
        estimated_gas: u64,
    ) -> FrictionBreakdown {
        let flash_loan_fee = (input_amount_wei as u128) * (AAVE_V3_FLASH_LOAN_FEE_BPS as u128) / 10000;
        // Uniswap V3 fee tiers are in hundredths of a bip (1,000,000 = 100%)
        let dex_fee = (input_amount_wei as u128) * (self.fee_tier_bps as u128) / 1_000_000;
        let gas_cost_wei = gas_price_gwei * estimated_gas;

        let total = flash_loan_fee as u64 + dex_fee as u64 + gas_cost_wei;

        FrictionBreakdown {
            flash_loan_fee_wei: flash_loan_fee as u64,
            dex_swap_fee_wei: dex_fee as u64,
            gas_cost_wei,
            total_friction_wei: total,
        }
    }




    pub fn is_profitable(
        &self,
        gross_profit_wei: u64,
        input_amount_wei: u64,
        gas_price_gwei: u64,
        estimated_gas: u64,
        eth_usd_price: f64,
    ) -> bool {
        let friction = self.calculate_friction(input_amount_wei, gas_price_gwei, estimated_gas);

        let friction_usd = wei_to_usd(friction.total_friction_wei, 18, eth_usd_price);
        let gross_profit_usd = wei_to_usd(gross_profit_wei, 18, eth_usd_price);

        let net_profit_usd = gross_profit_usd - friction_usd;

        tracing::debug!(
            "Profit check: gross=${:.2}, friction=${:.2} (flash={}, dex={}, gas={}), net=${:.2}",
            gross_profit_usd,
            friction_usd,
            wei_to_usd(friction.flash_loan_fee_wei, 18, eth_usd_price),
            wei_to_usd(friction.dex_swap_fee_wei, 18, eth_usd_price),
            wei_to_usd(friction.gas_cost_wei, 18, eth_usd_price),
            net_profit_usd
        );

        net_profit_usd >= SAFETY_BUFFER_USD
    }
}

#[inline(always)]
pub fn wei_to_usd(wei: u64, decimals: u8, eth_usd_price: f64) -> f64 {
    let eth_amount = wei as f64 / 10u64.pow(decimals as u32) as f64;
    eth_amount * eth_usd_price
}

#[inline(always)]
pub fn usd_to_wei(usd: f64, decimals: u8, eth_usd_price: f64) -> u64 {
    let eth_amount = usd / eth_usd_price;
    (eth_amount * 10u64.pow(decimals as u32) as f64) as u64
}

pub fn calculate_arbitrage_profit(
    buy_price: f64,
    sell_price: f64,
    input_amount_wei: u64,
    input_decimals: u8,
) -> u64 {
    let input_eth = input_amount_wei as f64 / 10u64.pow(input_decimals as u32) as f64;
    let bought_amount = input_eth * buy_price;
    let sold_amount = input_eth * sell_price;
    let profit_eth = sold_amount - bought_amount;
    
    (profit_eth * 10u64.pow(18) as f64) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sqrt_price_from_tick() {
        let sqrt_p = ArbitrageMath::calc_sqrt_price_from_tick(0);
        assert!(sqrt_p > 0);

        let sqrt_p_1000 = ArbitrageMath::calc_sqrt_price_from_tick(1000);
        assert!(sqrt_p_1000 > sqrt_p);
    }

    #[test]
    fn test_tick_from_sqrt_price() {
        let sqrt_p: u128 = 79228162514264337593543950336;
        let tick = ArbitrageMath::calc_tick_from_sqrt_price(sqrt_p);
        assert!(tick.abs() < 10);
    }

    #[test]
    fn test_friction_calculation() {
        let math = ArbitrageMath::with_tick_data(
            79228162514264337593543950336u128,
            1_000_000_000_000_000_000u128,
            0,
            30,
        );

        let friction = math.calculate_friction(1_000_000_000_000_000_000u64, 50, 200_000);

        assert!(friction.flash_loan_fee_wei > 0);
        assert!(friction.dex_swap_fee_wei > 0);
        assert_eq!(friction.gas_cost_wei, 50 * 200_000);
    }

    #[test]
    fn test_max_input_for_slippage() {
        let math = ArbitrageMath::with_tick_data(
            79228162514264337593543950336u128,
            10_000_000_000_000_000_000u128,
            0,
            30,
        );

        let max_input = math.max_input_for_slippage(1_000_000_000_000_000_000u64, 10);
        assert!(max_input <= 1_000_000_000_000_000_000u64, "Max input should not exceed output amount");
    }
    #[test]
    fn test_v3_fee_tiers_mathematical_accuracy() {
        let amount = 1_000_000_000_000_000_000u64; // 1 token
        // Fee 500 = 0.05%
        let math_500 = ArbitrageMath::with_tick_data(79228162514264337593543950336u128, 1_000_000_000_000_000_000u128, 0, 500);
        let friction_500 = math_500.calculate_friction(amount, 0, 0);
        assert_eq!(friction_500.dex_swap_fee_wei, 500_000_000_000_000); // 0.05% of 1e18 = 5e14

        // Fee 1000 = 0.10%
        let math_1000 = ArbitrageMath::with_tick_data(79228162514264337593543950336u128, 1_000_000_000_000_000_000u128, 0, 1000);
        let friction_1000 = math_1000.calculate_friction(amount, 0, 0);
        assert_eq!(friction_1000.dex_swap_fee_wei, 1_000_000_000_000_000); // 0.10% of 1e18 = 1e15

        // Fee 3000 = 0.30%
        let math_3000 = ArbitrageMath::with_tick_data(79228162514264337593543950336u128, 1_000_000_000_000_000_000u128, 0, 3000);
        let friction_3000 = math_3000.calculate_friction(amount, 0, 0);
        assert_eq!(friction_3000.dex_swap_fee_wei, 3_000_000_000_000_000); // 0.30% of 1e18 = 3e15

        // Fee 10000 = 1.00%
        let math_10000 = ArbitrageMath::with_tick_data(79228162514264337593543950336u128, 1_000_000_000_000_000_000u128, 0, 10000);
        let friction_10000 = math_10000.calculate_friction(amount, 0, 0);
        assert_eq!(friction_10000.dex_swap_fee_wei, 10_000_000_000_000_000); // 1.00% of 1e18 = 1e16
    }


    #[test]
    fn test_wei_to_usd() {
        let result = wei_to_usd(1_000_000_000_000_000_000u64, 18, 3500.0);
        assert!((result - 3500.0).abs() < 1.0, "1 ETH at 3500 USD should be ~3500 USD");
    }

    #[test]
    fn test_usd_to_wei() {
        let result = usd_to_wei(3500.0, 18, 3500.0);
        let expected = 1_000_000_000_000_000_000u64;
        assert!((result as i64 - expected as i64).abs() < 1_000_000_000, "3500 USD should be ~1 ETH worth");
    }

    #[test]
    fn test_profitability_check() {
        let math = ArbitrageMath::with_tick_data(
            79228162514264337593543950336u128,
            5_000_000_000_000_000_000u128,
            0,
            30,
        );

        let profit_wei = 100_000_000_000_000_000u64;
        let is_profitable = math.is_profitable(profit_wei, 1_000_000_000_000_000_000u64, 50, 200_000, 3500.0);
        assert!(is_profitable);
    }

    #[test]
    fn test_apply_swap() {
        let math = ArbitrageMath::with_tick_data(
            79228162514264337593543950336u128,
            10_000_000_000_000_000_000u128,
            0,
            30,
        );

        let output = math.apply_swap(1_000_000_000_000_000_000u64, 18, 18);
        assert!(output > 0);
    }
}
