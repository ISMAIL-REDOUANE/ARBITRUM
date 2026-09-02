#!/bin/bash
# =============================================================================
# MEV Arbitrage Engine - Arbitrum Sepolia Testnet Launch Script
# =============================================================================
# Usage: ./run_testnet.sh
# 
# Features:
#   - CPU core pinning (taskset -c 2,3)
#   - Shadow mode (no real transactions)
#   - Release build with maximum optimizations
#   - Concurrent telemetry WebSocket server
# =============================================================================

set -euo pipefail

# ─────────────────────────────────────────────────────────────────────────────
# CONFIGURATION
# ─────────────────────────────────────────────────────────────────────────────

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# Paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$SCRIPT_DIR/arbitrage-engine"
CARGO_RELEASE_FLAGS="--release --manifest-path=$PROJECT_DIR/Cargo.toml"

# Default values (can be overridden by .env.testnet)
WS_CORE="${WS_CORE:-2}"
SIM_CORE="${SIM_CORE:-3}"
SHADOW_MODE="${SHADOW_MODE:-true}"
RUST_LOG="${RUST_LOG:-info}"
TELEMETRY_PORT="${TELEMETRY_PORT:-3000}"

# ─────────────────────────────────────────────────────────────────────────────
# HELPER FUNCTIONS
# ─────────────────────────────────────────────────────────────────────────────

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

log_telemetry() {
    echo -e "${CYAN}[WS]${NC} $1"
}

check_environment() {
    log_info "Checking environment..."
    
    # Check if cargo is available
    if ! command -v cargo &> /dev/null; then
        log_error "cargo not found. Install Rust: https://rustup.rs"
        exit 1
    fi
    
    # Check if project exists
    if [[ ! -d "$PROJECT_DIR" ]]; then
        log_error "Project directory not found: $PROJECT_DIR"
        exit 1
    fi
    
    # Load .env.testnet if exists
    if [[ -f "$SCRIPT_DIR/.env.testnet" ]]; then
        log_info "Loading .env.testnet configuration..."
        source "$SCRIPT_DIR/.env.testnet"
    elif [[ -f "$SCRIPT_DIR/.env" ]]; then
        log_info "Loading .env configuration..."
        source "$SCRIPT_DIR/.env"
    fi
    
    log_info "Environment check complete"
}

build_release() {
    log_info "Building release binary..."
    
    # Clean and build with maximum optimizations
    cargo clean $CARGO_RELEASE_FLAGS 2>/dev/null || true
    cargo build $CARGO_RELEASE_FLAGS 2>&1 | tail -20
    
    if [[ ! -f "$PROJECT_DIR/target/release/arbitrage-engine" ]]; then
        log_error "Build failed - binary not found"
        exit 1
    fi
    
    log_info "Build complete: $PROJECT_DIR/target/release/arbitrage-engine"
}

start_telemetry_server() {
    log_telemetry "Starting telemetry WebSocket server on port $TELEMETRY_PORT..."
    
    # Simple telemetry WebSocket server (Python for zero dependencies)
    python3 -c "
import asyncio
import websockets
import json
from datetime import datetime

connected = set()

async def handler(websocket, path):
    connected.add(websocket)
    try:
        async for message in websocket:
            data = json.loads(message)
            # Broadcast to all connected dashboards
            for client in connected:
                if client != websocket:
                    await client.send(json.dumps(data))
    except websockets.exceptions.ConnectionClosed:
        pass
    finally:
        connected.remove(websocket)

async def main():
    async with websockets.serve(handler, 'localhost', $TELEMETRY_PORT) as server:
        print('Telemetry WebSocket server started on port $TELEMETRY_PORT')
        await asyncio.Future()  # Run forever

asyncio.run(main())
" &
    
    TELEMETRY_PID=$!
    log_telemetry "Telemetry server started (PID: $TELEMETRY_PID)"
    
    # Wait for server to start
    sleep 1
}

start_engine() {
    log_info "Starting MEV Arbitrage Engine..."
    log_info ""
    log_info "Architecture:"
    log_info "  ├── Core $WS_CORE: Event Loop (WebSocket listener)"
    log_info "  ├── Core $SIM_CORE: Simulation Engine (REVM hot path)"
    log_info "  ├── Core 0: Telemetry Logger"
    log_info "  └── Shadow Mode: ${SHADOW_MODE}"
    log_info ""
    
    # Build taskset command for CPU pinning
    TASKSET_CMD="taskset -c $WS_CORE,$SIM_CORE"
    
    # Set environment variables
    export SHADOW_MODE
    export RUST_LOG
    export ARBITRUM_RPC_URL
    export ARBITRUM_WS_URL
    export TELEMETRY_PORT
    
    # Build the command
    CMD="$PROJECT_DIR/target/release/arbitrage-engine"
    
    # Run with CPU pinning if taskset is available
    if command -v taskset &> /dev/null; then
        log_info "CPU Pinning enabled (cores: $WS_CORE, $SIM_CORE)"
        $TASKSET_CMD $CMD &
    else
        log_warn "taskset not available - running without CPU pinning"
        $CMD &
    fi
    
    ENGINE_PID=$!
    log_info "Engine started (PID: $ENGINE_PID)"
}

wait_forShutdown() {
    log_info "Engine running. Press Ctrl+C to shutdown."
    log_info ""
    
    # Wait for engine process
    while kill -0 $ENGINE_PID 2>/dev/null; do
        sleep 1
    done
    
    log_info "Engine process ended"
}

cleanup() {
    log_info "Cleaning up..."
    
    # Kill telemetry server
    if [[ -n "${TELEMETRY_PID:-}" ]]; then
        kill $TELEMETRY_PID 2>/dev/null || true
        log_info "Telemetry server stopped"
    fi
    
    # Kill engine if still running
    if [[ -n "${ENGINE_PID:-}" ]]; then
        kill $ENGINE_PID 2>/dev/null || true
        log_info "Engine stopped"
    fi
    
    log_info "Cleanup complete"
}

# ─────────────────────────────────────────────────────────────────────────────
# SIGNAL HANDLERS
# ─────────────────────────────────────────────────────────────────────────────

trap cleanup EXIT INT TERM

# ─────────────────────────────────────────────────────────────────────────────
# MAIN
# ─────────────────────────────────────────────────────────────────────────────

main() {
    echo ""
    echo "================================================================================"
    echo "        MEV ARBITRAGE ENGINE - ARBITRUM SEPOLIA TESTNET"
    echo "================================================================================"
    echo ""
    
    check_environment
    build_release
    start_telemetry_server
    start_engine
    wait_forShutdown
}

main "$@"
