//! # Lead-Lag Arbitrage Engine - Main Entry Point
//!
//! Sub-millisecond CEX-DEX Lead-Lag Arbitrage Engine for Ethereum L2s
//! Targets: Base, Arbitrum
//!
//! ## Usage
//!
//! ```bash
//! arbitrage-engine --config config.toml
//! ```
//!
//! ## Environment Variables
//!
//! Required:
//! - `PRIVATE_KEY` or `PRIVATE_KEY_FILE`: EOA private key for signing
//!
//! Optional:
//! - `ARBITRUM_RPC_URL`: Override default Arbitrum RPC
//! - `ARBITRUM_BROADCAST_RPC`: Private RPC for transaction broadcast
//! - `BLOXROUTE_RPC`: Arbitrum RPC for REVM fork testing

use lead_lag_arbitrage::{
    config::Config,
    engine::{CorePinning, SharedState},
    cache_db::RevmCacheDB,
    websocket, simulation, hydration,
    broadcaster, sender, tsc,
};

use anyhow::Result;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use std::sync::Arc;
use tokio::signal;

#[tokio::main]
async fn main() -> Result<()> {
    // ─────────────────────────────────────────────────────────────────────────
    // INITIALIZE TRACING/LOGGING
    // ─────────────────────────────────────────────────────────────────────────
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer().with_target(true))
        .init();

    tracing::info!("Lead-Lag Arbitrage Engine v{}", env!("CARGO_PKG_VERSION"));
    tracing::info!("Target chains: Base, Arbitrum");

    // ─────────────────────────────────────────────────────────────────────────
    // PHASE 0: TSC CALIBRATION (before threads start)
    // ─────────────────────────────────────────────────────────────────────────
    tracing::info!("═══════════════════════════════════════════════════════════════");
    tracing::info!("PHASE 0: TSC CALIBRATION");
    tracing::info!("Calibrating hardware cycle counter for latency measurement");
    tracing::info!("═══════════════════════════════════════════════════════════════");
    
    tsc::calibrate();
    
    // Create telemetry channel and stats
    let (telemetry_channel, telemetry_rx) = tsc::TelemetryChannel::new(10000);
    let telemetry_channel = Arc::new(telemetry_channel);
    let telemetry_stats = Arc::new(tsc::TelemetryStats::new());
    
    // Spawn telemetry logging thread
    let telemetry_handle = tsc::spawn_telemetry_logger(telemetry_rx, telemetry_stats.clone());
    tracing::info!("TSC telemetry logger spawned");

    // ─────────────────────────────────────────────────────────────────────────
    // LOAD CONFIGURATION
    // ─────────────────────────────────────────────────────────────────────────
    let config = Config::load()?;
    tracing::info!("Configuration loaded successfully");

    // ─────────────────────────────────────────────────────────────────────────
    // INITIALIZE CORE PINNING
    // ─────────────────────────────────────────────────────────────────────────
    let core_pinning = CorePinning {
        ws_core: 2,
        sim_core: 3,
        min_cores: 4,
    };
    tracing::info!(
        "Core pinning configured: WS={}, Sim={}",
        core_pinning.ws_core,
        core_pinning.sim_core
    );

    // ─────────────────────────────────────────────────────────────────────────
    // CREATE SHARED STATE
    // ─────────────────────────────────────────────────────────────────────────
    let shared_state = Arc::new(SharedState::new(config.clone(), core_pinning)?);
    tracing::info!("Shared state initialized");

    // ─────────────────────────────────────────────────────────────────────────
    // INITIALIZE CACHE DB
    // ─────────────────────────────────────────────────────────────────────────
    let cache_db = RevmCacheDB::new()?;
    tracing::info!("RevmCacheDB created (empty)");

    // ─────────────────────────────────────────────────────────────────────────
    // PHASE 1: STATE HYDRATION (BLOCKS UNTIL COMPLETE)
    // ─────────────────────────────────────────────────────────────────────────
    tracing::info!("═══════════════════════════════════════════════════════════════");
    tracing::info!("PHASE 1: STATE HYDRATION");
    tracing::info!("Fetching on-chain state from Arbitrum RPC...");
    tracing::info!("═══════════════════════════════════════════════════════════════");
    
    let hydration_start = std::time::Instant::now();
    
    match hydration::hydrate_state_with_fallback(&mut cache_db.clone()).await {
        Ok(state) => {
            tracing::info!(
                "State hydration complete in {}ms: {} contracts, {} storage slots",
                state.time_elapsed_ms,
                state.contracts_loaded,
                state.storage_slots_loaded
            );
        }
        Err(e) => {
            tracing::warn!(
                "State hydration failed: {}. Continuing with empty CacheDB.",
                e
            );
            tracing::warn!("Simulations may fail due to missing contract state.");
        }
    }
    
    let hydration_elapsed = hydration_start.elapsed().as_millis();
    tracing::info!("Hydration phase completed in {}ms", hydration_elapsed);

    // ─────────────────────────────────────────────────────────────────────────
    // INJECT HYDRATED CACHE DB INTO SHARED STATE
    // ─────────────────────────────────────────────────────────────────────────
    {
        let mut cache = shared_state.cache_db.write();
        *cache = cache_db;
    }
    tracing::info!("Hydrated CacheDB injected into shared state");

    // ─────────────────────────────────────────────────────────────────────────
    // PHASE 2: SPAWN BROADCASTER THREAD (before sim thread)
    // ─────────────────────────────────────────────────────────────────────────
    tracing::info!("═══════════════════════════════════════════════════════════════");
    tracing::info!("PHASE 2: STARTING BROADCASTER");
    tracing::info!("═══════════════════════════════════════════════════════════════");

    let (broadcast_tx, broadcast_handle) = if broadcaster::is_shadow_mode() {
        let (tx, handle) = broadcaster::spawn_shadow_broadcaster(shared_state.clone());
        tracing::info!("SHADOW MODE: Transaction broadcaster spawned (shadow mode)");
        (tx, Some(handle))
    } else {
        let (tx, handle) = broadcaster::spawn_broadcaster(shared_state.clone())?;
        tracing::info!("Transaction broadcaster spawned");
        (tx, Some(handle))
    };

    // ─────────────────────────────────────────────────────────────────────────
    // PHASE 3: SPAWN WEBSOCKET LISTENER THREAD (Core 2)
    // ─────────────────────────────────────────────────────────────────────────
    tracing::info!("═══════════════════════════════════════════════════════════════");
    tracing::info!("PHASE 3: STARTING WEBSOCKET LISTENER (Core 2)");
    tracing::info!("═══════════════════════════════════════════════════════════════");
    
    let ws_handle = websocket::spawn_binance_listener(shared_state.clone())?;
    tracing::info!("WebSocket listener spawned");

    // ─────────────────────────────────────────────────────────────────────────
    // PHASE 4: SPAWN SIMULATION THREAD (Core 3)
    // ─────────────────────────────────────────────────────────────────────────
    tracing::info!("═══════════════════════════════════════════════════════════════");
    tracing::info!("PHASE 4: STARTING SIMULATION ENGINE (Core 3)");
    tracing::info!("═══════════════════════════════════════════════════════════════");
    
    let sim_handle = simulation::spawn_simulation_engine(
        shared_state.clone(),
        broadcast_tx,
        telemetry_channel.clone(),
        telemetry_stats.clone(),
    )?;
    tracing::info!("Simulation engine spawned with TSC telemetry");

    // ─────────────────────────────────────────────────────────────────────────
    // PHASE 5: SPAWN SENDER THREAD (legacy, if needed)
    // ─────────────────────────────────────────────────────────────────────────
    tracing::info!("═══════════════════════════════════════════════════════════════");
    tracing::info!("PHASE 5: STARTING SENDER");
    tracing::info!("═══════════════════════════════════════════════════════════════");
    
    let sender_handle = sender::spawn_sender(shared_state.clone())?;
    tracing::info!("Sender thread spawned");

    // ─────────────────────────────────────────────────────────────────────────
    // ALL THREADS STARTED
    // ─────────────────────────────────────────────────────────────────────────
    tracing::info!("═══════════════════════════════════════════════════════════════");
    tracing::info!("ALL THREADS STARTED SUCCESSFULLY");
    tracing::info!("Engine ready - waiting for Binance price signals...");
    tracing::info!("═══════════════════════════════════════════════════════════════");
    
    tracing::info!("");
    tracing::info!("Architecture summary:");
    tracing::info!("  ├── Core 0: Telemetry Logger (async logging)");
    tracing::info!("  ├── Core 2: WebSocket Listener (Binance Lead Signal)");
    tracing::info!("  ├── Core 3: Simulation Engine (REVM + TSC Timing)");
    tracing::info!("  ├── Core 4: Sender/Broadcaster (TX Broadcast)");
    tracing::info!("  ├── TSC: {} MHz (hardware cycle-accurate timing)", tsc::tsc_khz() / 1000);
    tracing::info!("  └── Hydration: {}ms (one-time startup cost)", hydration_elapsed);
    tracing::info!("");

    // ─────────────────────────────────────────────────────────────────────────
    // GRACEFUL SHUTDOWN HANDLER
    // ─────────────────────────────────────────────────────────────────────────
    match signal::ctrl_c().await {
        Ok(()) => {
            tracing::info!("Shutdown signal received");
        }
        Err(err) => {
            tracing::error!("Failed to listen for shutdown signal: {}", err);
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // INITIATE GRACEFUL SHUTDOWN
    // ─────────────────────────────────────────────────────────────────────────
    tracing::info!("Shutting down threads...");
    
    // Signal shutdown
    shared_state.shutdown.store(true, std::sync::atomic::Ordering::Relaxed);

    // Wait for threads
    drop(telemetry_handle);
    drop(ws_handle);
    drop(sim_handle);
    drop(sender_handle);
    if let Some(handle) = broadcast_handle {
        drop(handle);
    }

    tracing::info!("Shutdown complete");
    
    // Print final stats
    let stats = &shared_state.stats;
    tracing::info!("Final stats:");
    tracing::info!("  Signals received: {}", stats.signals());
    tracing::info!("  Opportunities found: {}", stats.opportunities());
    tracing::info!("  Txs sent: {}", stats.txs_sent());
    tracing::info!("  Avg latency: {}µs", stats.avg_latency());
    
    // Print TSC telemetry stats
    tracing::info!("TSC Telemetry:");
    tracing::info!("  Total samples: {}", telemetry_stats.total());
    tracing::info!("  Min latency: {}ns", telemetry_stats.min_ns());
    tracing::info!("  Max latency: {}ns", telemetry_stats.max_ns());
    tracing::info!("  Avg latency: {}ns", telemetry_stats.avg_ns());

    Ok(())
}

#[cfg(test)]
mod tests {
    use lead_lag_arbitrage::revmsim::RevmSimulator;

    #[tokio::test]
    async fn test_binary_can_instantiate_revm_simulator() {
        let simulator = RevmSimulator::new("");
        assert!(!simulator.is_state_loaded());
    }

    #[tokio::test]
    #[ignore]
    async fn test_binary_revm_fork_execution() {
        let rpc_url = std::env::var("BLOXROUTE_RPC")
            .expect("BLOXROUTE_RPC must be set for fork test");

        let simulator = RevmSimulator::new(&rpc_url);
        let snapshot = simulator.hydrate_from_rpc(None).await
            .expect("Failed to hydrate from Arbitrum RPC");

        assert!(simulator.is_state_loaded(), "State should be loaded after hydration");
        assert!(snapshot.block.block_number > 0, "Block number should be positive");
        assert!(snapshot.accounts.len() >= 3, "Should have loaded Balancer + Uniswap + USDC accounts");
    }

    #[test]
    fn test_binary_compiles() {
        
    }
}
