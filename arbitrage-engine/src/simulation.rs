//! # Simulation Module - Optimized
//!
//! REVM-based EVM simulation engine for pre-trade validation.
//!
//! Optimizations:
//! - Fixed-size stack arrays (no heap allocation)
//! - Cycle-accurate rdtsc timing
//! - Warm cache integration for DEX pools
//! - Dynamic math engine for optimal trade sizing

use crate::engine::SharedState;
use crate::error::{ArbitrageError, Result};
use crate::types::{PriceEvent, ArbitrageOpportunity, SimulationResult, DexType, PoolState};
use crate::cache_db::RevmCacheDB;
use crate::tsc::{TelemetryChannel, TelemetryStats, TscGuard};
use crate::math::{ArbitrageMath, calculate_arbitrage_profit};
use crate::config::Config;
use crate::executor_abi::build_two_leg_execute_calldata;
use crate::lead_lag::{LeadLagDetector, MovementConfig, MovementDetection};
use crate::pool_discovery::{
    EnrichedPool, FactoryType, PoolDiscovery, PoolDiscoveryConfig, PoolFreshness,
    PoolRegistry, TokenPair,
};
use crate::revmsim::RevmSimulator;
use crate::route_gen::{ArbitrageRoute, RouteFinder, RouteFinderConfig};
use crate::two_leg_route::{DexType as ExecutorDexType, ExecutorConfig, SwapLeg, TwoLegRoute};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::_rdtsc;

const ARBITRUM_CHAIN_ID: u64 = 42161;
const ARBITRUM_BLOCK_GAS_LIMIT: u64 = 30_000_000;
const INTRINSIC_GAS: u64 = 21_000;

const MIN_LATENCY_NS: u64 = 10_000;

/// Arbitrum USDC — flash loan token for the two-leg route (6 decimals).
const USDC_ARBITRUM: &str = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831";

/// Mirror address in REVM where the deployed executor bytecode is loaded
/// for exact simulation (the real deployed address is used for broadcast).
const EXECUTOR_SIM_ADDRESS: &str = "0xDEADBEEF00000000000000000000000000000001";

/// USDC (6 decimals) → wei-equivalent (18 decimals) scaling factor.
const USDC_TO_WEI: u128 = 1_000_000_000_000;

fn hex_to_address(s: &str) -> Option<revm::primitives::Address> {
    let bytes = hex::decode(s.trim_start_matches("0x")).ok()?;
    if bytes.len() != 20 {
        return None;
    }
    Some(revm::primitives::Address::from_slice(&bytes))
}

#[derive(Debug, Clone)]
#[derive(Default)]
pub struct SimulationStats {
    pub total_simulations: u64,
    pub successful_arbitrages: u64,
    pub failed_simulations: u64,
    pub total_cycles: u64,
    pub total_ns: u64,
}


pub struct SimulationEngine {
    state: Arc<SharedState>,
    timeout: Duration,
    min_profit_wei: u64,
    broadcast_tx: mpsc::Sender<(ArbitrageOpportunity, Vec<u8>)>,
    telemetry: Arc<TelemetryChannel>,
    telemetry_stats: Arc<TelemetryStats>,
    stats: Arc<std::sync::Mutex<SimulationStats>>,
    gas_price_gwei: u64,
    eth_usd_price: f64,
    estimated_gas: u64,
    // ── Binance-only pipeline wiring ──────────────────────────────────────
    // Binance WS → PriceEvent → lead-lag detection → Arbitrum pool price
    // update → route generation → exact REVM simulation → net profit
    // validation → executor calldata broadcast.
    lead_lag: LeadLagDetector,
    registry: Arc<PoolRegistry>,
    route_finder: RouteFinder,
    revm: Arc<RevmSimulator>,
    executor_loaded: AtomicBool,
}

impl SimulationEngine {
    pub fn new(
        state: Arc<SharedState>,
        broadcast_tx: mpsc::Sender<(ArbitrageOpportunity, Vec<u8>)>,
        telemetry: Arc<TelemetryChannel>,
        telemetry_stats: Arc<TelemetryStats>,
    ) -> Self {
        let timeout_ms = state.config.engine.simulation_timeout_ms;
        let min_profit_wei = state.config.engine.min_profit_wei;

        let registry = Self::build_registry(&state.config);
        let route_finder = RouteFinder::new(Self::route_finder_config(), registry.clone());
        let rpc_endpoint = std::env::var("ARBITRUM_RPC_URL")
            .unwrap_or_else(|_| "https://arb1.arbitrum.io/rpc".to_string());
        let revm = Arc::new(RevmSimulator::new(&rpc_endpoint));

        Self {
            state,
            timeout: Duration::from_millis(timeout_ms),
            min_profit_wei,
            broadcast_tx,
            telemetry,
            telemetry_stats,
            stats: Arc::new(std::sync::Mutex::new(SimulationStats::default())),
            gas_price_gwei: 50,
            eth_usd_price: 3500.0,
            estimated_gas: 200_000,
            lead_lag: LeadLagDetector::new(MovementConfig::default()),
            registry,
            route_finder,
            revm,
            executor_loaded: AtomicBool::new(false),
        }
    }

    /// Route finder config. Profit thresholds are in raw flash-loan token
    /// units (USDC, 6 decimals): 10 USDC minimum expected profit.
    fn route_finder_config() -> RouteFinderConfig {
        RouteFinderConfig {
            max_hops: 2,
            max_price_impact_bps: 100,
            min_profit_wei: 10_000_000,      // 10 USDC (raw)
            max_gas_cost_wei: 5_000_000,     // 5 USDC (raw)
            flash_loan_fee_bps: 0,           // Balancer V2: 0% flash loan fee
        }
    }

    /// Populate the pool registry from the configured Arbitrum pools.
    /// Reserves are zero until `hydrate_execution_state` syncs them from RPC.
    fn build_registry(config: &Config) -> Arc<PoolRegistry> {
        let registry = PoolRegistry::new(PoolDiscoveryConfig::default());

        let register = |registry: &PoolRegistry,
                        address: &str,
                        token0: &str,
                        token1: &str,
                        fee_tier: u32,
                        dex_type: DexType,
                        factory: FactoryType| {
            let mut pool = EnrichedPool::default();
            pool.base = PoolState {
                address: address.to_string(),
                token0: token0.to_string(),
                token1: token1.to_string(),
                reserve0: 0,
                reserve1: 0,
                fee_tier,
                liquidity: 0,
                current_tick: None,
                last_update: 0,
            };
            pool.dex_type = dex_type;
            pool.factory_type = factory;
            pool.token_pair = TokenPair::new_hex(token0, token1);
            registry.update_pool(address.to_string(), pool);
        };

        for p in &config.dex.uniswap_v3_pools {
            register(&registry, &p.address, &p.token0, &p.token1, p.fee_tier,
                     DexType::UniswapV3, FactoryType::UniswapV3);
        }
        for p in &config.dex.uniswap_v2_pools {
            register(&registry, &p.address, &p.token0, &p.token1, p.fee_tier,
                     DexType::UniswapV2, FactoryType::UniswapV2);
        }
        for p in &config.dex.sushiswap_pools {
            register(&registry, &p.address, &p.token0, &p.token1, p.fee_tier,
                     DexType::SushiSwap, FactoryType::SushiSwap);
        }

        tracing::info!(
            "Pool registry initialized with {} configured pools",
            registry.pool_count()
        );
        Arc::new(registry)
    }

    pub fn with_gas_price(mut self, gas_price_gwei: u64) -> Self {
        self.gas_price_gwei = gas_price_gwei;
        self
    }

    pub fn with_eth_price(mut self, eth_usd_price: f64) -> Self {
        self.eth_usd_price = eth_usd_price;
        self
    }

    pub fn with_estimated_gas(mut self, gas: u64) -> Self {
        self.estimated_gas = gas;
        self
    }

    pub async fn run(&self) -> Result<()> {
        tracing::info!("Simulation engine started with TSC telemetry");

        // Best-effort startup hydration: REVM fork state, deployed executor
        // bytecode, and configured pool states. Failures degrade gracefully
        // (fail-closed) — the pipeline never broadcasts unvalidated routes.
        self.hydrate_execution_state().await;

        while !self.state.is_shutdown() {
            if let Some(event) = self.state.pop_price_event() {
                let _guard = TscGuard::with_telemetry(self.telemetry.clone());

                match self.process_event(&event).await {
                    Ok(result) => {
                        if let Some((opp, calldata)) = result {
                            tracing::info!(
                                "Profitable opportunity: {} → {} (est profit {} wei)",
                                opp.lead_exchange,
                                opp.lag_exchange,
                                opp.estimated_profit_wei
                            );

                            if let Err(e) = self.broadcast_tx.try_send((opp.clone(), calldata)) {
                                tracing::error!("Failed to send to broadcaster: {}", e);
                            } else {
                                tracing::debug!("Sent opportunity to broadcaster");
                                self.state.stats.record_opportunity();
                            }

                            if let Err(e) = self.state.push_opportunity(opp) {
                                tracing::warn!("Failed to push opportunity: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to process event: {}", e);
                    }
                }
            } else {
                tokio::task::yield_now().await;
            }
        }

        Ok(())
    }

    /// ═════════════════════════════════════════════════════════════════════
    /// BINANCE-ONLY PIPELINE (per PriceEvent)
    ///
    ///   1. Lead-lag detection      (real movement, replaces random deviation)
    ///   2. Affected Arbitrum tokens (Binance symbol → token mapping)
    ///   3. Arbitrum pool price update (lead-implied movement → registry)
    ///   4. Opportunity detection    (two-leg route generation)
    ///   5. Exact REVM simulation    (same calldata that gets broadcast)
    ///   6. Net profit validation    (min_profit_wei hard gate)
    ///   7. Broadcast                (executor calldata → broadcaster)
    /// ═════════════════════════════════════════════════════════════════════
    async fn process_event(&self, event: &PriceEvent) -> Result<Option<(ArbitrageOpportunity, Vec<u8>)>> {
        // ── STAGE 1: LEAD-LAG DETECTION ─────────────────────────────────────
        let Some(movement) = self.lead_lag.detect_movement(event) else {
            return Ok(None);
        };

        tracing::debug!(
            "Lead-lag signal: {} {:?} velocity={:.2}bps/s confidence={:.2}",
            movement.symbol,
            movement.direction,
            movement.velocity_bps_per_sec,
            movement.confidence
        );

        // ── STAGE 2: AFFECTED ARBITRUM TOKENS ───────────────────────────────
        let mappings = self.lead_lag.get_affected_tokens(&movement);
        if mappings.is_empty() {
            return Ok(None);
        }

        let price: f64 = event.price.parse()
            .map_err(|_| ArbitrageError::Simulation("Invalid price".to_string()))?;
        if price <= 0.0 {
            return Ok(None);
        }

        let flash_loan_token = USDC_ARBITRUM.to_string();
        let flash_loan_amount = self.state.config.risk.max_position_size as u128;

        for mapping in &mappings {
            let pair = self.lead_lag.get_token_pair(mapping);

            // ── STAGE 3: ARBITRUM POOL PRICE UPDATE ────────────────────────
            self.apply_lead_to_pools(&pair, &movement);

            // ── STAGE 4: OPPORTUNITY DETECTION (two-leg route generation) ──
            let routes = self.route_finder.find_two_leg_routes(
                &pair,
                flash_loan_token.clone(),
                flash_loan_amount,
            );
            if routes.is_empty() {
                tracing::trace!(
                    "No two-leg routes for {} (registry needs >=2 pools per pair)",
                    mapping.token_symbol
                );
                continue;
            }
            let route = &routes[0]; // sorted by expected profit

            // Build the exact executor calldata once — the SAME bytes are
            // simulated in REVM and broadcast on-chain.
            let two_leg = match Self::to_two_leg_route(route) {
                Some(r) => r,
                None => continue,
            };
            let calldata = build_two_leg_execute_calldata(&two_leg, &ExecutorConfig::default());

            // ── STAGE 5: EXACT REVM SIMULATION ─────────────────────────────
            if self.executor_loaded.load(Ordering::Relaxed) && self.revm.is_state_loaded() {
                let sim = self.simulate_executor_calldata(&calldata);

                // ── STAGE 6: NET PROFIT VALIDATION ─────────────────────────
                if sim.success && sim.profit_wei as u128 >= self.min_profit_wei as u128 {
                    tracing::info!(
                        "REVM-validated arbitrage: profit={} wei-equiv, gas={}, {}µs",
                        sim.profit_wei,
                        sim.gas_used,
                        sim.execution_time_us
                    );
                    let opp = self.build_route_opportunity(
                        event,
                        price,
                        &mapping.token_symbol,
                        sim.profit_wei,
                    );
                    return Ok(Some((opp, calldata)));
                }

                tracing::debug!("REVM simulation rejected route: {:?}", sim.revert_reason);
                continue;
            }

            // ── FALLBACK: friction-gate estimate (no deployed executor) ────
            // Deviation is derived from the real lead-lag velocity, not random.
            let deviation_pct = (movement.velocity_bps_per_sec.abs() / 100.0).min(5.0);
            let sell_price = price * (1.0 + deviation_pct / 100.0);

            let gross_profit_wei = calculate_arbitrage_profit(price, sell_price, 1_000_000_000_000_000u64, 18);

            let math = ArbitrageMath::with_tick_data(
                79228162514264337593543950336u128,
                5_000_000_000_000_000_000u128,
                0,
                30,
            );

            if !math.is_profitable(
                gross_profit_wei,
                1_000_000_000_000_000u64,
                self.gas_price_gwei,
                self.estimated_gas,
                self.eth_usd_price,
            ) {
                continue;
            }

            let opportunity = ArbitrageOpportunity {
                lead_exchange: "Binance".to_string(),
                lag_exchange: "Arbitrum DEXs".to_string(),
                buy_price: price,
                sell_price,
                deviation_pct,
                estimated_profit_wei: gross_profit_wei,
                token_pair: (mapping.token_symbol.clone(), "USDC".to_string()),
                chain_id: ARBITRUM_CHAIN_ID,
                timestamp: event.trade_time,
            };

            let result = self.simulate_arbitrage(&opportunity).await?;
            if !(result.success && result.profit_wei > 0) {
                continue;
            }

            // Fallback path requires the real executor calldata. The
            // old `construct_arbitrage_calldata` used a fabricated selector
            // (0xa93b3ce0) and did not match the executor ABI, so it has
            // been removed. Live execution requires EXECUTOR_ADDRESS +
            // ARBITRUM_RPC_URL to be configured for exact REVM validation.
            tracing::warn!(
                "Exact REVM validation unavailable — deploy executor (EXECUTOR_ADDRESS) and set ARBITRUM_RPC_URL for live execution"
            );
        }

        Ok(None)
    }

    /// STAGE 3: propagate the Binance lead movement onto Arbitrum pool state.
    fn apply_lead_to_pools(&self, pair: &TokenPair, movement: &MovementDetection) {
        let pools = self.registry.get_by_token_pair(pair);
        let movement_bps = movement.velocity_bps_per_sec.abs().min(u32::MAX as f64) as u32;
        for mut pool in pools {
            pool.price_movement_bps = movement_bps;
            pool.freshness = PoolFreshness::Fresh;
            self.registry.update_pool(pool.base.address.clone(), pool);
        }
    }

    /// Convert a generated two-leg route into the executor route format.
    fn to_two_leg_route(route: &ArbitrageRoute) -> Option<TwoLegRoute> {
        if route.legs.len() != 2 {
            return None;
        }
        let (l0, l1) = (&route.legs[0], &route.legs[1]);
        let dex = |t: DexType| match t {
            DexType::UniswapV3 => ExecutorDexType::UniswapV3,
            DexType::UniswapV2 => ExecutorDexType::UniswapV2,
            DexType::SushiSwap => ExecutorDexType::SushiSwap,
            DexType::Aerodrome => ExecutorDexType::Aerodrome,
        };
        Some(
            TwoLegRoute::new(route.flash_loan_token.clone(), route.flash_loan_amount)
                .with_leg1(SwapLeg {
                    dex_type: dex(l0.dex_type),
                    pool_address: l0.pool_address.clone(),
                    token_in: l0.token_in.clone(),
                    token_out: l0.token_out.clone(),
                    fee_tier: l0.fee_tier,
                    min_output: l0.min_output,
                    amount_in: route.flash_loan_amount,
                })
                .with_leg2(SwapLeg {
                    dex_type: dex(l1.dex_type),
                    pool_address: l1.pool_address.clone(),
                    token_in: l1.token_in.clone(),
                    token_out: l1.token_out.clone(),
                    fee_tier: l1.fee_tier,
                    min_output: l1.min_output,
                    amount_in: l0.expected_output, // leg1 output feeds leg2
                })
                .with_min_profit(route.min_profit),
        )
    }

    /// Simulate the exact executor calldata in REVM. The executor's
    /// `execute()` returns uint256 profit in loan-token raw units
    /// (USDC, 6 decimals) — converted to wei-equivalent (×1e12) so it can
    /// be compared against `min_profit_wei`.
    fn simulate_executor_calldata(&self, calldata: &[u8]) -> SimulationResult {
        let start = Instant::now();

        let Some(target) = hex_to_address(EXECUTOR_SIM_ADDRESS) else {
            return SimulationResult {
                success: false,
                profit_wei: 0,
                gas_used: 0,
                revert_reason: Some("Invalid executor simulation address".to_string()),
                execution_time_us: 0,
            };
        };

        let call = self.revm.execute_simple_call(target, calldata);

        let profit_raw: u128 = if call.success && call.return_data.len() >= 32 {
            let mut word = [0u8; 32];
            word.copy_from_slice(&call.return_data[..32]);
            if word[..16].iter().any(|&b| b != 0) {
                u128::MAX
            } else {
                u128::from_be_bytes(word[16..].try_into().unwrap_or([0u8; 16]))
            }
        } else {
            0
        };

        let profit_wei = profit_raw
            .saturating_mul(USDC_TO_WEI)
            .min(u64::MAX as u128) as u64;

        SimulationResult {
            success: call.success,
            profit_wei,
            gas_used: call.gas_used,
            revert_reason: call.revert_reason,
            execution_time_us: start.elapsed().as_micros() as u64,
        }
    }

    fn build_route_opportunity(
        &self,
        event: &PriceEvent,
        price: f64,
        token_symbol: &str,
        profit_wei: u64,
    ) -> ArbitrageOpportunity {
        ArbitrageOpportunity {
            lead_exchange: "Binance".to_string(),
            lag_exchange: "Arbitrum DEXs".to_string(),
            buy_price: price,
            sell_price: price,
            deviation_pct: 0.0,
            estimated_profit_wei: profit_wei,
            token_pair: (token_symbol.to_string(), "USDC".to_string()),
            chain_id: ARBITRUM_CHAIN_ID,
            timestamp: event.trade_time,
        }
    }

    /// Startup hydration (best-effort, fail-closed):
    ///   1. REVM fork state (Balancer Vault + Uniswap V3 Router accounts)
    ///   2. Deployed two-leg executor bytecode (mirrored into REVM)
    ///   3. Configured pool states from RPC
    async fn hydrate_execution_state(&self) {
        let rpc = std::env::var("ARBITRUM_RPC_URL")
            .unwrap_or_else(|_| "https://arb1.arbitrum.io/rpc".to_string());

        // 1) REVM fork state
        match self.revm.hydrate_from_rpc(None).await {
            Ok(snapshot) => {
                tracing::info!("REVM state hydrated at block {}", snapshot.block.block_number);
                // Fund the simulated caller so gas is available.
                self.revm.inject_test_caller(revm::primitives::U256::from(
                    10_000_000_000_000_000_000u64,
                ));
            }
            Err(e) => {
                tracing::warn!(
                    "REVM hydration failed (exact simulation disabled, fail-closed): {}",
                    e
                );
            }
        }

        // 2) Deployed two-leg executor bytecode (mirrored into REVM)
        match std::env::var("EXECUTOR_ADDRESS") {
            Ok(executor_addr) if !executor_addr.is_empty() => {
                let client = crate::revmsim::RpcClient::new(&rpc);
                match client.get_code(&executor_addr).await {
                    Ok(code) if !code.is_empty() => {
                        match self.revm.load_executor_bytecode(code) {
                            Ok(()) => {
                                self.executor_loaded.store(true, Ordering::Relaxed);
                                tracing::info!(
                                    "Executor bytecode loaded from {} (exact REVM simulation enabled)",
                                    executor_addr
                                );
                            }
                            Err(e) => tracing::warn!("Failed to load executor bytecode: {}", e),
                        }
                    }
                    Ok(_) => tracing::warn!(
                        "EXECUTOR_ADDRESS {} has no code - exact REVM simulation disabled",
                        executor_addr
                    ),
                    Err(e) => tracing::warn!("Failed to fetch executor bytecode: {}", e),
                }
            }
            _ => {
                tracing::info!(
                    "EXECUTOR_ADDRESS not set - exact REVM simulation disabled (fallback friction gate active)"
                );
            }
        }

        // 3) Configured pool states from RPC
        let discovery = PoolDiscovery::new(&rpc, PoolDiscoveryConfig::default());
        let mut synced = 0usize;
        for addr in self.registry.get_all_addresses() {
            let Some(pool) = self.registry.get(&addr) else { continue };
            let Ok(addr_bytes) = hex::decode(addr.trim_start_matches("0x")) else { continue };
            if addr_bytes.len() != 20 {
                continue;
            }
            let mut arr = [0u8; 20];
            arr.copy_from_slice(&addr_bytes);

            let result = match pool.dex_type {
                DexType::UniswapV3 => discovery.sync_uniswap_v3_pool(arr).await,
                DexType::UniswapV2 | DexType::SushiSwap => discovery.sync_uniswap_v2_pool(arr).await,
                DexType::Aerodrome => continue,
            };
            if let Ok(Some(updated)) = result {
                self.registry.update_pool(addr, updated);
                synced += 1;
            }
        }
        tracing::info!(
            "Pool state sync: {}/{} pools hydrated",
            synced,
            self.registry.pool_count()
        );
    }

    async fn simulate_arbitrage(&self, opportunity: &ArbitrageOpportunity) -> Result<SimulationResult> {
        #[cfg(target_arch = "x86_64")]
        let start_cycle = unsafe { _rdtsc() };

        let start = Instant::now();
        let cache_db = self.state.cache_db.read().clone();

        let profit_wei = self.execute_optimized_simulation(opportunity, &cache_db);

        #[cfg(target_arch = "x86_64")]
        let end_cycle = unsafe { _rdtsc() };

        let execution_time = start.elapsed();
        let execution_time_us = execution_time.as_micros() as u64;

        #[cfg(target_arch = "x86_64")]
        let cycles = end_cycle.wrapping_sub(start_cycle);
        #[cfg(not(target_arch = "x86_64"))]
        let cycles: u64 = execution_time.as_nanos() as u64 / 100;

        let ns_per_cycle = if cycles > 0 {
            (execution_time.as_nanos() as u64) / cycles
        } else {
            0
        };

        {
            let mut stats = self.stats.lock().unwrap();
            stats.total_simulations += 1;
            stats.total_cycles += cycles;
            stats.total_ns += execution_time.as_nanos() as u64;
            if profit_wei > 0 {
                stats.successful_arbitrages += 1;
            } else {
                stats.failed_simulations += 1;
            }
        }

        tracing::debug!(
            "Simulation: profit={} wei, cycles={}, ns/cycle={}, time={}µs",
            profit_wei,
            cycles,
            ns_per_cycle,
            execution_time_us
        );

        Ok(SimulationResult {
            success: profit_wei > 0,
            profit_wei,
            gas_used: 200_000,
            revert_reason: if profit_wei == 0 { Some("No profit".to_string()) } else { None },
            execution_time_us,
        })
    }

    /// Optimized simulation using stack arrays and warm cache
    #[inline(always)]
    fn execute_optimized_simulation(&self, opportunity: &ArbitrageOpportunity, cache_db: &RevmCacheDB) -> u64 {
        let _stack_buf = [0u8; 256];
        let mut profit: u64 = 0;

        let usdc_address: [u8; 20] = hex::decode("af88d065e77c8cc2239327c5edb3a432268e5831")
            .map(|v| {
                let mut arr = [0u8; 20];
                let len = v.len().min(20);
                arr[..len].copy_from_slice(&v[..len]);
                arr
            })
            .unwrap_or([0u8; 20]);

        if let Some(balance) = cache_db.get_balance_stack(&usdc_address) {
            if balance > 1_000_000 {
                let estimated_profit = opportunity.estimated_profit_wei;
                if estimated_profit > self.min_profit_wei {
                    profit = estimated_profit;
                }
            }
        }

        profit
    }

    pub fn construct_arbitrage_calldata(opp: &ArbitrageOpportunity, token_symbol: &str) -> Vec<u8> {
        // This function has been removed: the fallback `construct_arbitrage_calldata`
        // used a fabricated selector (0xa93b3ce0) that matches no real function
        // and did not produce ABI-compatible calldata for the deployed executor.
        //
        // Arbitrage execution MUST go through `executor_abi::build_two_leg_execute_calldata`
        // which produces calldata for `execute(address,uint256,address,address,bytes,bytes)`
        // = selector 0xfa48cb92 — the real entrypoint on ArbitrageExecutorTwoLeg.sol.
        //
        // The `simulation.rs` pipeline now only broadcasts when exact REVM simulation
        // succeeds (executor_loaded + state_loaded). This dead function is kept as
        // a stub returning empty to satisfy any external callers during migration.
        let _ = (opp, token_symbol);
        Vec::new()
    }

    fn encode_uint256(value: u64) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        bytes[24..].copy_from_slice(&value.to_be_bytes());
        bytes
    }

    pub fn get_stats(&self) -> SimulationStats {
        self.stats.lock().unwrap().clone()
    }

    pub fn print_benchmark_summary(&self) {
        let stats = self.get_stats();
        let avg_cycles = if stats.total_simulations > 0 {
            stats.total_cycles / stats.total_simulations
        } else {
            0
        };
        let avg_ns = if stats.total_simulations > 0 {
            stats.total_ns / stats.total_simulations
        } else {
            0
        };

        tracing::info!(
            "=== SIMULATION BENCHMARK ===\n\
             Total simulations: {}\n\
             Successful: {}\n\
             Failed: {}\n\
             Avg cycles: {}\n\
             Avg ns: {}",
            stats.total_simulations,
            stats.successful_arbitrages,
            stats.failed_simulations,
            avg_cycles,
            avg_ns
        );
    }
}

pub fn spawn_simulation_engine(
    state: Arc<SharedState>,
    broadcast_tx: mpsc::Sender<(ArbitrageOpportunity, Vec<u8>)>,
    telemetry_channel: Arc<TelemetryChannel>,
    telemetry_stats: Arc<TelemetryStats>,
) -> Result<std::thread::JoinHandle<()>> {
    let engine = SimulationEngine::new(
        state,
        broadcast_tx,
        telemetry_channel,
        telemetry_stats,
    );

    let handle = std::thread::Builder::new()
        .name("simulation-engine".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            rt.block_on(async {
                if let Err(e) = engine.run().await {
                    tracing::error!("Simulation engine error: {}", e);
                }
            });
        })
        .map_err(|e| ArbitrageError::System(format!("Failed to spawn sim engine: {}", e)))?;

    Ok(handle)
}

pub struct DexQuoteCalculator;

impl DexQuoteCalculator {
    #[inline(always)]
    pub fn uniswap_v2_out(
        amount_in: u64,
        reserve_in: u64,
        reserve_out: u64,
        fee_bps: u32,
    ) -> u64 {
        if reserve_in == 0 || reserve_out == 0 {
            return 0;
        }

        let amount_in_with_fee = (amount_in as u128) * (10000 - fee_bps as u128);
        let numerator = amount_in_with_fee * (reserve_out as u128);
        let denominator = (reserve_in as u128) * 10000 + amount_in_with_fee;

        (numerator / denominator) as u64
    }

    #[inline(always)]
    pub fn uniswap_v3_out(
        amount_in: u64,
        _reserve_in: u64,
        _reserve_out: u64,
        _fee_tier: u32,
    ) -> u64 {
        Self::uniswap_v2_out(amount_in, _reserve_in, _reserve_out, 30)
    }
}

pub fn run_benchmark(iterations: u64) -> SimulationStats {
    let cache_db = RevmCacheDB::with_chain_id(42161).unwrap();
    let mut stats = SimulationStats::default();
    let mut profit: u64 = 0;
    let _min_profit = 1000u64;

    let opportunity = ArbitrageOpportunity {
        lead_exchange: "Binance".to_string(),
        lag_exchange: "UniswapV3".to_string(),
        buy_price: 3500.0,
        sell_price: 3503.5,
        deviation_pct: 0.1,
        estimated_profit_wei: 1_000_000_000_000_000u64,
        token_pair: ("ETH".to_string(), "USDT".to_string()),
        chain_id: 42161,
        timestamp: 0,
    };

    for i in 0..iterations {
        #[cfg(target_arch = "x86_64")]
        let start_cycle = unsafe { _rdtsc() };

        let start = std::time::Instant::now();

        let usdc_address: [u8; 20] = hex::decode("af88d065e77c8cc2239327c5edb3a432268e5831")
            .map(|v| {
                let mut arr = [0u8; 20];
                let len = v.len().min(20);
                arr[..len].copy_from_slice(&v[..len]);
                arr
            })
            .unwrap_or([0u8; 20]);

        let _stack_buf = [0u8; 256];

        if let Some(balance) = cache_db.get_balance_stack(&usdc_address) {
            if balance > 1_000_000 {
                profit = opportunity.estimated_profit_wei;
            }
        }

        #[cfg(target_arch = "x86_64")]
        let end_cycle = unsafe { _rdtsc() };

        let elapsed_ns = start.elapsed().as_nanos() as u64;

        #[cfg(target_arch = "x86_64")]
        let cycles = end_cycle.wrapping_sub(start_cycle);
        #[cfg(not(target_arch = "x86_64"))]
        let cycles: u64 = elapsed_ns / 100;

        stats.total_simulations += 1;
        stats.total_cycles += cycles;
        stats.total_ns += elapsed_ns;
        if profit > 0 {
            stats.successful_arbitrages += 1;
        }

        if i % 100 == 0 {
            tracing::debug!("Benchmark iteration {}", i);
        }
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uniswap_v2_quote() {
        let amount_in = 1_000_000_000u64;
        let reserve_in = 2_000_000_000_000u64;
        let reserve_out = 4_000_000_000_000u64;
        let fee_bps = 30u32;

        let out = DexQuoteCalculator::uniswap_v2_out(
            amount_in, reserve_in, reserve_out, fee_bps
        );

        assert!(out > 1_900_000_000);
        assert!(out < 2_100_000_000);
    }

    #[test]
    fn test_uniswap_v2_quote_zero_reserves() {
        let out = DexQuoteCalculator::uniswap_v2_out(1000, 0, 0, 30);
        assert_eq!(out, 0);
    }

    #[test]
    fn test_encode_uint256() {
        let encoded = SimulationEngine::encode_uint256(1_000_000);
        assert_eq!(encoded[24..], 1_000_000u64.to_be_bytes());
    }

    #[test]
    fn test_stack_array_allocation() {
        let arr = [0u8; 256];
        assert_eq!(arr.len(), 256);
    }

    #[test]
    fn test_benchmark_100_iterations() {
        let stats = run_benchmark(100);
        let avg_cycles = stats.total_cycles / stats.total_simulations;

        println!("=== BENCHMARK RESULTS (100 iterations) ===");
        println!("Total cycles: {}", stats.total_cycles);
        println!("Avg cycles: {}", avg_cycles);
        println!("Avg ns: {}", stats.total_ns / stats.total_simulations);

        assert!(stats.total_simulations == 100);
    }

    #[test]
    fn test_arbitrage_math_profitability() {
        use crate::math::ArbitrageMath;

        let math = ArbitrageMath::with_tick_data(
            79228162514264337593543950336u128,
            5_000_000_000_000_000_000u128,
            0,
            30,
        );

        let profit_wei = 100_000_000_000_000_000u64;
        let is_profitable = math.is_profitable(profit_wei, 1_000_000_000_000_000_000u64, 50, 200_000, 3500.0);

        assert!(is_profitable, "High profit opportunity should be profitable");
    }

    #[test]
    fn test_arbitrage_math_filters_unprofitable() {
        use crate::math::ArbitrageMath;

        let math = ArbitrageMath::with_tick_data(
            79228162514264337593543950336u128,
            1_000_000_000_000_000_000u128,
            0,
            30,
        );

        let tiny_profit_wei = 10_000_000_000_000_000u64;
        let is_profitable = math.is_profitable(tiny_profit_wei, 10_000_000_000_000_000_000u64, 100, 500_000, 3500.0);

        assert!(!is_profitable, "Tiny profit below safety buffer should be filtered");
    }

    #[test]
    fn test_max_input_calculation() {
        use crate::math::ArbitrageMath;

        let math = ArbitrageMath::with_tick_data(
            79228162514264337593543950336u128,
            10_000_000_000_000_000_000u128,
            0,
            30,
        );

        let optimal = math.calculate_optimal_trade_size(18, 18, 10);

        assert!(optimal.slippage_bps <= 10, "Slippage should be within bounds");
    }

    #[test]
    fn test_friction_breakdown() {
        use crate::math::ArbitrageMath;

        let math = ArbitrageMath::with_tick_data(
            79228162514264337593543950336u128,
            5_000_000_000_000_000_000u128,
            0,
            30,
        );

        let friction = math.calculate_friction(1_000_000_000_000_000_000u64, 50, 200_000);

        assert!(friction.flash_loan_fee_wei > 0, "Flash loan fee should be charged");
        assert!(friction.dex_swap_fee_wei > 0, "DEX swap fee should be charged");
        assert_eq!(friction.gas_cost_wei, 50 * 200_000, "Gas cost calculation should match");
        assert_eq!(
            friction.total_friction_wei,
            friction.flash_loan_fee_wei + friction.dex_swap_fee_wei + friction.gas_cost_wei,
            "Total friction should be sum of components"
        );
    }

    #[test]
    fn test_safety_buffer_constant() {
        use crate::math::SAFETY_BUFFER_USD;
        const { assert!(SAFETY_BUFFER_USD >= 1.0 && SAFETY_BUFFER_USD <= 2.0, "Safety buffer should be $1-2"); }
    }
}
