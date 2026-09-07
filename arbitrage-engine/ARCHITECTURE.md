# CEX-DEX Lead-Lag Arbitrage Engine Architecture

## System Overview

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                           ARBITRAGE ENGINE - HIGH-LEVEL DATA FLOW                   │
├─────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                      │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐     ┌──────────────┐   │
│  │   BINANCE    │     │   RUST       │     │    REVM      │     │  SOLIDITY    │   │
│  │  WebSocket   │────▶│   ENGINE     │────▶│  SIMULATOR   │────▶│  CONTRACT    │   │
│  │  (aggTrade)  │     │  (Lead Signal)│     │ (In-Memory) │     │  (Executor)  │   │
│  └──────────────┘     └──────────────┘     └──────────────┘     └──────────────┘   │
│         │                    │                    │                    │             │
│         │                    │                    │                    │             │
│         ▼                    ▼                    ▼                    ▼             │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐     ┌──────────────┐   │
│  │ Price Ticker │     │ SPSC Ring   │     │  CacheDB     │     │   Balancer   │   │
│  │   Stream     │     │   Buffer    │     │  (State)     │     │  Vault V2    │   │
│  │  (Lead)      │     │  (0-Copy)   │     │  (Synced)    │     │ (Flash Loan) │   │
│  └──────────────┘     └──────────────┘     └──────────────┘     └──────────────┘   │
│                                                        │                    │             │
│                                                        │                    │             │
│                                                        ▼                    ▼             │
│                                                 ┌──────────────┐     ┌──────────────┐   │
│                                                 │  DEX Pools   │     │  Uniswap V2  │   │
│                                                 │  Pre-Synced  │◀───▶│    Router    │   │
│                                                 │    (RAM)     │     │              │   │
│                                                 └──────────────┘     └──────────────┘   │
│                                                        │                    │             │
│                                                        │                    ▼             │
│                                                 ┌──────────────┐     ┌──────────────┐   │
│                                                 │  Uniswap V3  │     │  Aerodrome   │   │
│                                                 │    Pools     │     │   (Base)     │   │
│                                                 └──────────────┘     └──────────────┘   │
│                                                                                      │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

## Module 1: Lead Signal Acquisition (Binance WebSocket)

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                        BINANCE WEBSOCKET STREAM HANDLING                             │
├─────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                      │
│  ┌───────────────────────────────────────────────────────────────────────────────┐   │
│  │                    WebSocket Connection Manager                               │   │
│  │  ┌─────────────────────────────────────────────────────────────────────────┐   │   │
│  │  │  Endpoint: wss://stream.binance.com:9443/ws/aggTrade                   │   │   │
│  │  │  Stream:   <symbol>@aggTrade                                            │   │   │
│  │  │  Format:   JSON (simd-json zero-copy parsing)                           │   │   │
│  │  └─────────────────────────────────────────────────────────────────────────┘   │   │
│  │                                    │                                          │   │
│  │                                    ▼                                          │   │
│  │  ┌─────────────────────────────────────────────────────────────────────────┐   │   │
│  │  │                    Price Update Handler                                │   │   │
│  │  │                                                                         │   │   │
│  │  │  struct AggTrade {                                                     │   │   │
│  │  │    e: "aggTrade",           // Event type                              │   │   │
│  │  │    s: "ETHUSDT",            // Symbol                                  │   │   │
│  │  │    p: "3456.78",            // Price (lead indicator)                  │   │   │
│  │  │    q: "1.234",              // Quantity                                │   │   │
│  │  │    T: 1699999999999,        // Trade timestamp                         │   │   │
│  │  │    m: false                  // Is buyer maker?                        │   │   │
│  │  │  }                                                                         │   │   │
│  │  └─────────────────────────────────────────────────────────────────────────┘   │   │
│  │                                    │                                          │   │
│  │                                    ▼                                          │   │
│  │  ┌─────────────────────────────────────────────────────────────────────────┐   │   │
│  │  │                    SPSC Ring Buffer (Multi-Producer)                    │   │   │
│  │  │                                                                         │   │   │
│  │  │   ┌─────────┬─────────┬─────────┬─────────┬─────────┬─────────┐          │   │   │
│  │  │   │ Price 1 │ Price 2 │ Price 3 │ Price 4 │   ...  │ Price N │          │   │   │
│  │  │   └─────────┴─────────┴─────────┴─────────┴─────────┴─────────┘          │   │   │
│  │  │        ▲                                                            │   │   │
│  │  │        │                                                            │   │   │
│  │  │   Writer (Binance Handler)                              Reader (Engine)│   │   │
│  │  │                                                                         │   │   │
│  │  │  - Lock-free, wait-free SPSC implementation                            │   │   │
│  │  │  - Zero-copy reads via ring buffer slots                               │   │   │
│  │  │  - Backpressure handling via bounded capacity                          │   │   │
│  │  └─────────────────────────────────────────────────────────────────────────┘   │   │
│  │                                                                                  │
│  └─────────────────────────────────────────────────────────────────────────────────┘   │
│                                                                                      │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

## Module 2: REVM In-Memory Simulation Engine

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                         REVM LOCAL EVM SIMULATION                                   │
├─────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                      │
│  ┌───────────────────────────────────────────────────────────────────────────────┐   │
│  │                    CacheDB (In-Memory State)                                  │   │
│  │  ┌─────────────────────────────────────────────────────────────────────────┐   │   │
│  │  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐     │   │   │
│  │  │  │   Account   │  │   Account   │  │   Account   │  │   Account   │     │   │   │
│  │  │  │   State     │  │   State     │  │   State     │  │   State     │     │   │   │
│  │  │  │  (Nonce,    │  │  (Nonce,    │  │  (Nonce,    │  │  (Nonce,    │     │   │   │
│  │  │  │   Balance,  │  │   Balance,  │  │   Balance,  │  │   Balance,  │     │   │   │
│  │  │  │   Code,     │  │   Code,     │  │   Code,     │  │   Code,     │     │   │   │
│  │  │  │   Storage)  │  │   Storage)  │  │   Storage)  │  │   Storage)  │     │   │   │
│  │  │  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘     │   │   │
│  │  │         │                │                │                │              │   │   │
│  │  │         └────────────────┴────────────────┴────────────────┘              │   │   │
│  │  │                          │                                               │   │   │
│  │  │                          ▼                                               │   │   │
│  │  │  ┌─────────────────────────────────────────────────────────────────────┐ │   │   │
│  │  │  │                    State Updates via WebSocket Events               │ │   │   │
│  │  │  │                                                                     │ │   │   │
│  │  │  │  - NewHeads subscription (block updates)                          │ │   │   │
│  │  │  │  - Logs subscription ( DEX pool syncs, transfers)                  │ │   │   │
│  │  │  │  - PendingTransactions (mempool monitoring)                        │ │   │   │
│  │  │  └─────────────────────────────────────────────────────────────────────┘ │   │   │
│  │  └─────────────────────────────────────────────────────────────────────────┘   │   │
│  │                                    │                                          │   │
│  │                                    ▼                                          │   │
│  │  ┌─────────────────────────────────────────────────────────────────────────┐   │   │
│  │  │                    REVM Executor                                        │   │   │
│  │  │                                                                         │   │   │
│  │  │   ┌─────────────────────────────────────────────────────────────────┐   │   │   │
│  │  │   │  Input: Balancer Flash Loan Transaction                         │   │   │   │
│  │  │   │  - CallData: flashLoan(address[], uint256[], bytes)             │   │   │   │
│  │  │   │  - Value: 0                                                    │   │   │   │
│  │  │   └─────────────────────────────────────────────────────────────────┘   │   │   │
│  │  │                                    │                                  │   │   │
│  │  │                                    ▼                                  │   │   │
│  │  │   ┌─────────────────────────────────────────────────────────────────┐   │   │   │
│  │  │   │  Execution Steps:                                               │   │   │   │
│  │  │   │  1. CALL to Balancer Vault (flash loan principal)               │   │   │   │
│  │  │   │  2. CALL to Arbitrage Contract (receiveFlashLoan callback)       │   │   │   │
│  │  │   │  3. Internal SWAP calls to DEX pools                           │   │   │   │
│  │  │   │  4. CALL to repay Flash Loan                                   │   │   │   │
│  │  │   │  5. CALL to Coinbase (builder bribe transfer)                  │   │   │   │
│  │  │   │  6. Self-RETURN with profit / REVERT on loss                   │   │   │   │
│  │  │   └─────────────────────────────────────────────────────────────────┘   │   │   │
│  │  │                                    │                                  │   │   │
│  │  │                                    ▼                                  │   │   │
│  │  │   ┌─────────────────────────────────────────────────────────────────┐   │   │   │
│  │  │   │  Output: (Revert Reason | Profit Amount | Gas Used)            │   │   │   │
│  │  │   └─────────────────────────────────────────────────────────────────┘   │   │   │
│  │  └─────────────────────────────────────────────────────────────────────────┘   │   │
│  │                                                                                  │
│  └─────────────────────────────────────────────────────────────────────────────────┘   │
│                                                                                      │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

## Module 3: Smart Contract Execution Flow

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                         SMART CONTRACT EXECUTION FLOW                                │
├─────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                      │
│  ┌───────────────────────────────────────────────────────────────────────────────┐   │
│  │                         ENTRY POINT: executeArbitrage                          │   │
│  │                                                                                │   │
│  │   function executeArbitrage(                                                  │   │
│  │       address[] calldata path,     // DEX path for swap                       │   │
│  │       uint256 amountIn,            // Flash loan amount                       │   │
│  │       uint256 minProfit            // Minimum profit threshold                 │   │
│  │   ) external nonReentrant {                                                    │   │
│  │       // ...                                                                  │   │
│  │   }                                                                            │   │
│  └───────────────────────────────────────────────────────────────────────────────┘   │
│                                    │                                               │
│                                    ▼                                               │
│  ┌───────────────────────────────────────────────────────────────────────────────┐   │
│  │                    STEP 1: BALANCER VAULT FLASH LOAN                          │   │
│  │                                                                                │   │
│  │   IBalancerVault(balancerVault).flashLoan(                                    │   │
│  │       address(this),           // receiverAddress                             │   │
│  │       tokens,                  // address[] - tokens to borrow                │   │
│  │       amounts,                 // uint256[] - amounts to borrow               │   │
│  │       0,                      // uint256 - fee percentage (0% for Balancer)  │   │
│  │       abi.encode(params)       // bytes - user defined params                 │   │
│  │   );                                                                           │   │
│  │                                                                                │   │
│  │   └─────────────▶ VAULT CALLS receiveFlashLoan(...) ON THIS CONTRACT         │   │
│  │                                                                                │   │
│  └───────────────────────────────────────────────────────────────────────────────┘   │
│                                    │                                               │
│                                    ▼                                               │
│  ┌───────────────────────────────────────────────────────────────────────────────┐   │
│  │                    STEP 2: RECEIVE FLASH LOAN + SWAP                          │   │
│  │                                                                                │   │
│  │   function receiveFlashLoan(                                                  │   │
│  │       IERC20[] memory tokens,                                                 │   │
│  │       uint256[] memory amounts,                                                │   │
│  │       uint256[] memory fee amounts,                                            │   │
│  │       bytes memory userData                                                   │   │
│  │   ) external override nonReentrant {                                          │   │
│  │                                                                                │   │
│  │       // ═══════════════════════════════════════════════════════════════════   │   │
│  │       // YUL ASM: Optimal DEX Selection & Routing                             │   │
│  │       // ═══════════════════════════════════════════════════════════════════   │   │
│  │                                                                                │   │
│  │       assembly {                                                              │   │
│  │           // Stack layout for gas optimization:                               │   │
│  │           // [0] = amountIn                                                   │   │
│  │           // [1] = amountOutMin                                               │   │
│  │           // [2] = dexType (0=UniswapV2, 1=UniswapV3, 2=Aerodrome)           │   │
│  │           // [3] = path.length                                                │   │
│  │                                                                                │   │
│  │           // ──── UNISWAP V2 ROUTING ────                                    │   │
│  │           // For each hop in path:                                           │   │
│  │           // 1. Approve router for token spending                           │   │
│  │           // 2. Call swapExactTokensForTokens                                │   │
│  │           // 3. Validate output amount >= minimum                            │   │
│  │                                                                                │   │
│  │           // ──── UNISWAP V3 SWAP ────                                       │   │
│  │           // 1. For exactInput: call uniswapV3SwapCallback                   │   │
│  │           // 2. Parse encoded path (has fees encoded)                       │   │
│  │           // 3. Execute swap with tick arrays                               │   │
│  │                                                                                │   │
│  │           // ──── AERODROME (BASE) ────                                      │   │
│  │           // 1. Direct swap on Base chain                                    │   │
│  │           // 2. StableSwap math for stable pairs                             │   │
│  │       }                                                                      │   │
│  │   }                                                                            │   │
│  └───────────────────────────────────────────────────────────────────────────────┘   │
│                                    │                                               │
│                                    ▼                                               │
│  ┌───────────────────────────────────────────────────────────────────────────────┐   │
│  │                    STEP 3: PROFIT VALIDATION (HARD GATE)                      │   │
│  │                                                                                │   │
│  │   assembly {                                                                  │   │
│  │       // Calculate net profit:                                                │   │
│  │       // Net_Profit = Amount_Out - Amount_In - Fees - Gas                     │   │
│  │                                                                                │   │
│  │       let profit := sub(amountOut, add(amountIn, totalFees))                  │   │
│  │                                                                                │   │
│  │       // HARD GATE: Revert if profit <= 0                                    │   │
│  │       if iszero(gt(profit, 0)) {                                             │   │
│  │           mstore(0x00, 0x08c379a0)  // Error(string) selector                 │   │
│  │           mstore(0x04, 0x14)       // String length: 20                       │   │
│  │           mstore(0x24, 0x494e53554646494349454e5450524f465459525f50524f464954) // "INSUFFICIENT_PROFIT" │   │
│  │           revert(0x00, 0x44)       // Revert with error                        │   │
│  │       }                                                                       │   │
│  │                                                                                │   │
│  │       // COINBASE BRIBE for builder (if block.coinbase != 0x000...0)         │   │
│  │       // call {gas: gasleft(), value: bribeAmount}(block.coinbase)           │   │
│  │   }                                                                            │   │
│  └───────────────────────────────────────────────────────────────────────────────┘   │
│                                    │                                               │
│                                    ▼                                               │
│  ┌───────────────────────────────────────────────────────────────────────────────┐   │
│  │                    STEP 4: REPAY FLASH LOAN + PROFIT EXTRACTION              │   │
│  │                                                                                │   │
│  │   assembly {                                                                  │   │
│  │       // Repay to Balancer Vault:                                            │   │
│  │       // IERC20(token).transfer(vault, amount + fee)                         │   │
│  │                                                                                │   │
│  │       // Extract profit to caller:                                           │   │
│  │       // msg.sender.transfer(profit)                                         │   │
│  │   }                                                                            │   │
│  └───────────────────────────────────────────────────────────────────────────────┘   │
│                                                                                      │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

## Module 4: Threaded Architecture

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                           RUST MULTI-THREADED ARCHITECTURE                          │
├─────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                      │
│  ┌───────────────────────────────────────────────────────────────────────────────┐   │
│  │                         MAIN THREAD (Orchestrator)                            │   │
│  │  ┌─────────────────────────────────────────────────────────────────────────┐   │   │
│  │  │  - Spawns child threads                                                 │   │   │
│  │  │  - Manages shutdown signals (Ctrl+C, SIGTERM)                           │   │   │
│  │  │  - Monitors thread health via heartbeats                                │   │   │
│  │  │  - Coordinates graceful shutdown                                        │   │   │
│  │  └─────────────────────────────────────────────────────────────────────────┘   │   │
│  └───────────────────────────────────────────────────────────────────────────────┘   │
│                                    │                                               │
│        ┌──────────────────────────┼──────────────────────────┐                    │
│        │                          │                          │                     │
│        ▼                          ▼                          ▼                     │
│  ┌───────────────┐     ┌───────────────────────┐     ┌───────────────┐            │
│  │  THREAD 1:    │     │     THREAD 2:        │     │   THREAD 3:   │            │
│  │  WebSocket    │     │   Revm Simulation    │     │   TX Sender   │            │
│  │  Listener     │     │   Engine             │     │   + Monitor   │            │
│  │               │     │                      │     │               │            │
│  │  - Connect    │     │  - SPSC Consumer     │     │  - SPSC       │            │
│  │  - Subscribe  │────▶│  - Parse trigger     │────▶│  - Sign TX    │────▶ ON-CHAIN
│  │  - Parse JSON │     │  - Execute in REVM   │     │  - Broadcast  │            │
│  │  - Push to    │     │  - Validate profit   │     │  - Monitor    │            │
│  │    RingBuf    │     │  - If profitable:    │     │  - Retry on   │            │
│  │               │     │    push to TX queue  │     │    failure    │            │
│  │               │     │                      │     │               │            │
│  │               │     │  else: DROP (0-gas) │     │               │            │
│  └───────────────┘     └───────────────────────┘     └───────────────┘            │
│        │                          │                          │                     │
│        │                          │                          │                     │
│        │    ┌─────────────────────┘                          │                     │
│        │    │                                                │                     │
│        ▼    ▼                                                ▼                     │
│  ┌───────────────────────────────────────────────────────────────────────────┐     │
│  │                    SPSC RING BUFFER (Lock-Free)                          │     │
│  │                                                                           │     │
│  │   ┌─────────────────────────────────────────────────────────────────┐     │     │
│  │   │                     1024 SLOT CAPACITY                          │     │     │
│  │   │                                                                  │     │     │
│  │   │   PRODUCER (Thread 1)                         CONSUMER (Thread 2)│     │     │
│  │   │        │                                          │            │     │     │
│  │   │        ▼                                          ▼            │     │     │
│  │   │   ┌────┬────┬────┬────┬────┬────┬────┬────┐   ┌────┬────┐      │     │     │
│  │   │   │ S0 │ S1 │ S2 │ S3 │ S4 │ S5 │ S6 │ S7 │...│S1022│S1023│     │     │     │
│  │   │   └────┴────┴────┴────┴────┴────┴────┴────┘   └────┴────┘      │     │     │
│  │   │                                                                  │     │     │
│  │   │   Head ──────────────────────────────────────────────────◀ Tail │     │     │
│  │   │   (write)                               (read)                   │     │     │
│  │   │                                                                  │     │     │
│  │   │   Zero-copy: slots contain direct PriceEvent references         │     │     │
│  │   │   Wait-free SPSC: no locks, no atomics for cache-line writes   │     │     │
│  │   └─────────────────────────────────────────────────────────────────┘     │     │
│  │                                                                           │     │
│  └───────────────────────────────────────────────────────────────────────────┘     │
│                                                                                      │
│  ┌───────────────────────────────────────────────────────────────────────────────┐   │
│  │                    SHARED STATE (Arc<Mutex<>> for reads only)               │   │
│  │  ┌─────────────────────────────────────────────────────────────────────────┐   │   │
│  │  │  struct SharedState {                                                  │   │   │
│  │  │    pools: RwLock<HashMap<PoolKey, PoolData>>,  // Pre-synced DEX pools│   │   │
│  │  │    cache_db: Arc<CacheDB<InMemory>>,           // REVM state           │   │   │
│  │  │    config: Config,                              // Static config       │   │   │
│  │  │    stats: AtomicU64,                            // Performance stats   │   │   │
│  │  │  }                                                                     │   │   │
│  │  └─────────────────────────────────────────────────────────────────────────┘   │   │
│  └───────────────────────────────────────────────────────────────────────────────┘   │
│                                                                                      │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

## Financial Model: Zero-Capital Arbitrage

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                         ZERO-CAPITAL ARBITRAGE MATH                                 │
├─────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                      │
│  ╔═══════════════════════════════════════════════════════════════════════════════╗   │
│  ║                        FLASH LOAN CYCLE (NO CAPITAL)                          ║   │
│  ╠═══════════════════════════════════════════════════════════════════════════════╣   │
│  ║                                                                                ║   │
│  ║   Step 1: Borrow USDT from Balancer Vault                                    ║   │
│  ║           BORROWED = A                                                        ║   │
│  ║                                                                                ║   │
│  ║   Step 2: Buy ETH on CEX (Binance) at Price_PC                                ║   │
│  ║           ETH_IN = A / Price_PC                                               ║   │
│  ║                                                                                ║   │
│  ║   Step 3: Sell ETH on DEX (Uniswap) at Price_PD > Price_PC                    ║   │
│  ║           USDT_OUT = ETH_IN * Price_PD                                        ║   │
│  ║                                                                                ║   │
│  ║   Step 4: Repay Balancer (A + 0% fee)                                         ║   │
│  ║           REPAID = A * (1 + fee_flash) = A                                    ║   │
│  ║                                                                                ║   │
│  ╚═══════════════════════════════════════════════════════════════════════════════╝   │
│                                                                                      │
│  ╔═══════════════════════════════════════════════════════════════════════════════╗   │
│  ║                           PROFIT CALCULATION                                   ║   │
│  ╠═══════════════════════════════════════════════════════════════════════════════╣   │
│  ║                                                                                ║   │
│  ║   GROSS_PROFIT = USDT_OUT - BORROWED                                          ║   │
│  ║                 = A * (Price_PD / Price_PC - 1)                               ║   │
│  ║                                                                                ║   │
│  ║   TOTAL_FEES = FEE_DEX_A + FEE_DEX_B + FEE_L2_GAS                            ║   │
│  ║                                                                                ║   │
│  ║   NET_PROFIT = GROSS_PROFIT - TOTAL_FEES                                      ║   │
│  ║                                                                                ║   │
│  ║   EXECUTE IF: NET_PROFIT > MIN_PROFIT_THRESHOLD                               ║   │
│  ║                                                                                ║   │
│  ╚═══════════════════════════════════════════════════════════════════════════════╝   │
│                                                                                      │
│  ╔═══════════════════════════════════════════════════════════════════════════════╗   │
│  ║                         AERODROME (BASE) SPECIFIC                             ║   │
│  ╠═══════════════════════════════════════════════════════════════════════════════╣   │
│  ║                                                                                ║   │
│  ║   Aerodrome StableSwap Math:                                                  ║   │
│  ║   x³*y + y³*x = k (constant product with stables)                            ║   │
│  ║                                                                                ║   │
│  ║   Fee calculation:                                                            ║   │
│  ║   fee = (amountIn * fee_bps) / 10000                                          ║   │
│  ║   - Stable swap: 4 bps (0.04%)                                               ║   │
│  ║   - Volatile swap: 30 bps (0.30%)                                            ║   │
│  ║                                                                                ║   │
│  ╚═══════════════════════════════════════════════════════════════════════════════╝   │
│                                                                                      │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

## Supported DEX Pools & Chains

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                           SUPPORTED DEX POOLS                                       │
├─────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                      │
│  CHAIN: ETHEREUM MAINNET (for reference, L2s below)                                 │
│  ┌───────────────────────────────────────────────────────────────────────────────┐   │
│  │  Uniswap V2 Router: 0x7a250d5630B4cF539739dF2C5dAcb4c659F2488D               │   │
│  │  Uniswap V3 SwapRouter: 0xE592427A0AEce92De3Edee1F18E0157CA0580D           │   │
│  │  SushiSwap Router: 0xd9e1cE17f2641f24aE83637ab66a2cca9C378B9F                │   │
│  └───────────────────────────────────────────────────────────────────────────────┘   │
│                                                                                      │
│  CHAIN: BASE                                                                      │
│  ┌───────────────────────────────────────────────────────────────────────────────┐   │
│  │  Aerodrome Router: 0x0B404b975d461A45E3Aa6b96809746b40A76239F                │   │
│  │  Uniswap V3 Base: 0x2626664c2603336E57B271c5C0b26D421d473758                   │   │
│  │  Base USDC: 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913                        │   │
│  └───────────────────────────────────────────────────────────────────────────────┘   │
│                                                                                      │
│  CHAIN: ARBITRUM                                                                  │
│  ┌───────────────────────────────────────────────────────────────────────────────┐   │
│  │  Uniswap V3 Arbitrum: 0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45              │   │
│  │  SushiSwap Arbitrum: 0x1b02dA8Cb0d097cB8d57d3c6EFb1b86D8F2E2b3F            │   │
│  │  Camelot Router: 0x51eC82945A4482D01b01C0C2C45d314198D2DEE2                   │   │
│  └───────────────────────────────────────────────────────────────────────────────┘   │
│                                                                                      │
│  FLASH LOAN PROVIDER                                                               │
│  ┌───────────────────────────────────────────────────────────────────────────────┐   │
│  │  Balancer Vault V2 (ALL CHAINS): 0xBA12222222228d8Ba445958a75a0704d588BF65   │   │
│  │  - Fee: 0% (Balancer Protocol Fee)                                           │   │
│  │  - Flash Loan Type: No fee, standard ERC20                                   │   │
│  └───────────────────────────────────────────────────────────────────────────────┘   │
│                                                                                      │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

## Latency Budget (Sub-Millisecond Target)

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                           LATENCY BUDGET                                            │
├─────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                      │
│  Component                          │ Target    │ Maximum    │ Notes               │
│  ───────────────────────────────────┼───────────┼────────────┼─────────────────────│
│  Binance WebSocket → Engine         │ 50µs      │ 100µs      │ simd-json parsing   │
│  Ring Buffer Pop                    │ 5µs       │ 10µs       │ Lock-free SPSC      │
│  Price Validation                   │ 10µs      │ 20µs       │ Simple comparison   │
│  REVM Simulation                    │ 200µs     │ 500µs      │ Full EVM execution  │
│  Profit Calculation                 │ 5µs       │ 10µs       │ Assembly math      │
│  TX Construction                    │ 20µs      │ 50µs       │ ABI encoding       │
│  ───────────────────────────────────┼───────────┼────────────┼─────────────────────│
│  TOTAL                              │ ~290µs    │ ~690µs     │ UNDER 1ms TARGET   │
│                                                                                      │
│  Note: On-chain settlement is NOT included in latency budget (L1 confirmation)      │
│                                                                                      │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

## Error Handling & Risk Management

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                           ERROR STATES & HANDLING                                   │
├─────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                      │
│  ┌───────────────────────────────────────────────────────────────────────────────┐   │
│  │                           ERROR CLASSIFICATION                                  │   │
│  ├───────────────────────────────────────────────────────────────────────────────┤   │
│  │                                                                                │   │
│  │  CRITICAL (Drop Execution, Log Alert):                                        │   │
│  │  ├── REVM simulation reverts                                                  │   │
│  │  ├── Net profit <= 0 (Hard Gate trigger)                                     │   │
│  │  ├── Pool liquidity insufficient                                              │   │
│  │  └── Flash loan callback fails                                               │   │
│  │                                                                                │   │
│  │  WARNING (Log & Continue):                                                     │   │
│  │  ├── WebSocket reconnection                                                   │   │
│  │  ├── Block reorganization (REVM resync)                                      │   │
│  │  └── Gas price spike (skip block)                                            │   │
│  │                                                                                │   │
│  │  INFO (Debug Logging):                                                        │   │
│  │  ├── Price deviation detected but below threshold                            │   │
│  │  ├── Pool state updated                                                       │   │
│  │  └── Heartbeat monitoring                                                     │   │
│  │                                                                                │   │
│  └───────────────────────────────────────────────────────────────────────────────┘   │
│                                                                                      │
│  ┌───────────────────────────────────────────────────────────────────────────────┐   │
│  │                         ZERO-RISK GUARANTEES                                 │   │
│  ├───────────────────────────────────────────────────────────────────────────────┤   │
│  │                                                                                │   │
│  │  1. NO CAPITAL AT RISK: All execution uses flash loans (0% fee)             │   │
│  │  2. REVM GATE: Only executes on-chain if REVM simulation passes              │   │
│  │  3. ATOMIC REVERT: On-chain contract reverts if profit <= 0                 │   │
│  │  4. NON-REENTRANT: All contract functions use nonReentrant modifier         │   │
│  │  5. GAS CAP: Maximum gas limit per arbitrage tx (prevent runaway)           │   │
│  │                                                                                │   │
│  └───────────────────────────────────────────────────────────────────────────────┘   │
│                                                                                      │
└─────────────────────────────────────────────────────────────────────────────────────┘
```
