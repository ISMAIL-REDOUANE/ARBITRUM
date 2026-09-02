#!/bin/bash
# =============================================================================
# Local Anvil Testnet Launch Script
# =============================================================================
# Starts the MEV Arbitrage Engine against local Anvil chain
# CPU pinning on cores 2 and 3
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
ENGINE_DIR="$PROJECT_ROOT/arbitrage-engine"

# Load environment
if [[ -f "$PROJECT_ROOT/.env.local" ]]; then
    set -a
    source "$PROJECT_ROOT/.env.local"
    set +a
fi

# Defaults
: "${CHAIN_ID:=421614}"
: "${ARBITRUM_RPC_URL:=http://127.0.0.1:8545}"
: "${WS_CORE:=2}"
: "${SIM_CORE:=3}"
: "${SHADOW_MODE:=true}"
: "${RUST_LOG:=info}"
: "${TELEMETRY_PORT:=3000}"

echo ""
echo "================================================================================"
echo "         MEV ARBITRAGE ENGINE - LOCAL ANVIL TESTNET"
echo "================================================================================"
echo ""
echo "Configuration:"
echo "  CHAIN_ID:       $CHAIN_ID"
echo "  RPC_URL:        $ARBITRUM_RPC_URL"
echo "  WS_CORE:        $WS_CORE"
echo "  SIM_CORE:       $SIM_CORE"
echo "  SHADOW_MODE:    $SHADOW_MODE"
echo "  TELEMETRY_PORT: $TELEMETRY_PORT"
echo ""

# Calculate CPU affinity mask (cores 2 and 3)
AFFINITY_MASK=$(( (1 << WS_CORE) + (1 << SIM_CORE) ))
echo "[INFO] CPU Affinity: 0x$(printf '%x' $AFFINITY_MASK) (cores $WS_CORE, $SIM_CORE)"

# Build engine if needed
if [[ ! -f "$ENGINE_DIR/target/release/arbitrage-engine.exe" ]]; then
    if [[ ! -f "$ENGINE_DIR/target/release/arbitrage-engine" ]]; then
        echo "[INFO] Building release binary..."
        cd "$ENGINE_DIR"
        cargo build --release 2>&1 | tail -5
    fi
fi

ENGINE_BIN="$ENGINE_DIR/target/release/arbitrage-engine"
if [[ ! -f "$ENGINE_BIN" ]] && [[ ! -f "${ENGINE_BIN}.exe" ]]; then
    echo "[ERROR] Engine binary not found: $ENGINE_BIN"
    exit 1
fi

echo "[OK] Engine binary found"
echo ""

# Set environment for engine
export ARBITRUM_RPC_URL
export CHAIN_ID
export SHADOW_MODE
export RUST_LOG
export TELEMETRY_PORT
export CONTRACT_ADDRESS

echo "[INFO] Starting engine with CPU affinity..."
echo ""

# Start engine with CPU affinity (Windows: use start /affinity)
if [[ "$(uname)" == *"MINGW"* ]] || [[ "$(uname)" == *"MSYS"* ]] || [[ "$OSTYPE" == "msys" ]]; then
    # Windows
    start /affinity $(printf '%x' $AFFINITY_MASK) "$ENGINE_BIN"
else
    # Unix/Linux
    taskset -c $WS_CORE,$SIM_CORE "$ENGINE_BIN" &
fi

echo "[OK] Engine started"
echo ""
echo "================================================================================"
echo "                     ENGINE RUNNING IN SHADOW MODE"
echo "================================================================================"
echo ""
echo "Dashboard: http://localhost:$TELEMETRY_PORT"
echo "WebSocket: ws://localhost:$TELEMETRY_PORT/ws"
echo ""
