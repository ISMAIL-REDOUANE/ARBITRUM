# Lead-Lag Arbitrage Engine

Sub-millisecond CEX-DEX Lead-Lag Arbitrage Engine for Ethereum L2s (Base, Arbitrum).

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│  BINANCE WS ──▶ RUST ENGINE ──▶ REVM SIM ──▶ SOLIDITY CONTRACT ──▶ ON-CHAIN DEX  │
│   (Lead)         (Multi-thread)    (In-Memory)   (Balancer Flash Loan)           │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

## Components

- **SmartContract.sol**: Balancer V2 Flash Loan + DEX routing with Yul optimizations
- **Rust Engine**: Tokio-based multi-threaded engine with SPSC ring buffers
- **AnvilLocalTest.sol**: Foundry test suite for local fork testing

## Quick Start

### Prerequisites

- Rust 1.75+
- Foundry
- Ethereum RPC URL (for fork testing)

### Build Solidity

```bash
forge build
```

### Run Tests

```bash
# Local tests
forge test

# Fork tests (requires RPC URL)
forge test --fork-url $RPC_URL -vvv
```

### Build Rust

```bash
cargo build --release
```

### Run Engine

```bash
# Set environment variables
export RPC_URL="your_rpc_url"
export PRIVATE_KEY="your_private_key"

# Run
cargo run --release
```

## Configuration

Copy `config.toml.example` to `config.toml` and configure:

- RPC endpoints
- Contract addresses
- Risk parameters

## Key Features

- Zero-capital via Balancer V2 flash loans (0% fee)
- Zero-copy JSON parsing with simd-json
- Lock-free SPSC ring buffers
- REVM in-memory simulation
- Yul-optimized smart contract
- Sub-millisecond latency target

## License

MIT
