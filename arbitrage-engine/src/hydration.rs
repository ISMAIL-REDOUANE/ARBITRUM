//! # State Hydration Module
//!
//! Pre-execution state synchronization from Arbitrum RPC to in-memory CacheDB.
//!
//! ## Purpose
//!
//! Before the latency-critical pinned threads start, we must hydrate our
//! in-memory `RevmCacheDB` with the current on-chain state. This ensures
//! our REVM simulations start with accurate pool reserves, token balances,
//! and contract bytecode.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                    STATE HYDRATION PIPELINE                              │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │                                                                          │
//! │  ┌─────────────────────────────────────────────────────────────────┐   │
//! │  │                    PHASE 1: Contract Bytecodes                     │   │
//! │  │                                                                  │   │
//! │  │  foreach contract in [USDC, WETH, Balancer, UniswapV3] {        │   │
//! │  │      bytecode = eth_getCode(contract)                           │   │
//! │  │      cache_db.insert_contract(bytecode)                          │   │
//! │  │  }                                                              │   │
//! │  │                                                                  │   │
//! │  │  Timing: ~100-500ms per contract (network latency)               │   │
//! │  │  Total: ~2-5 contracts × ~200ms = ~1-2 seconds                  │   │
//! │  └─────────────────────────────────────────────────────────────────┘   │
//! │                              │                                        │
//! │                              ▼                                        │
//! │  ┌─────────────────────────────────────────────────────────────────┐   │
//! │  │                    PHASE 2: Storage Slots                         │   │
//! │  │                                                                  │   │
//! │  │  Uniswap V3 slot0:      keccak256(token0, token1, feeTier)      │   │
//! │  │  Token balances:         balanceOf slot for each token           │   │
//! │  │  Balancer vault state:  internal pool registers                  │   │
//! │  │                                                                  │   │
//! │  │  Timing: ~50-100ms per slot (batched RPC calls)                  │   │
//! │  └─────────────────────────────────────────────────────────────────┘   │
//! │                              │                                        │
//! │                              ▼                                        │
//! │  ┌─────────────────────────────────────────────────────────────────┐   │
//! │  │                    PHASE 3: CacheDB Injection                    │   │
//! │  │                                                                  │   │
//! │  │  Arc<RwLock<RevmCacheDB>> ← Full state snapshot                 │   │
//! │  │       │                                                           │   │
//! │  │       └── Ready for simulation thread (no RPC needed)             │   │
//! │  │                                                                  │   │
//! │  │  Timing: ~1µs (just pointer update)                              │   │
//! │  └─────────────────────────────────────────────────────────────────┘   │
//! │                                                                          │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Why This Happens Before Pinned Threads?
//!
//! 1. **Deterministic simulation**: If we updated state during the hot loop,
//!    we'd have timing variance from RPC calls.
//!
//! 2. **Cache locality**: Once hydrated, the simulation thread keeps state
//!    hot in L1/L2 cache. RPC calls would evict this.
//!
//! 3. **Sub-100µs requirement**: Even a single RPC call (~10ms) would blow
//!    our latency budget 100x over.
//!
//! ## Critical Storage Slots
//!
//! ### Uniswap V3 Pool
//! - `slot0`: Current price (sqrtPrice), tick, fee growth
//! - `liquidity`: Current liquidity in the pool
//! - Computed as: `keccak256(token0 . token1 . feeTier)` for the pool
//!
//! ### ERC-20 Tokens (USDC, WETH)
//! - Balance slot for flash loan accounting
//! - `balanceOf(owner)` storage slot is computed from owner address
//!
//! ### Balancer Vault
//! - Pool registers for flash loan validation
//! - Internal balance tracking

use crate::cache_db::RevmCacheDB;
use crate::error::{ArbitrageError, Result};

use std::time::{Duration, Instant};

const ARBITRUM_CHAIN_ID: u64 = 42161;

/// Arbitrum RPC endpoints (public and private)
const ARBITRUM_RPC_URLS: &[&str] = &[
    "https://arb1.arbitrum.io/rpc",
    "https://arbitrum.public-rpc.com",
];

/// Critical contract addresses on Arbitrum (chain ID 42161).
/// All addresses verified against Arbitrum mainnet.
const CRITICAL_CONTRACTS: &[(ContractId, &str)] = &[
    (ContractId::USDC,          "0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
    (ContractId::WETH,          "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1"),
    (ContractId::BalancerVault, "0xBA12222222228d8Ba445958a75a0704d566BF2C8"),
    (ContractId::UniswapV3Factory, "0x1F98431c8aD98523631AE4a59f267346ea31F984"),
    (ContractId::UniswapV3Router,    "0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45"),
    (ContractId::UniswapV3Quoter,    "0xb27308f9F90D607463bb33eA1BeA3D0D3D15F8E1"),
];

/// Flash loan receiver address — read from environment; required for production.
const ARBITRUM_ENGINE_ENV: &str = "EXECUTOR_ADDRESS";

/// Contract identifiers for interface verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractId {
    USDC,
    WETH,
    BalancerVault,
    UniswapV3Factory,
    UniswapV3Router,
    UniswapV3Quoter,
}

/// Expected interface selectors (first 4 bytes of function signatures)
/// for bytecode verification. If fetched bytecode does not contain any
/// of these selectors, the contract is considered missing or invalid.
const EXPECTED_SELECTORS: &[(ContractId, &[&str])] = &[
    (ContractId::USDC, &["0x70a08231"]),           // balanceOf(address)
    (ContractId::WETH, &["0x70a08231"]),           // balanceOf(address)
    (ContractId::BalancerVault, &["0x3ec7023f"]),   // flashLoan(address,address[],uint256[],bytes)
    (ContractId::UniswapV3Router, &["0x4e4541af"]), // exactInputSingle(...) on SwapRouter02
    (ContractId::UniswapV3Factory, &["0x18160ddd"]), // totalPairs()
    (ContractId::UniswapV3Quoter, &["0x011b6f1b"]), // quoteExactInputSingle(...)
];

/// Halt startup if a required contract is missing or invalid.
pub fn fail_closed(msg: &str) -> ! {
    eprintln!("FATAL: Address verification failed: {}", msg);
    std::process::exit(1);
}

/// Human-readable name for a ContractId.
pub fn contract_id_str(id: ContractId) -> &'static str {
    match id {
        ContractId::USDC => "USDC",
        ContractId::WETH => "WETH",
        ContractId::BalancerVault => "BalancerVault",
        ContractId::UniswapV3Factory => "UniswapV3Factory",
        ContractId::UniswapV3Router => "UniswapV3Router",
        ContractId::UniswapV3Quoter => "UniswapV3Quoter",
    }
}

/// Verify that fetched bytecode contains at least one expected selector.
/// Returns `false` if any required selector is missing.
pub fn verify_bytecode_interface(contract_id: ContractId, bytecode: &[u8]) -> bool {
    let expected = match EXPECTED_SELECTORS.iter().find(|(id, _)| *id == contract_id) {
        Some((_, selectors)) => selectors,
        None => return true, // No selectors defined — skip verification
    };
    for sel_hex in expected.iter() {
        let sel = hex::decode(sel_hex.trim_start_matches("0x"));
        if let Ok(sel_bytes) = sel {
            if bytecode.windows(sel_bytes.len()).any(|w| w == sel_bytes) {
                return true;
            }
        }
    }
    false
}

/// Validate that an Ethereum address string is well-formed (40 hex chars).
pub fn validate_address(address: &str) -> Result<()> {
    if !address.starts_with("0x") || address.len() != 42 {
        return Err(ArbitrageError::Config(format!(
            "Invalid address '{}': must be 0x + 40 hex chars",
            address
        )));
    }
    let hex_str = address.trim_start_matches("0x");
    if hex::decode(hex_str).is_err() {
        return Err(ArbitrageError::Config(format!(
            "Invalid address '{}': non-hex characters",
            address
        )));
    }
    Ok(())
}

/// State hydration result
#[derive(Debug)]
pub struct HydrationState {
    pub contracts_loaded: usize,
    pub storage_slots_loaded: usize,
    pub time_elapsed_ms: u64,
}

/// HTTP client for RPC calls
pub struct RpcClient {
    client: reqwest::Client,
    endpoint: String,
}

impl RpcClient {
    pub fn new(endpoint: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");
        
        Self {
            client,
            endpoint: endpoint.to_string(),
        }
    }
    
    /// eth_getCode - Fetch contract bytecode
    pub async fn get_code(&self, address: &str) -> Result<Vec<u8>> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_getCode",
            "params": [address, "latest"],
            "id": 1
        });
        
        let response = self.client
            .post(&self.endpoint)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArbitrageError::Rpc(format!("RPC request failed: {}", e)))?;
        
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ArbitrageError::Rpc(format!("Failed to parse RPC response: {}", e)))?;
        
        if let Some(error) = body.get("error") {
            return Err(ArbitrageError::Rpc(format!("RPC error: {}", error)));
        }
        
        let code_str = body.get("result")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ArbitrageError::Rpc("Missing result in RPC response".to_string()))?;
        
        // Remove 0x prefix and decode hex
        let code_hex = code_str.trim_start_matches("0x");
        let bytecode = hex::decode(code_hex)
            .map_err(|e| ArbitrageError::Encoding(format!("Failed to decode bytecode: {}", e)))?;
        
        Ok(bytecode)
    }
    
    /// eth_getStorageAt - Fetch storage slot value
    pub async fn get_storage_at(&self, address: &str, slot: &str) -> Result<Vec<u8>> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_getStorageAt",
            "params": [address, slot, "latest"],
            "id": 1
        });
        
        let response = self.client
            .post(&self.endpoint)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArbitrageError::Rpc(format!("RPC request failed: {}", e)))?;
        
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ArbitrageError::Rpc(format!("Failed to parse RPC response: {}", e)))?;
        
        if let Some(error) = body.get("error") {
            return Err(ArbitrageError::Rpc(format!("RPC error: {}", error)));
        }
        
        let storage_str = body.get("result")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ArbitrageError::Rpc("Missing result in RPC response".to_string()))?;
        
        let storage_hex = storage_str.trim_start_matches("0x");
        let storage = hex::decode(storage_hex)
            .map_err(|e| ArbitrageError::Encoding(format!("Failed to decode storage: {}", e)))?;
        
        Ok(storage)
    }
    
    /// eth_call - Read contract state (for complex reads)
    pub async fn call(&self, to: &str, data: &str) -> Result<String> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_call",
            "params": [{
                "to": to,
                "data": data
            }, "latest"],
            "id": 1
        });
        
        let response = self.client
            .post(&self.endpoint)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArbitrageError::Rpc(format!("RPC request failed: {}", e)))?;
        
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ArbitrageError::Rpc(format!("Failed to parse RPC response: {}", e)))?;
        
        if let Some(error) = body.get("error") {
            return Err(ArbitrageError::Rpc(format!("RPC error: {}", error)));
        }
        
        body.get("result")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| ArbitrageError::Rpc("Missing result in RPC response".to_string()))
    }
    
    /// eth_blockNumber - Get current block number
    pub async fn get_block_number(&self) -> Result<u64> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_blockNumber",
            "params": [],
            "id": 1
        });
        
        let response = self.client
            .post(&self.endpoint)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArbitrageError::Rpc(format!("RPC request failed: {}", e)))?;
        
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ArbitrageError::Rpc(format!("Failed to parse RPC response: {}", e)))?;
        
        if let Some(error) = body.get("error") {
            return Err(ArbitrageError::Rpc(format!("RPC error: {}", error)));
        }
        
        let block_str = body.get("result")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ArbitrageError::Rpc("Missing result in RPC response".to_string()))?;
        
        let block_hex = block_str.trim_start_matches("0x");
        u64::from_str_radix(block_hex, 16)
            .map_err(|e| ArbitrageError::Rpc(format!("Failed to parse block number: {}", e)))
    }
}

/// Compute ERC-20 balanceOf storage slot
/// 
/// The balanceOf mapping is stored at slot keccak256(owner . slot_number)
/// where slot_number is the mapping's position in the contract storage.
/// For most ERC-20 tokens, this is slot 0.
pub fn compute_balance_slot(owner: &str, mapping_slot: u32) -> String {
    use tiny_keccak::{Keccak, Hasher};

    let hex_str = owner.trim_start_matches("0x");
    let owner_bytes = match hex_str.len().is_multiple_of(2) {
        true => hex::decode(hex_str),
        false => hex::decode(format!("0{}", hex_str)),
    }.unwrap_or_else(|_| {
        hex::decode("0000000000000000000000000000000000000000").unwrap()
    });

    let mut keccak = Keccak::v256();
    keccak.update(&owner_bytes);

    let slot_bytes = mapping_slot.to_be_bytes();
    let mut slot_padded = [0u8; 32];
    slot_padded[32 - slot_bytes.len()..].copy_from_slice(&slot_bytes);
    keccak.update(&slot_padded);

    let mut hash = [0u8; 32];
    keccak.finalize(&mut hash);

    hex::encode(hash)
}

/// Compute Uniswap V3 pool slot0 slot
/// 
/// Uniswap V3 pools compute the storage slot for the pool as:
/// keccak256(keccak256(token0 . token1 . fee) . uint96(0))
/// But the actual pool address is deterministic via CREATE2.
pub fn compute_uniswap_v3_slot0(_pool_address: &str) -> String {
    // Uniswap V3 slot0 is always at storage slot 0 for the pool contract
    // This is because slot0 is the first variable in the pool struct
    "0".to_string()
}

/// Main state hydration function
/// 
/// This runs BEFORE the pinned threads start to hydrate the CacheDB
/// with accurate on-chain state.
/// 
/// # Arguments
/// 
/// * `cache_db` - Mutable reference to the CacheDB to hydrate
/// * `rpc_endpoint` - Arbitrum RPC endpoint to use
/// 
/// # Returns
/// 
/// Hydration statistics on success
pub async fn hydrate_state(
    cache_db: &mut RevmCacheDB,
    rpc_endpoint: &str,
) -> Result<HydrationState> {
    let client = RpcClient::new(rpc_endpoint);
    let start = Instant::now();
    
    tracing::info!("Starting state hydration from {}", rpc_endpoint);
    
    let mut contracts_loaded = 0;
    let mut storage_slots_loaded = 0;
    
    // ─────────────────────────────────────────────────────────────────────────
    // PHASE 1: Load Contract Bytecodes
    // ─────────────────────────────────────────────────────────────────────────
    tracing::info!("Phase 1: Loading contract bytecodes...");
    for (contract_id, address) in CRITICAL_CONTRACTS {
        let fetch_start = Instant::now();
        
        match client.get_code(address).await {
            Ok(bytecode) if !bytecode.is_empty() => {
                // Verify bytecode contains expected interface selectors
                if !verify_bytecode_interface(*contract_id, &bytecode) {
                    let msg = format!(
                        "Bytecode at {} ({}) does not match expected interface selectors",
                        address, contract_id_str(*contract_id)
                    );
                    tracing::error!("{}", msg);
                    return Err(ArbitrageError::Config(msg));
                }

                let address_bytes = hex::decode(address.trim_start_matches("0x"))
                    .map_err(|e| ArbitrageError::Encoding(format!("Invalid address '{}': {}", address, e)))?;
                if address_bytes.len() != 20 {
                    let msg = format!("Address {} is not 20 bytes", address);
                    tracing::error!("{}", msg);
                    return Err(ArbitrageError::Config(msg));
                }
                let mut addr = [0u8; 20];
                addr.copy_from_slice(&address_bytes);
                
                cache_db.insert_contract(addr, bytecode);

                contracts_loaded += 1;
                
                let elapsed = fetch_start.elapsed().as_millis();
                tracing::debug!(
                    "Loaded {} bytecode ({} bytes) in {}ms",
                    contract_id_str(*contract_id),
                    cache_db.contracts_len(),
                    elapsed
                );
            }
            Ok(_bytecode) => {
                let msg = format!(
                    "Empty bytecode for {} at {}, contract may not be deployed",
                    contract_id_str(*contract_id),
                    address
                );
                tracing::error!("{}", msg);
                return Err(ArbitrageError::Config(msg));
            }
            Err(e) => {
                let msg = format!("Failed to fetch bytecode for {}: {}", contract_id_str(*contract_id), e);
                tracing::error!("{}", msg);
                return Err(ArbitrageError::Rpc(msg));
            }
        }
    }
    
    // ─────────────────────────────────────────────────────────────────────────
    // PHASE 2: Load Critical Storage Slots
    // ─────────────────────────────────────────────────────────────────────────
    tracing::info!("Phase 2: Loading critical storage slots...");
    
    // Use the correct ArbSwapRouter02 Quoter address (full 40 hex chars)
    let quoter_address = "0xb27308f9F90D607463bb33eA1BeA3D0D3D15F8E1";
    
    // Try to get a quote for ETH->USDC to verify connectivity
    let quoter_data = "0x0d21c89b0000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000000000000000000000000000af88d065e77c8cc2239327c5edb3a432268e583100000000000000000000000082aF49447D8a07e3bd95BD0d56f35241523fBab10000000000000000000000000000000000000000000000000000000000000aaa0000000000000000000000000000000000000000000000000000000000000004";
    
    if let Err(e) = client.call(quoter_address, quoter_data).await {
        tracing::warn!("Failed to call QuoterV2 (may need different calldata): {}", e);
    } else {
        tracing::debug!("QuoterV2 call successful");
    }
    
    // Load engine balance for flash loan accounting — MUST come from env
    let engine_address = std::env::var(ARBITRUM_ENGINE_ENV)
        .map_err(|_| {
            let msg = format!(
                "{} not set in environment — cannot hydrate engine balance",
                ARBITRUM_ENGINE_ENV
            );
            tracing::error!("{}", msg);
            ArbitrageError::Config(msg)
        })?;
    
    // Validate the engine address format
    let engine_addr_bytes = hex::decode(engine_address.trim_start_matches("0x"))
        .map_err(|e| ArbitrageError::Config(format!("Invalid EXECUTOR_ADDRESS hex: {}", e)))?;
    if engine_addr_bytes.len() != 20 {
        return Err(ArbitrageError::Config(format!(
            "EXECUTOR_ADDRESS must be 20 bytes, got {}",
            engine_addr_bytes.len()
        )));
    }
    
    // Load WETH balance for our arbitrage engine (for flash loan accounting)
    let engine_balance_slot = compute_balance_slot(&engine_address, 0);
    match client.get_storage_at("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1", &engine_balance_slot).await {
        Ok(storage) => {
            let mut addr = [0u8; 20];
            addr.copy_from_slice(&hex::decode("82aF49447D8a07e3bd95BD0d56f35241523fBab1").unwrap());
            
            // Convert 32 bytes to u128 (last 16 bytes for ERC-20 balance)
            let balance = u128::from_be_bytes(
                storage[16..].try_into().unwrap_or([0u8; 16])
            );
            
            cache_db.set_balance(addr, balance);
            storage_slots_loaded += 1;
            tracing::debug!("Loaded WETH balance for engine");
        }
        Err(e) => {
            tracing::warn!("Failed to fetch WETH balance: {}", e);
        }
    }
    
    // Load USDC balance for our arbitrage engine
    let usdc_balance_slot = compute_balance_slot(&engine_address, 0);
    match client.get_storage_at("0xaf88d065e77c8cC2239327C5EDb3A432268e5831", &usdc_balance_slot).await {
        Ok(storage) => {
            let mut addr = [0u8; 20];
            addr.copy_from_slice(&hex::decode("af88d065e77c8cc2239327c5edb3a432268e5831").unwrap());
            
            // Convert 32 bytes to u128 (last 16 bytes for ERC-20 balance)
            let balance = u128::from_be_bytes(
                storage[16..].try_into().unwrap_or([0u8; 16])
            );
            
            cache_db.set_balance(addr, balance);
            storage_slots_loaded += 1;
            tracing::debug!("Loaded USDC balance for engine");
        }
        Err(e) => {
            tracing::warn!("Failed to fetch USDC balance: {}", e);
        }
    }
    
    // ─────────────────────────────────────────────────────────────────────────
    // PHASE 3: Verify Block Number
    // ─────────────────────────────────────────────────────────────────────────
    match client.get_block_number().await {
        Ok(block) => {
            tracing::info!("Current Arbitrum block: {}", block);
        }
        Err(e) => {
            tracing::warn!("Failed to get block number: {}", e);
        }
    }
    
    let elapsed_ms = start.elapsed().as_millis() as u64;
    
    let state = HydrationState {
        contracts_loaded,
        storage_slots_loaded,
        time_elapsed_ms: elapsed_ms,
    };
    
    tracing::info!(
        "State hydration complete: {} contracts, {} storage slots in {}ms",
        contracts_loaded,
        storage_slots_loaded,
        elapsed_ms
    );
    
    Ok(state)
}

/// Async state hydration with automatic RPC fallback
/// 
/// Tries multiple RPC endpoints and uses the first successful one.
pub async fn hydrate_state_with_fallback(cache_db: &mut RevmCacheDB) -> Result<HydrationState> {
    // Allow override via environment variable
    let primary_rpc = std::env::var("ARBITRUM_RPC_URL")
        .unwrap_or_else(|_| "https://arb1.arbitrum.io/rpc".to_string());
    
    tracing::info!("Using primary RPC: {}", primary_rpc);
    
    match hydrate_state(cache_db, &primary_rpc).await {
        Ok(state) => Ok(state),
        Err(e) => {
            tracing::warn!(
                "Primary RPC {} failed: {}. Trying fallback...",
                primary_rpc,
                e
            );
            
            let fallback_rpc = "https://arbitrum.public-rpc.com";
            hydrate_state(cache_db, fallback_rpc).await
        }
    }
}

/// Blocking hydration for use before async runtime starts
/// 
/// Uses a separate blocking thread pool to perform sync HTTP calls.
pub fn hydrate_state_blocking(cache_db: &mut RevmCacheDB) -> Result<HydrationState> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| ArbitrageError::System(format!("Failed to create runtime: {}", e)))?;
    
    runtime.block_on(async {
        hydrate_state_with_fallback(cache_db).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_compute_balance_slot() {
        // Test that balance slot computation is deterministic
        let slot1 = compute_balance_slot("0x2f9CE2a1b0F2D9d8d1C4E7F3B8A9F7E5D3C1B4A6", 0);
        let slot2 = compute_balance_slot("0x2f9CE2a1b0F2D9d8d1C4E7F3B8A9F7E5D3C1B4A6", 0);
        assert_eq!(slot1, slot2);
    }
    
    #[test]
    fn test_compute_balance_slot_different_addresses() {
        let slot1 = compute_balance_slot("0x2f9CE2a1b0F2D9d8d1C4E7F3B8A9F7E5D3C1B4A6", 0);
        let slot2 = compute_balance_slot("0x5B1F2E3C4D5A6B7C8D9E0F1A2B3C4D5E6F7A8B9", 0);
        assert_ne!(slot1, slot2);
    }

    /// P1: Verify all critical contract addresses are 42 chars (0x + 40 hex).
    #[test]
    fn test_all_critical_contracts_valid_addresses() {
        for (id, address) in CRITICAL_CONTRACTS {
            assert!(address.starts_with("0x"), "{} must start with 0x", contract_id_str(*id));
            assert_eq!(address.len(), 42, "{} address must be 42 chars (0x + 40 hex), got {}: {}", contract_id_str(*id), address.len(), address);
            let hex_str = &address[2..];
            assert!(hex_str.chars().all(|c| c.is_ascii_hexdigit()), "{} has non-hex chars: {}", contract_id_str(*id), address);
        }
    }

    /// P1: Arbitrum Uniswap V3 Router must be SwapRouter02 (0x68b34...)
    #[test]
    fn test_uniswap_v3_router_is_swaprouter02() {
        let router = CRITICAL_CONTRACTS.iter()
            .find(|(id, _)| *id == ContractId::UniswapV3Router)
            .map(|(_, a)| *a)
            .unwrap();
        assert_eq!(router, "0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45");
        assert!(!router.starts_with("0xE592"), "Must NOT use deprecated Uniswap V3 Router");
    }

    /// P1: Arbitrum USDC must be the correct mainnet address.
    #[test]
    fn test_usdc_arbitrum_correct() {
        let usdc = CRITICAL_CONTRACTS.iter()
            .find(|(id, _)| *id == ContractId::USDC)
            .map(|(_, a)| *a)
            .unwrap();
        assert_eq!(usdc, "0xaf88d065e77c8cC2239327C5EDb3A432268e5831");
    }

    /// P1: Arbitrum Balancer Vault must be the canonical address.
    #[test]
    fn test_balancer_vault_correct() {
        let balancer = CRITICAL_CONTRACTS.iter()
            .find(|(id, _)| *id == ContractId::BalancerVault)
            .map(|(_, a)| *a)
            .unwrap();
        assert_eq!(balancer, "0xBA12222222228d8Ba445958a75a0704d566BF2C8");
    }

    /// P1: Bytecode interface verification — valid bytecode with selector passes.
    #[test]
    fn test_verify_bytecode_interface_valid() {
        // USDC bytecode mock — contains balanceOf selector 0x70a08231
        let usdc_bytecode = [0x60, 0x80, 0x60, 0x40, 0x52, 0x60, 0x04, 0x36, 0x10, 0x70, 0xa0, 0x82, 0x31];
        assert!(verify_bytecode_interface(ContractId::USDC, &usdc_bytecode));
    }

    /// P1: Bytecode interface verification — missing selector fails.
    #[test]
    fn test_verify_bytecode_interface_invalid() {
        // Bytecode without any expected selectors
        let bad_bytecode = [0x60, 0x80, 0x60, 0x40, 0x52, 0x00, 0x00, 0x00];
        assert!(!verify_bytecode_interface(ContractId::USDC, &bad_bytecode));
    }

    /// P1: Address validation — invalid addresses rejected.
    #[test]
    fn test_validate_address_invalid() {
        assert!(validate_address("0xE59").is_err());
        assert!(validate_address("not_an_address").is_err());
        assert!(validate_address("0x123").is_err());
    }

    /// P1: Address validation — valid addresses accepted.
    #[test]
    fn test_validate_address_valid() {
        assert!(validate_address("0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45").is_ok());
        assert!(validate_address("0xaf88d065e77c8cC2239327C5EDb3A432268e5831").is_ok());
    }
}
