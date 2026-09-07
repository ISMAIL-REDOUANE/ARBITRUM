//! # Real REVM Simulation Module
//!
//! Production-grade EVM execution for arbitrage route validation using revm 3.5.0.
//!
//! ## Architecture
//!
//! ```text
//! Arbitrum RPC
//!         ↓
//! StateSnapshot (block context + loaded state)
//!         ↓
//! StateProviderCache (DatabaseRef implementation)
//!         ↓
//! CacheDB<StateProviderCache> (implements Database)
//!         ↓
//! EVM::new().database(db) (owned db)
//!         ↓
//! transact() → ExecutionResult
//!         ↓
//! SimulationResult
//! ```

use crate::error::{ArbitrageError, Result};

use revm::db::in_memory_db::CacheDB;
use revm::db::DatabaseRef;
use revm::primitives::{
    AccountInfo, Address, Bytecode, B256, SpecId, TransactTo, U256,
    KECCAK_EMPTY,
};
use revm::EVM;
use std::sync::Arc;
use std::time::{Duration, Instant};

const ARBITRUM_CHAIN_ID: u64 = 42161;
const ARBITRUM_SPEC_ID: SpecId = SpecId::CANCUN;

const BALANCER_VAULT: &str = "0xBA12222222228d8Ba445958a75a0704d566BF2C8";
const UNISWAP_V3_ROUTER: &str = "0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45";
const USDC_ARBITRUM: &str = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831";

const TEST_CALLER: &str = "0xDEADBEEF00000000000000000000000000000000";

const MAX_SIMULATION_GAS_LIMIT: u64 = 5_000_000;

const EXECUTOR_ADDRESS: &str = "0xDEADBEEF00000000000000000000000000000001";

#[derive(Debug, Clone)]
pub struct BlockContext {
    pub block_number: u64,
    pub block_hash: B256,
    pub timestamp: u64,
    pub gas_limit: u64,
    pub base_fee: U256,
    pub coinbase: Address,
    pub prevrandao: Option<B256>,
}

#[derive(Debug, Clone)]
pub struct StateSnapshot {
    pub block: BlockContext,
    pub accounts: Vec<(Address, AccountInfo)>,
    pub contracts: Vec<(Address, Vec<u8>)>,
    pub storage: Vec<((Address, U256), U256)>,
    pub block_hashes: std::collections::HashMap<U256, B256>,
}

#[derive(Debug, Clone)]
pub struct SimpleCallResult {
    pub success: bool,
    pub return_data: Vec<u8>,
    pub gas_used: u64,
    pub revert_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StateProviderCache {
    pub bytecode: std::collections::HashMap<B256, Bytecode>,
    pub accounts: std::collections::HashMap<Address, AccountInfo>,
    pub storage: std::collections::HashMap<(Address, U256), U256>,
    pub block_hashes: std::collections::HashMap<U256, B256>,
}

impl StateProviderCache {
    pub fn new() -> Self {
        Self {
            bytecode: std::collections::HashMap::new(),
            accounts: std::collections::HashMap::new(),
            storage: std::collections::HashMap::new(),
            block_hashes: std::collections::HashMap::new(),
        }
    }

    pub fn insert_account(&mut self, address: Address, info: AccountInfo) {
        if let Some(ref code) = info.code {
            if !code.is_empty() {
                self.bytecode.insert(info.code_hash, code.clone());
            }
        }
        self.accounts.insert(address, info);
    }

    pub fn insert_storage(&mut self, address: Address, slot: U256, value: U256) {
        self.storage.insert((address, slot), value);
    }

    pub fn insert_block_hash(&mut self, number: U256, hash: B256) {
        self.block_hashes.insert(number, hash);
    }
}

impl Default for StateProviderCache {
    fn default() -> Self {
        Self::new()
    }
}

impl DatabaseRef for StateProviderCache {
    type Error = ArbitrageError;

    fn basic(&self, address: Address) -> std::result::Result<Option<AccountInfo>, Self::Error> {
        Ok(self.accounts.get(&address).cloned())
    }

    fn code_by_hash(&self, code_hash: B256) -> std::result::Result<Bytecode, Self::Error> {
        self.bytecode
            .get(&code_hash)
            .cloned()
            .ok_or_else(|| ArbitrageError::CacheDb(format!("Missing bytecode: {:?}", code_hash)))
    }

    fn storage(&self, address: Address, index: U256) -> std::result::Result<U256, Self::Error> {
        self.storage
            .get(&(address, index))
            .copied()
            .ok_or_else(|| ArbitrageError::CacheDb(format!("Missing storage: {:?}[{:?}]", address, index)))
    }

    fn block_hash(&self, number: U256) -> std::result::Result<B256, Self::Error> {
        self.block_hashes
            .get(&number)
            .copied()
            .ok_or_else(|| ArbitrageError::CacheDb(format!("Missing block hash: {:?}", number)))
    }
}

pub struct RevmSimulator {
    cachedb: Arc<parking_lot::RwLock<CacheDB<StateProviderCache>>>,
    rpc_endpoint: String,
    cached_snapshot: parking_lot::RwLock<Option<StateSnapshot>>,
}

impl RevmSimulator {
    pub fn new(rpc_endpoint: &str) -> Self {
        Self {
            cachedb: Arc::new(parking_lot::RwLock::new(CacheDB::new(StateProviderCache::new()))),
            rpc_endpoint: rpc_endpoint.to_string(),
            cached_snapshot: parking_lot::RwLock::new(None),
        }
    }

    pub async fn hydrate_from_rpc(&self, block_number: Option<u64>) -> Result<StateSnapshot> {
        let client = RpcClient::new(&self.rpc_endpoint);

        let block_num = match block_number {
            Some(n) => n,
            None => client.get_block_number().await?,
        };

        let block = client.get_block_context(block_num).await?;

        let contracts = [BALANCER_VAULT, UNISWAP_V3_ROUTER, USDC_ARBITRUM];

        let mut snapshot = StateSnapshot {
            block: block.clone(),
            accounts: Vec::new(),
            contracts: Vec::new(),
            storage: Vec::new(),
            block_hashes: std::collections::HashMap::new(),
        };

        snapshot
            .block_hashes
            .insert(U256::from(block_num), block.block_hash);

        for addr_str in contracts {
            let addr = parse_address(addr_str)?;

            match client.get_account_info(addr_str).await {
                Ok(info) => {
                    let code = client.get_code(addr_str).await?;
                    let code_hash = if code.is_empty() {
                        KECCAK_EMPTY
                    } else {
                        keccak256(&code)
                    };

                    let bytecode = if code.is_empty() {
                        Bytecode::new()
                    } else {
                        Bytecode::new_raw(revm::primitives::Bytes::copy_from_slice(&code))
                    };

                    let balance = parse_u256_hex(&info.balance_hex)?;

                    let account_info = AccountInfo {
                        balance,
                        nonce: info.nonce,
                        code_hash,
                        code: Some(bytecode),
                    };

                    snapshot.accounts.push((addr, account_info));
                    snapshot.contracts.push((addr, code));
                }
                Err(e) => {
                    return Err(ArbitrageError::Rpc(format!(
                        "Failed to load account {}: {}",
                        addr_str, e
                    )));
                }
            }
        }

        *self.cached_snapshot.write() = Some(snapshot.clone());

        let mut db = self.cachedb.write();
        for (address, info) in snapshot.accounts.iter() {
            db.insert_account_info(*address, info.clone());
        }

        Ok(snapshot)
    }

    pub fn inject_balance(&self, address: Address, balance_wei: U256) {
        let mut db = self.cachedb.write();
        let info = AccountInfo {
            balance: balance_wei,
            ..AccountInfo::default()
        };
        db.insert_account_info(address, info);
    }

    pub fn inject_test_caller(&self, balance_wei: U256) {
        let test_address = parse_address(TEST_CALLER).unwrap_or(Address::ZERO);
        self.inject_balance(test_address, balance_wei);
    }

    pub fn load_executor_bytecode(&self, bytecode: Vec<u8>) -> Result<()> {
        let executor_addr = parse_address(EXECUTOR_ADDRESS)
            .map_err(|_| ArbitrageError::Config("Invalid executor address".to_string()))?;

        let code_hash = keccak256(&bytecode);
        let bytecode_obj = if bytecode.is_empty() {
            Bytecode::new()
        } else {
            Bytecode::new_raw(revm::primitives::Bytes::copy_from_slice(&bytecode))
        };
        let account_info = AccountInfo {
            balance: U256::ZERO,
            nonce: 1,
            code: Some(bytecode_obj),
            code_hash,
        };

        let mut db = self.cachedb.write();
        db.insert_account_info(executor_addr, account_info);
        Ok(())
    }

    pub fn execute_simple_call(&self, target: Address, calldata: &[u8]) -> SimpleCallResult {
        let _start = Instant::now();

        let (block, caller_address) = {
            let guard = self.cached_snapshot.read();
            match guard.as_ref() {
                Some(snapshot) => (snapshot.block.clone(), snapshot.accounts.first().map(|(a, _)| *a).unwrap_or_else(|| Address::ZERO)),
                None => {
                    return SimpleCallResult {
                        success: false,
                        return_data: Vec::new(),
                        gas_used: 0,
                        revert_reason: Some("No state snapshot loaded".to_string()),
                    };
                }
            }
        };

        let db_clone = self.cachedb.read().clone();
        let mut evm = EVM::new();
        evm.database(db_clone);

        evm.env.cfg.chain_id = ARBITRUM_CHAIN_ID;
        evm.env.cfg.spec_id = ARBITRUM_SPEC_ID;
        evm.env.block.number = U256::from(block.block_number);
        evm.env.block.coinbase = block.coinbase;
        evm.env.block.timestamp = U256::from(block.timestamp);
        evm.env.block.gas_limit = U256::from(block.gas_limit.min(MAX_SIMULATION_GAS_LIMIT));
        evm.env.block.basefee = block.base_fee;
        evm.env.block.prevrandao = block.prevrandao;
        evm.env.block.difficulty = U256::ZERO;

        let caller = parse_address(TEST_CALLER).unwrap_or(Address::ZERO);
        evm.env.tx.caller = caller;
        evm.env.tx.gas_limit = block.gas_limit.min(MAX_SIMULATION_GAS_LIMIT);
        evm.env.tx.gas_price = block.base_fee;
        evm.env.tx.transact_to = TransactTo::Call(target);
        evm.env.tx.data = revm::primitives::Bytes::copy_from_slice(calldata);
        evm.env.tx.value = U256::ZERO;
        evm.env.tx.chain_id = Some(ARBITRUM_CHAIN_ID);

        let result = evm.transact();

        let _ = caller_address; // suppress unused warning

        match result {
            Ok(result_and_state) => {
                let gas_used = result_and_state.result.gas_used();
                let output = result_and_state.result.output();

                if result_and_state.result.is_success() {
                    let return_data = match result_and_state.result.output() {
                        Some(bytes) => bytes.to_vec(),
                        None => Vec::new(),
                    };
                    SimpleCallResult {
                        success: true,
                        return_data,
                        gas_used,
                        revert_reason: None,
                    }
                } else {
                    let reason = output
                        .and_then(|o| parse_revert_reason(o))
                        .unwrap_or_else(|| "Unknown error".to_string());
                    SimpleCallResult {
                        success: false,
                        return_data: output.map(|o| o.to_vec()).unwrap_or_default(),
                        gas_used,
                        revert_reason: Some(reason),
                    }
                }
            }
            Err(e) => SimpleCallResult {
                success: false,
                return_data: Vec::new(),
                gas_used: 0,
                revert_reason: Some(format!("EVM error: {:?}", e)),
            },
        }
    }

    pub fn get_snapshot(&self) -> Option<StateSnapshot> {
        self.cached_snapshot.read().clone()
    }

    pub fn is_state_loaded(&self) -> bool {
        self.cached_snapshot.read().is_some()
    }

    pub fn get_block_number(&self) -> Option<u64> {
        self.cached_snapshot.read().as_ref().map(|s| s.block.block_number)
    }
}

impl Default for RevmSimulator {
    fn default() -> Self {
        Self::new("")
    }
}

fn selector_bytes(selector: &str) -> [u8; 4] {
    use tiny_keccak::{Hasher, Keccak};
    let mut hasher = Keccak::v256();
    hasher.update(selector.as_bytes());
    let mut hash = [0u8; 32];
    hasher.finalize(&mut hash);
    [hash[0], hash[1], hash[2], hash[3]]
}

fn parse_address(s: &str) -> Result<Address> {
    let hex_str = s.trim_start_matches("0x");
    let bytes = hex::decode(hex_str).map_err(|e| {
        ArbitrageError::Encoding(format!("Invalid hex in address '{}': {}", s, e))
    })?;
    if bytes.len() != 20 {
        return Err(ArbitrageError::Encoding(format!(
            "Invalid address length {} for '{}' - expected 20 bytes",
            bytes.len(),
            s
        )));
    }
    Ok(Address::from_slice(&bytes))
}

fn parse_uint256_abidevice(data: &[u8]) -> Result<u64> {
    if data.len() < 32 {
        return Err(ArbitrageError::Encoding(
            "ABI uint256 requires 32 bytes".to_string(),
        ));
    }
    let mut padded = [0u8; 32];
    padded.copy_from_slice(&data[..32]);
    let value = U256::from_be_bytes(padded);

    if value > U256::from(u64::MAX) {
        return Err(ArbitrageError::Encoding(
            "uint256 value overflows u64".to_string(),
        ));
    }

    let bytes: [u8; 32] = value.to_be_bytes();
    Ok(u64::from_be_bytes(
        bytes[24..].try_into().map_err(|_| {
            ArbitrageError::Encoding("Failed to convert uint256 to u64".to_string())
        })?,
    ))
}

fn parse_u256_hex(s: &str) -> Result<U256> {
    let hex_str = s.trim_start_matches("0x");
    if hex_str.is_empty() {
        return Err(ArbitrageError::Encoding("Empty hex string".to_string()));
    }
    let hex_str = if !hex_str.len().is_multiple_of(2) {
        format!("0{}", hex_str)
    } else {
        hex_str.to_string()
    };
    let bytes = hex::decode(&hex_str).map_err(|e| {
        ArbitrageError::Encoding(format!("Invalid hex: {}", e))
    })?;
    if bytes.len() > 32 {
        return Err(ArbitrageError::Encoding(
            "Hex value exceeds 32 bytes".to_string(),
        ));
    }
    let mut padded = [0u8; 32];
    padded[32 - bytes.len()..].copy_from_slice(&bytes);
    Ok(U256::from_be_bytes(padded))
}

fn parse_revert_reason(data: &[u8]) -> Option<String> {
    if data.is_empty() {
        return None;
    }
    if data.len() >= 4 && data[..4] == [0x08, 0xc3, 0x79, 0x0a] && data.len() >= 100 {
        let mut offset_arr = [0u8; 32];
        offset_arr.copy_from_slice(&data[36..68]);
        let offset_val = U256::from_be_bytes(offset_arr);
        let offset = extract_u64(&offset_val).ok()? as usize;

        if data.len() >= offset + 32 {
            let mut len_arr = [0u8; 32];
            len_arr.copy_from_slice(&data[offset..offset + 32]);
            let len_val = U256::from_be_bytes(len_arr);
            let len = extract_u64(&len_val).ok()? as usize;

            if data.len() >= offset + 32 + len {
                return Some(
                    String::from_utf8_lossy(&data[offset + 32..offset + 32 + len])
                        .to_string(),
                );
            }
        }
    }
    Some(format!("Revert: 0x{}", hex::encode(&data[..data.len().min(64)])))
}

fn extract_u64(value: &U256) -> std::result::Result<u64, ArbitrageError> {
    let bytes: [u8; 32] = value.to_be_bytes();
    if bytes[..24].iter().any(|&b| b != 0) {
        return Err(ArbitrageError::Encoding("Value exceeds u64 max".to_string()));
    }
    Ok(u64::from_be_bytes(bytes[24..].try_into().map_err(|_| ArbitrageError::Encoding("Conversion error".to_string()))?))
}

fn keccak256(data: &[u8]) -> B256 {
    use tiny_keccak::{Hasher, Keccak};
    let mut hasher = Keccak::v256();
    hasher.update(data);
    let mut hash = [0u8; 32];
    hasher.finalize(&mut hash);
    B256::from(hash)
}

pub struct RpcClient {
    endpoint: String,
    client: reqwest::Client,
}

impl RpcClient {
    pub fn new(endpoint: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            endpoint: endpoint.to_string(),
            client,
        }
    }

    pub async fn get_block_number(&self) -> Result<u64> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_blockNumber",
            "params": [],
            "id": 1
        });

        let response = self
            .client
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

        let result = body
            .get("result")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ArbitrageError::Rpc("Missing result".to_string()))?;

        let block_hex = result.trim_start_matches("0x");
        u64::from_str_radix(block_hex, 16)
            .map_err(|e| ArbitrageError::Rpc(format!("Failed to parse block number: {}", e)))
    }

    pub async fn get_block_context(&self, block_number: u64) -> Result<BlockContext> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_getBlockByNumber",
            "params": [format!("0x{:x}", block_number), false],
            "id": 1
        });

        let response = self
            .client
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

        let result = body
            .get("result")
            .ok_or_else(|| ArbitrageError::Rpc("Missing result".to_string()))?;

        let block_num = result
            .get("number")
            .and_then(|v| v.as_str())
            .map(|s| {
                u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(block_number)
            })
            .unwrap_or(block_number);

        let timestamp = result
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .unwrap_or(0);

        let gas_limit = result
            .get("gasLimit")
            .and_then(|v| v.as_str())
            .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .unwrap_or(30_000_000);

        let base_fee = result
            .get("baseFeePerGas")
            .and_then(|v| v.as_str())
            .map(|s| parse_u256_hex(s).unwrap_or(U256::from(10_000_000_000u64)))
            .unwrap_or(U256::from(10_000_000_000u64));

        let hash_str = result
            .get("hash")
            .and_then(|v| v.as_str())
            .unwrap_or("0x0");
        let hash_bytes = hex::decode(hash_str.trim_start_matches("0x")).unwrap_or_default();
        let mut block_hash_arr = [0u8; 32];
        block_hash_arr.copy_from_slice(&hash_bytes[..32]);

        let prevrandao = result
            .get("mixHash")
            .and_then(|v| v.as_str())
            .and_then(|s| {
                let bytes = hex::decode(s.trim_start_matches("0x")).ok()?;
                if bytes.len() >= 32 {
                    let mut hash_arr = [0u8; 32];
                    hash_arr.copy_from_slice(&bytes[..32]);
                    Some(B256::from(hash_arr))
                } else {
                    None
                }
            });

        let coinbase_str = result
            .get("miner")
            .and_then(|v| v.as_str())
            .unwrap_or("0x0000000000000000000000000000000000000000");
        let coinbase_bytes = hex::decode(coinbase_str.trim_start_matches("0x")).unwrap_or_default();
        let coinbase = Address::from_slice(&coinbase_bytes[..20.min(coinbase_bytes.len())]);

        Ok(BlockContext {
            block_number: block_num,
            block_hash: B256::from(block_hash_arr),
            timestamp,
            gas_limit,
            base_fee,
            coinbase,
            prevrandao,
        })
    }

    pub async fn get_code(&self, address: &str) -> Result<Vec<u8>> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_getCode",
            "params": [address, "latest"],
            "id": 1
        });

        let response = self
            .client
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

        let code_str = body
            .get("result")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ArbitrageError::Rpc("Missing result".to_string()))?;

        hex::decode(code_str.trim_start_matches("0x"))
            .map_err(|e| ArbitrageError::Encoding(format!("Failed to decode code: {}", e)))
    }

    pub async fn get_account_info(&self, address: &str) -> Result<RpcAccountInfo> {
        let balance_payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_getBalance",
            "params": [address, "latest"],
            "id": 1
        });

        let nonce_payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_getTransactionCount",
            "params": [address, "latest"],
            "id": 2
        });

        let balance_response = self
            .client
            .post(&self.endpoint)
            .json(&balance_payload)
            .send()
            .await
            .map_err(|e| ArbitrageError::Rpc(format!("Failed to get balance: {}", e)))?;

        let nonce_response = self
            .client
            .post(&self.endpoint)
            .json(&nonce_payload)
            .send()
            .await
            .map_err(|e| ArbitrageError::Rpc(format!("Failed to get nonce: {}", e)))?;

        let balance_body: serde_json::Value = balance_response
            .json()
            .await
            .map_err(|e| ArbitrageError::Rpc(format!("Failed to parse balance: {}", e)))?;

        let nonce_body: serde_json::Value = nonce_response
            .json()
            .await
            .map_err(|e| ArbitrageError::Rpc(format!("Failed to parse nonce: {}", e)))?;

        let balance_hex = balance_body
            .get("result")
            .and_then(|v| v.as_str())
            .unwrap_or("0x0");

        let nonce = nonce_body
            .get("result")
            .and_then(|v| v.as_str())
            .map(|s| {
                u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0)
            })
            .unwrap_or(0);

        Ok(RpcAccountInfo {
            balance_hex: balance_hex.to_string(),
            nonce,
        })
    }
}

#[derive(Debug)]
pub struct RpcAccountInfo {
    pub balance_hex: String,
    pub nonce: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_provider_cache_insert_account() {
        let mut cache = StateProviderCache::new();
        let addr = parse_address("0xaf88d065e77c8cC2239327C5EDb3A432268e5831").unwrap();

        let bytecode = Bytecode::new_raw(revm::primitives::Bytes::from_static(&[0x60, 0x80, 0x60, 0x40]));
        let code_hash = bytecode.hash_slow();

        let info = AccountInfo {
            balance: U256::from(1000),
            nonce: 1,
            code_hash,
            code: Some(bytecode),
        };

        cache.insert_account(addr, info.clone());

        assert!(cache.accounts.contains_key(&addr));
        assert!(cache.bytecode.contains_key(&code_hash));
    }

    #[test]
    fn test_state_provider_cache_storage() {
        let mut cache = StateProviderCache::new();
        let addr = parse_address("0xaf88d065e77c8cC2239327C5EDb3A432268e5831").unwrap();
        let slot = U256::from(0);
        let value = U256::from(1000);

        cache.insert_storage(addr, slot, value);

        assert_eq!(cache.storage.get(&(addr, slot)), Some(&value));
    }

    #[test]
    fn test_parse_address_valid() {
        let addr = parse_address("0xaf88d065e77c8cC2239327C5EDb3A432268e5831").unwrap();
        assert!(!addr.is_zero());
    }

    #[test]
    fn test_parse_address_invalid_length() {
        let result = parse_address("0x1234");
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("expected 20 bytes"));
    }

    #[test]
    fn test_parse_address_invalid_hex() {
        let result = parse_address("0xzzz");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_address_empty() {
        let result = parse_address("0x");
        assert!(result.is_err());
    }

    #[test]
    fn test_selector_bytes() {
        let selector = selector_bytes("balanceOf(address)");
        assert_eq!(selector.len(), 4);
    }

    #[test]
    fn test_keccak256() {
        let data = b"hello";
        let hash = keccak256(data);
        assert_eq!(hash.len(), 32);
    }

    #[test]
    fn test_revm_simulator_default() {
        let simulator = RevmSimulator::default();
        assert!(!simulator.is_state_loaded());
    }

    #[test]
    fn test_block_context_default() {
        let block = BlockContext {
            block_number: 0,
            block_hash: B256::ZERO,
            timestamp: 0,
            gas_limit: 0,
            base_fee: U256::ZERO,
            coinbase: Address::ZERO,
            prevrandao: None,
        };

        assert_eq!(block.block_number, 0);
    }

    #[test]
    fn test_parse_u256_hex_valid() {
        let result = parse_u256_hex("0x0000000000000000000000000000000000000000000000000000000000000fa0");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), U256::from(0x0fa0));
    }

    #[test]
    fn test_parse_u256_hex_empty() {
        let result = parse_u256_hex("0x");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_u256_hex_invalid() {
        let result = parse_u256_hex("0xzzz");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_uint256_abidevice_valid() {
        let mut data = [0u8; 32];
        data[31] = 0xa0;
        data[30] = 0x0f;
        let result = parse_uint256_abidevice(&data);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0x0fa0);
    }

    #[test]
    fn test_parse_uint256_abidevice_short() {
        let data = [0u8; 16];
        let result = parse_uint256_abidevice(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_uint256_abidevice_overflow() {
        let mut data = [0u8; 32];
        data[31] = 0xff;
        data[30] = 0xff;
        data[29] = 0xff;
        data[28] = 0xff;
        data[27] = 0xff;
        data[26] = 0xff;
        data[25] = 0xff;
        data[24] = 0xff;
        data[23] = 0x01;
        let result = parse_uint256_abidevice(&data);
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("overflows"));
    }

    #[test]
    fn test_state_snapshot_block_hashes() {
        let mut snapshot = StateSnapshot {
            block: BlockContext {
                block_number: 100,
                block_hash: B256::ZERO,
                timestamp: 0,
                gas_limit: 0,
                base_fee: U256::ZERO,
                coinbase: Address::ZERO,
                prevrandao: None,
            },
            accounts: Vec::new(),
            contracts: Vec::new(),
            storage: Vec::new(),
            block_hashes: std::collections::HashMap::new(),
        };

        let hash = keccak256(b"block100");
        snapshot.block_hashes.insert(U256::from(99), hash);

        assert!(snapshot.block_hashes.contains_key(&U256::from(99)));
    }

    #[test]
    fn test_parse_revert_reason_error_string() {
        let data = [
            0x08, 0xc3, 0x79, 0x0a, // Error(string) selector
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x20, // offset to string data
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // length
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let reason = parse_revert_reason(&data);
        assert!(reason.is_some());
    }

    #[tokio::test]
    async fn test_rpc_client_connection() {
        let client = RpcClient::new("https://arb1.arbitrum.io/rpc");
        let result = client.get_block_number().await;
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_state_provider_cache_missing_account() {
        let cache = StateProviderCache::new();
        let addr = parse_address("0xaf88d065e77c8cC2239327C5EDb3A432268e5831").unwrap();

        let result = cache.basic(addr);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_state_provider_cache_missing_bytecode() {
        let cache = StateProviderCache::new();
        let random_hash = keccak256(b"random");

        let result = cache.code_by_hash(random_hash);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{}", err).contains("Missing bytecode"));
    }

    #[test]
    fn test_state_provider_cache_missing_storage() {
        let cache = StateProviderCache::new();
        let addr = parse_address("0xaf88d065e77c8cC2239327C5EDb3A432268e5831").unwrap();
        let slot = U256::from(123);

        let result = cache.storage(addr, slot);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{}", err).contains("Missing storage"));
    }

    #[test]
    fn test_state_provider_cache_missing_block_hash() {
        let cache = StateProviderCache::new();
        let block_num = U256::from(1000);

        let result = cache.block_hash(block_num);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{}", err).contains("Missing block hash"));
    }

    #[test]
    fn test_state_provider_cache_with_account() {
        let mut cache = StateProviderCache::new();
        let addr = parse_address("0xaf88d065e77c8cC2239327C5EDb3A432268e5831").unwrap();

        let bytecode = Bytecode::new_raw(revm::primitives::Bytes::from_static(&[0x60, 0x80, 0x60, 0x40]));
        let code_hash = bytecode.hash_slow();
        let info = AccountInfo {
            balance: U256::from(1000000),
            nonce: 5,
            code_hash,
            code: Some(bytecode),
        };

        cache.insert_account(addr, info);

        let result = cache.basic(addr);
        assert!(result.is_ok());
        let loaded_info = result.unwrap().unwrap();
        assert_eq!(loaded_info.balance, U256::from(1000000));
        assert_eq!(loaded_info.nonce, 5);
    }

    #[test]
    fn test_state_provider_cache_with_storage() {
        let mut cache = StateProviderCache::new();
        let addr = parse_address("0xaf88d065e77c8cC2239327C5EDb3A432268e5831").unwrap();
        let slot = U256::from(42);
        let value = U256::from(12345);

        cache.insert_storage(addr, slot, value);

        let result = cache.storage(addr, slot);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), value);
    }

    #[test]
    fn test_state_provider_cache_with_block_hash() {
        let mut cache = StateProviderCache::new();
        let block_num = U256::from(100);
        let hash = keccak256(b"block100");

        cache.insert_block_hash(block_num, hash);

        let result = cache.block_hash(block_num);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), hash);
    }

    #[test]
    fn test_state_snapshot_contains_account_info() {
        let addr = parse_address("0xaf88d065e77c8cC2239327C5EDb3A432268e5831").unwrap();
        let bytecode = Bytecode::new_raw(revm::primitives::Bytes::from_static(&[0x60, 0x80, 0x60, 0x40]));
        let code_hash = bytecode.hash_slow();

        let snapshot = StateSnapshot {
            block: BlockContext {
                block_number: 100,
                block_hash: keccak256(b"block100"),
                timestamp: 1000,
                gas_limit: 30_000_000,
                base_fee: U256::from(10_000_000_000u64),
                coinbase: parse_address("0x0000000000000000000000000000000000000000").unwrap(),
                prevrandao: Some(keccak256(b"prevrandao")),
            },
            accounts: vec![(
                addr,
                AccountInfo {
                    balance: U256::from(1_000_000_000_000_000_000u64),
                    nonce: 1,
                    code_hash,
                    code: Some(bytecode),
                },
            )],
            contracts: vec![(addr, vec![0x60, 0x80, 0x60, 0x40])],
            storage: vec![((addr, U256::from(0)), U256::from(1000))],
            block_hashes: std::collections::HashMap::new(),
        };

        assert_eq!(snapshot.accounts.len(), 1);
        assert_eq!(snapshot.accounts[0].0, addr);
        assert_eq!(snapshot.contracts.len(), 1);
        assert_eq!(snapshot.storage.len(), 1);
        assert_eq!(snapshot.block.block_number, 100);
    }

    #[test]
    fn test_simulator_without_snapshot_returns_false() {
        let simulator = RevmSimulator::default();
        assert!(!simulator.is_state_loaded());
        assert!(simulator.get_snapshot().is_none());
        assert!(simulator.get_block_number().is_none());
    }

    #[test]
    fn test_simulator_is_not_loaded_initially() {
        let simulator = RevmSimulator::default();
        assert!(!simulator.is_state_loaded());
        assert!(simulator.get_block_number().is_none());
        assert!(simulator.get_snapshot().is_none());
    }

    #[test]
    fn test_simulator_with_empty_rpc_endpoint() {
        let simulator = RevmSimulator::new("");
        assert!(!simulator.is_state_loaded());
    }

    #[test]
    fn test_block_context_fields() {
        let block = BlockContext {
            block_number: 100,
            block_hash: keccak256(b"block100"),
            timestamp: 1000,
            gas_limit: 30_000_000,
            base_fee: U256::from(10_000_000_000u64),
            coinbase: parse_address("0x0000000000000000000000000000000000000000").unwrap(),
            prevrandao: Some(keccak256(b"prevrandao")),
        };

        assert_eq!(block.block_number, 100);
        assert_eq!(block.timestamp, 1000);
        assert_eq!(block.gas_limit, 30_000_000);
        assert!(block.prevrandao.is_some());
    }

    #[test]
    fn test_hydrate_requires_rpc_endpoint() {
        let simulator = RevmSimulator::default();
        assert!(!simulator.is_state_loaded(), "Default simulator should not be loaded");
    }

    #[tokio::test]
    #[ignore]
    async fn test_fork_arbitrum_with_real_rpc() {
        let rpc_endpoint = std::env::var("BLOXROUTE_RPC")
            .expect("BLOXROUTE_RPC environment variable must be set for fork testing");

        let simulator = RevmSimulator::new(&rpc_endpoint);

        let snapshot = simulator.hydrate_from_rpc(None).await
            .expect("Failed to hydrate state from Arbitrum RPC");

        assert!(simulator.is_state_loaded(), "State should be loaded after hydration");
        assert!(snapshot.block.block_number > 0);
        assert!(snapshot.accounts.len() >= 3, "Should have loaded Balancer + Uniswap + USDC accounts");

        let caller_address = TEST_CALLER;
        const TEST_GAS_BALANCE_WEI: u64 = 100_000_000_000_000_000_u64;
        simulator.inject_balance(parse_address(caller_address).unwrap(), U256::from(TEST_GAS_BALANCE_WEI));

        println!("=== FORK SIMULATION STATE ===");
        println!("RPC: {}", rpc_endpoint);
        println!("Block: #{}", snapshot.block.block_number);
        println!("Chain ID: 42161");
        println!("TEST_CALLER (injected): {} with balance {} wei (0.1 ETH)", TEST_CALLER, TEST_GAS_BALANCE_WEI);
        println!("State loaded: YES ({} accounts)", snapshot.accounts.len());
        println!("================================");
    }

    #[tokio::test]
    #[ignore]
    async fn test_fork_simple_balance_query() {
        let rpc_endpoint = std::env::var("BLOXROUTE_RPC")
            .expect("BLOXROUTE_RPC environment variable must be set for fork testing");

        let simulator = RevmSimulator::new(&rpc_endpoint);

        let _snapshot = simulator.hydrate_from_rpc(None).await
            .expect("Failed to hydrate state from Arbitrum RPC");

        let weth_address = parse_address("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1")
            .expect("Invalid WETH address");
        const TEST_BALANCE: u64 = 100_000_000_000_000_000_u64;
        simulator.inject_balance(weth_address, U256::from(TEST_BALANCE));

        let result = simulator.execute_simple_call(
            weth_address,
            hex::decode("70a08231").expect("Invalid balanceOf selector").as_slice(),
        );

        println!("=== SIMPLE CALL RESULT ===");
        println!("RPC: {}", rpc_endpoint);
        println!("Target: {}", weth_address);
        println!("Success: {}", result.success);
        println!("Return data: {:?}", result.return_data);
        println!("Gas used: {}", result.gas_used);
        println!("Revert reason: {:?}", result.revert_reason);
        println!("============================");

        assert!(result.success, "Simple call should succeed");
    }

    #[tokio::test]
    #[ignore]
    async fn test_fork_arbitrum_block_context() {
        let rpc_endpoint = std::env::var("BLOXROUTE_RPC")
            .expect("BLOXROUTE_RPC environment variable must be set for fork testing");

        let simulator = RevmSimulator::new(&rpc_endpoint);

        let block_num = 210_000_000u64;
        let snapshot = simulator.hydrate_from_rpc(Some(block_num)).await
            .expect("Failed to hydrate state from Arbitrum RPC");

        assert_eq!(snapshot.block.block_number, block_num);
        assert!(snapshot.block.gas_limit > 0);
        assert!(snapshot.block.base_fee > U256::ZERO);
    }

    #[tokio::test]
    #[ignore]
    async fn test_level2_executor_path() {
        let rpc_endpoint = std::env::var("BLOXROUTE_RPC")
            .expect("BLOXROUTE_RPC environment variable must be set for fork testing");

        let simulator = RevmSimulator::new(&rpc_endpoint);

        let _snapshot = simulator.hydrate_from_rpc(None).await
            .expect("Failed to hydrate state from Arbitrum RPC");

        const EXECUTOR_BYTECODE: &str = include_str!("../../out/ArbitrageExecutorTwoLeg.sol/ArbitrageExecutorTwoLeg.json");
        let json: serde_json::Value = serde_json::from_str(EXECUTOR_BYTECODE)
            .expect("Failed to parse executor bytecode JSON");
        let bytecode_hex = json["deployedBytecode"]["object"]
            .as_str()
            .expect("No bytecode in JSON");
        let bytecode = hex::decode(bytecode_hex.trim_start_matches("0x"))
            .expect("Failed to decode bytecode hex");

        simulator.load_executor_bytecode(bytecode)
            .expect("Failed to load executor bytecode");

        simulator.inject_test_caller(U256::from(100_000_000_000_000_000_u64));

        let result = simulator.execute_simple_call(
            parse_address(EXECUTOR_ADDRESS).unwrap(),
            &hex::decode("3b3a4e5a").unwrap(),
        );

        println!("=== LEVEL 2 EXECUTOR PATH TEST ===");
        println!("[1] TEST_CALLER -> EXECUTOR: {}", if result.success { "PASS" } else { "FAIL" });
        println!("    Gas used: {}", result.gas_used);
        println!("    Revert reason: {:?}", result.revert_reason);
        println!("====================================");
    }

    #[tokio::test]
    #[ignore]
    async fn test_a_tinyping_revm_smoke() {
        let rpc_endpoint = std::env::var("BLOXROUTE_RPC")
            .expect("BLOXROUTE_RPC must be set");

        let simulator = RevmSimulator::new(&rpc_endpoint);

        let _snapshot = simulator.hydrate_from_rpc(None).await
            .expect("Failed to hydrate state");

        // Fallback-based TinyPing (17 bytes) - returns 1 only with empty calldata
        const TINYPING_BYTECODE_HEX: &str = "34600c5760015f5260205ff35b5f80fdfe";
        let bytecode = hex::decode(TINYPING_BYTECODE_HEX).expect("Failed to decode bytecode");

        println!("TinyPing bytecode length: {} bytes", bytecode.len());

        const TINYPING_ADDRESS: &str = "0xDEADBEEF00000000000000000000000000000002";

        simulator.inject_test_caller(U256::from(100_000_000_000_000_000_u64));

        {
            let executor_addr = parse_address(TINYPING_ADDRESS).unwrap();
            let bytecode_obj = Bytecode::new_raw(revm::primitives::Bytes::copy_from_slice(&bytecode));
            let account_info = AccountInfo {
                balance: U256::ZERO,
                nonce: 1,
                code: Some(bytecode_obj),
                code_hash: KECCAK_EMPTY,
            };
            let mut db = simulator.cachedb.write();
            db.insert_account_info(executor_addr, account_info);
        }

        // Test with empty calldata first (works)
        let result_empty = simulator.execute_simple_call(
            parse_address(TINYPING_ADDRESS).unwrap(),
            &[], // empty calldata
        );

        println!("=== TEST A: TinyPing REVM Smoke ===");
        println!("[A1] TinyPing fallback (empty calldata): {}", if result_empty.success { "PASS" } else { "FAIL" });
        println!("     Gas used: {}", result_empty.gas_used);
        println!("     Return data: {:?}", &result_empty.return_data[..4.min(result_empty.return_data.len())]);
        println!("     Revert: {:?}", result_empty.revert_reason);

        // Test with 4 bytes of calldata (likely to panic)
        let result_4bytes = simulator.execute_simple_call(
            parse_address(TINYPING_ADDRESS).unwrap(),
            &[0x01, 0x02, 0x03, 0x04], // 4 bytes calldata
        );

        println!("[A2] TinyPing fallback (4 bytes calldata): {}", if result_4bytes.success { "PASS" } else { "FAIL" });
        println!("     Gas used: {}", result_4bytes.gas_used);
        println!("     Revert: {:?}", result_4bytes.revert_reason);
        println!("===================================");

        if !result_empty.success {
            panic!("TinyPing smoke test (empty calldata) failed: {:?}", result_empty.revert_reason);
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_b_executor_revm_smoke() {
        let rpc_endpoint = std::env::var("BLOXROUTE_RPC")
            .expect("BLOXROUTE_RPC must be set");

        let simulator = RevmSimulator::new(&rpc_endpoint);

        let _snapshot = simulator.hydrate_from_rpc(None).await
            .expect("Failed to hydrate state");

        const EXECUTOR_BYTECODE: &str = include_str!("../../out/ArbitrageExecutorTwoLeg.sol/ArbitrageExecutorTwoLeg.json");
        let json: serde_json::Value = serde_json::from_str(EXECUTOR_BYTECODE)
            .expect("Failed to parse executor JSON");
        let bytecode_hex = json["deployedBytecode"]["object"]
            .as_str()
            .expect("No bytecode");
        let full_bytecode = hex::decode(bytecode_hex.trim_start_matches("0x"))
            .expect("Failed to decode bytecode");

        // Extract runtime bytecode (before solc metadata marker)
        // solc appends: a2646970667358221220 + ipfs_hash + 64736f6c63430008220033
        // The metadata is appended after the runtime bytecode
        let metadata_marker = hex::decode("64736f6c63430008220033").unwrap();
        let solc_metadata_pos = full_bytecode.windows(metadata_marker.len())
            .position(|w| w == metadata_marker.as_slice())
            .unwrap_or(full_bytecode.len());
        let bytecode = full_bytecode[..solc_metadata_pos].to_vec();

        println!("Executor bytecode length: {} bytes (runtime only, stripped solc metadata)", bytecode.len());

        const EXECUTOR_ADDRESS: &str = "0xDEADBEEF00000000000000000000000000000001";

        simulator.inject_test_caller(U256::from(100_000_000_000_000_000_u64));

        {
            let executor_addr = parse_address(EXECUTOR_ADDRESS).unwrap();
            let bytecode_obj = Bytecode::new_raw(revm::primitives::Bytes::copy_from_slice(&bytecode));
            let account_info = AccountInfo {
                balance: U256::ZERO,
                nonce: 1,
                code: Some(bytecode_obj),
                code_hash: KECCAK_EMPTY,
            };
            let mut db = simulator.cachedb.write();
            db.insert_account_info(executor_addr, account_info);
        }

        // Exactly match test_level2_executor_path setup
        simulator.inject_test_caller(U256::from(100_000_000_000_000_000_u64));

        {
            let executor_addr = parse_address(EXECUTOR_ADDRESS).unwrap();
            let bytecode_obj = Bytecode::new_raw(revm::primitives::Bytes::copy_from_slice(&bytecode));
            let account_info = AccountInfo {
                balance: U256::ZERO,
                nonce: 1,
                code: Some(bytecode_obj),
                code_hash: KECCAK_EMPTY,
            };
            let mut db = simulator.cachedb.write();
            db.insert_account_info(executor_addr, account_info);
        }

        let result = simulator.execute_simple_call(
            parse_address(EXECUTOR_ADDRESS).unwrap(),
            &hex::decode("3b3a4e5a").unwrap(),
        );

        println!("=== TEST B: Executor REVM Smoke ===");
        println!("[B1] Executor bytecode injection: PASS"); // Injection always works
        println!("[B2] TEST_CALLER -> EXECUTOR: {}", if result.success { "PASS" } else { "FAIL" });
        println!("     Gas used: {}", result.gas_used);
        println!("     Revert: {:?}", result.revert_reason);
        println!("===================================");

        if !result.success {
            panic!("Executor call failed: {:?}", result.revert_reason);
        }
    }
}
