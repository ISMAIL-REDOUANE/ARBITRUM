//! # Pool Discovery Module
//!
//! Discovers and tracks liquidity pools from Uniswap V2/V3 factories and other DEXes.

use crate::error::{ArbitrageError, Result};
use crate::types::{DexType, PoolState};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

const ARBITRUM_CHAIN_ID: u64 = 42161;

/// Factory configuration for different DEXes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactoryType {
    UniswapV2,
    UniswapV3,
    SushiSwap,
    Aerodrome,
}

impl FactoryType {
    pub fn address(&self, chain_id: u64) -> Option<[u8; 20]> {
        match chain_id {
            1 => match self {
                FactoryType::UniswapV2 => {
                    Some(hex_to_addr("0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f"))
                }
                FactoryType::UniswapV3 => {
                    Some(hex_to_addr("0x1F98431c8aD98523631AE4a59f267346ea31F984"))
                }
                FactoryType::SushiSwap => {
                    Some(hex_to_addr("0xC0AEe478e3658e2610c5F7A4A2E1777cE9e4f2c"))
                }
                FactoryType::Aerodrome => None,
            },
            42161 => match self {
                // Arbitrum Uniswap V2 Factory: 0x8605c81479211E8e5B7a97D91cD585276077412B
                FactoryType::UniswapV2 => {
                    Some(hex_to_addr("0x8605c81479211E8e5B7a97D91cD585276077412B"))
                }
                // Arbitrum Uniswap V3 Factory: 0x1F98431c8aD98523631AE4a59f267346ea31F984
                FactoryType::UniswapV3 => {
                    Some(hex_to_addr("0x1F98431c8aD98523631AE4a59f267346ea31F984"))
                }
                // Arbitrum SushiSwap Factory: 0x1b02da8cb0d097cb8d57d3c6efb1b86d8f2e2b3F (verified)
                FactoryType::SushiSwap => {
                    Some(hex_to_addr("0x1b02da8cb0d097cb8d57d3c6efb1b86d8f2e2b3F"))
                }
                // Aerodrome is Base-only — not deployed on Arbitrum
                FactoryType::Aerodrome => None,
            },
            8453 => match self {
                FactoryType::UniswapV2 => {
                    Some(hex_to_addr("0x33128a8fAC17884797f674A8F92d0DD8E1f6d28c"))
                }
                FactoryType::UniswapV3 => {
                    Some(hex_to_addr("0x33128a8fAC17884797f674A8F92d0DD8E1f6d28c"))
                }
                FactoryType::SushiSwap => None,
                FactoryType::Aerodrome => {
                    Some(hex_to_addr("0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f"))
                }
            },
            _ => None,
        }
    }
}

fn hex_to_addr(hex: &str) -> [u8; 20] {
    let bytes = hex::decode(hex.trim_start_matches("0x")).unwrap_or_default();
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&bytes[..20]);
    addr
}

/// Token pair identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenPair {
    pub token0: String,
    pub token1: String,
}

impl TokenPair {
    pub fn new_hex(token0_hex: &str, token1_hex: &str) -> Self {
        let t0 = token0_hex.to_lowercase();
        let t1 = token1_hex.to_lowercase();
        if t0 < t1 {
            Self {
                token0: t0,
                token1: t1,
            }
        } else {
            Self {
                token0: t1,
                token1: t0,
            }
        }
    }

    pub fn contains_hex(&self, token_hex: &str) -> bool {
        let token = token_hex.to_lowercase();
        self.token0 == token || self.token1 == token
    }
}

/// Pool with enriched state for scoring
#[derive(Debug, Clone)]
pub struct EnrichedPool {
    pub base: PoolState,
    pub dex_type: DexType,
    pub factory_type: FactoryType,
    pub token_pair: TokenPair,
    pub sqrt_price_x96: Option<u128>,
    pub current_tick: Option<i32>,
    pub executable_liquidity: u128,
    pub active_liquidity: u128,
    pub recent_swaps: VecDeque<SwapRecord>,
    pub recent_volume: u128,
    pub price_movement_bps: u32,
    pub last_swap_block: u64,
    pub last_update_block: u64,
    pub freshness: PoolFreshness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolFreshness {
    Fresh,
    Stale,
    Expired,
}

impl EnrichedPool {
    pub fn is_fresh(&self) -> bool {
        self.freshness == PoolFreshness::Fresh
    }

    pub fn is_usable(&self) -> bool {
        self.executable_liquidity > 0 && self.active_liquidity > 0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SwapRecord {
    pub amount0: i128,
    pub amount1: i128,
    pub sqrt_price_x96_after: u128,
    pub tick_after: i32,
    pub block_number: u64,
    pub timestamp: u64,
    pub fee_tier: u32,
}

impl Default for EnrichedPool {
    fn default() -> Self {
        Self {
            base: PoolState {
                address: String::new(),
                token0: String::new(),
                token1: String::new(),
                reserve0: 0,
                reserve1: 0,
                fee_tier: 0,
                liquidity: 0,
                sqrt_price_x96: 0,
                current_tick: None,
                last_update: 0,
            },
            dex_type: DexType::UniswapV2,
            factory_type: FactoryType::UniswapV2,
            token_pair: TokenPair {
                token0: String::new(),
                token1: String::new(),
            },
            sqrt_price_x96: None,
            current_tick: None,
            executable_liquidity: 0,
            active_liquidity: 0,
            recent_swaps: VecDeque::with_capacity(100),
            recent_volume: 0,
            price_movement_bps: 0,
            last_swap_block: 0,
            last_update_block: 0,
            freshness: PoolFreshness::Expired,
        }
    }
}

/// Pool discovery configuration
#[derive(Debug, Clone)]
pub struct PoolDiscoveryConfig {
    pub chain_id: u64,
    pub factories: Vec<FactoryType>,
    pub block_batch_size: usize,
    pub max_pools_per_dex: usize,
    pub refresh_interval_blocks: u64,
    pub staleness_threshold_blocks: u64,
}

impl Default for PoolDiscoveryConfig {
    fn default() -> Self {
        Self {
            chain_id: ARBITRUM_CHAIN_ID,
            factories: vec![FactoryType::UniswapV3, FactoryType::UniswapV2],
            block_batch_size: 100,
            max_pools_per_dex: 1000,
            refresh_interval_blocks: 5,
            staleness_threshold_blocks: 3,
        }
    }
}

/// Pool registry - maintains discovered pools
pub struct PoolRegistry {
    pools: RwLock<HashMap<String, EnrichedPool>>,
    config: PoolDiscoveryConfig,
}

impl PoolRegistry {
    pub fn new(config: PoolDiscoveryConfig) -> Self {
        Self {
            pools: RwLock::new(HashMap::new()),
            config,
        }
    }

    pub fn get(&self, address: &str) -> Option<EnrichedPool> {
        self.pools.read().get(address).cloned()
    }

    pub fn get_by_token_pair(&self, pair: &TokenPair) -> Vec<EnrichedPool> {
        self.pools
            .read()
            .values()
            .filter(|p| p.token_pair == *pair)
            .cloned()
            .collect()
    }

    pub fn get_fresh_pools(&self) -> Vec<EnrichedPool> {
        self.pools
            .read()
            .values()
            .filter(|p| p.is_fresh() && p.is_usable())
            .cloned()
            .collect()
    }

    pub fn get_usable_pools(&self) -> Vec<EnrichedPool> {
        self.pools
            .read()
            .values()
            .filter(|p| p.is_usable())
            .cloned()
            .collect()
    }

    pub fn update_pool(&self, address: String, pool: EnrichedPool) {
        let mut pools = self.pools.write();
        pools.insert(address, pool);
    }

    pub fn remove_stale_pools(&self, current_block: u64) {
        let threshold = self.config.staleness_threshold_blocks;
        let mut pools = self.pools.write();
        pools.retain(|_, pool| current_block - pool.last_update_block <= threshold);
    }

    pub fn pool_count(&self) -> usize {
        self.pools.read().len()
    }

    pub fn get_all_addresses(&self) -> Vec<String> {
        self.pools.read().keys().cloned().collect()
    }
}

/// RPC client for pool state queries
pub struct PoolRpcClient {
    endpoint: String,
    client: reqwest::Client,
}

impl PoolRpcClient {
    pub fn new(endpoint: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            endpoint: endpoint.to_string(),
            client,
        }
    }

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

        body.get("result")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| ArbitrageError::Rpc("Missing result in RPC response".to_string()))
    }
}

/// Uniswap V2 pool state parser
pub struct UniswapV2PoolParser;

impl UniswapV2PoolParser {
    pub fn parse_reserves(data: &str) -> Result<(u128, u128, u32)> {
        let hex = data.trim_start_matches("0x");
        let bytes = hex::decode(hex)
            .map_err(|e| ArbitrageError::Encoding(format!("Failed to decode reserves: {}", e)))?;

        if bytes.len() < 96 {
            return Err(ArbitrageError::Encoding(
                "Invalid reserves data length".to_string(),
            ));
        }

        let reserve0 = decode_uint112(&bytes[0..32]);
        let reserve1 = decode_uint112(&bytes[32..64]);
        let block_timestamp_last = decode_uint32(&bytes[64..96]);

        Ok((reserve0, reserve1, block_timestamp_last))
    }
}

fn decode_uint112(data: &[u8]) -> u128 {
    let mut padded = [0u8; 16];
    let len = std::cmp::min(data.len(), 16);
    padded[16 - len..].copy_from_slice(&data[..len]);
    u128::from_be_bytes(padded)
}

fn decode_uint32(data: &[u8]) -> u32 {
    let mut padded = [0u8; 4];
    let len = std::cmp::min(data.len(), 4);
    padded[4 - len..].copy_from_slice(&data[..len]);
    u32::from_be_bytes(padded)
}

/// Uniswap V3 pool state parser
pub struct UniswapV3PoolParser;

impl UniswapV3PoolParser {
    pub fn parse_slot0(data: &str) -> Result<(u128, i32)> {
        let hex = data.trim_start_matches("0x");
        let bytes = hex::decode(hex)
            .map_err(|e| ArbitrageError::Encoding(format!("Failed to decode slot0: {}", e)))?;

        if bytes.len() < 64 {
            return Err(ArbitrageError::Encoding(
                "Invalid slot0 data length".to_string(),
            ));
        }

        let sqrt_price_x96 = decode_uint256(&bytes[0..32]);
        let tick = i32::from_be_bytes(bytes[44..48].try_into().unwrap_or([0u8; 4]));

        Ok((sqrt_price_x96, tick))
    }

    pub fn parse_liquidity(data: &str) -> Result<u128> {
        let hex = data.trim_start_matches("0x");
        let bytes = hex::decode(hex)
            .map_err(|e| ArbitrageError::Encoding(format!("Failed to decode liquidity: {}", e)))?;

        if bytes.len() < 32 {
            return Err(ArbitrageError::Encoding(
                "Invalid liquidity data length".to_string(),
            ));
        }

        Ok(decode_uint128(&bytes[16..32]))
    }
}

fn decode_uint256(data: &[u8]) -> u128 {
    let mut padded = [0u8; 16];
    let len = std::cmp::min(data.len(), 16);
    padded[16 - len..].copy_from_slice(&data[..len]);
    u128::from_be_bytes(padded)
}

fn decode_uint128(data: &[u8]) -> u128 {
    let mut padded = [0u8; 16];
    let len = std::cmp::min(data.len(), 16);
    padded[16 - len..].copy_from_slice(&data[..len]);
    u128::from_be_bytes(padded)
}

/// Calculate executable liquidity for concentrated liquidity pools
pub fn calculate_executable_liquidity(
    pool: &EnrichedPool,
    _tick_spacing: i32,
    _range_multiplier: f64,
) -> u128 {
    let Some(_current_tick) = pool.current_tick else {
        return pool.active_liquidity;
    };

    let liquidity_fraction = 0.5;
    (pool.active_liquidity as f64 * liquidity_fraction) as u128
}

/// Pool discovery engine
pub struct PoolDiscovery {
    rpc_client: PoolRpcClient,
    registry: Arc<PoolRegistry>,
    config: PoolDiscoveryConfig,
}

impl PoolDiscovery {
    pub fn new(rpc_endpoint: &str, config: PoolDiscoveryConfig) -> Self {
        Self {
            rpc_client: PoolRpcClient::new(rpc_endpoint),
            registry: Arc::new(PoolRegistry::new(config.clone())),
            config,
        }
    }

    pub fn registry(&self) -> Arc<PoolRegistry> {
        self.registry.clone()
    }

    pub async fn discover_uniswap_v2_pools(&self, factory: [u8; 20]) -> Result<Vec<[u8; 20]>> {
        let factory_hex = hex::encode(factory);
        let data = "0x18160ddd";
        let result = self.rpc_client.call(&factory_hex, data).await?;

        let hex = result.trim_start_matches("0x");
        let count_bytes = hex::decode(hex)
            .map_err(|e| ArbitrageError::Encoding(format!("Failed to decode pair count: {}", e)))?;

        let pair_count = decode_uint256(&count_bytes) as usize;

        tracing::info!(
            "Uniswap V2 factory {} has {} pairs",
            factory_hex,
            pair_count
        );

        let mut pools = Vec::new();
        let max_pools = self.config.max_pools_per_dex;

        for i in 0..std::cmp::min(pair_count, max_pools) {
            let idx = encode_uint256(i as u128);
            let data = format!("0x6a627842{}", hex::encode(idx));
            let result = self.rpc_client.call(&factory_hex, &data).await?;

            if let Ok(pool_addr) = parse_address_from_result(&result) {
                pools.push(pool_addr);
            }
        }

        Ok(pools)
    }

    pub async fn sync_uniswap_v2_pool(&self, pool_addr: [u8; 20]) -> Result<Option<EnrichedPool>> {
        let pool_hex = hex::encode(pool_addr);
        let data = "0x0902f13c";
        let result = self.rpc_client.call(&pool_hex, data).await?;

        let (reserve0, reserve1, _) = UniswapV2PoolParser::parse_reserves(&result)?;

        let token0_data = "0x0dfe1681";
        let token1_data = "0xd21220a7";

        let token0_result = self.rpc_client.call(&pool_hex, token0_data).await?;
        let token1_result = self.rpc_client.call(&pool_hex, token1_data).await?;

        let token0 = parse_address_from_result(&token0_result)?;
        let token1 = parse_address_from_result(&token1_result)?;

        let pool = EnrichedPool {
            base: PoolState {
                address: format!("0x{}", hex::encode(pool_addr)),
                token0: format!("0x{}", hex::encode(token0)),
                token1: format!("0x{}", hex::encode(token1)),
                reserve0,
                reserve1,
                fee_tier: 30,
                liquidity: reserve0.saturating_add(reserve1),
                sqrt_price_x96: 0,
                current_tick: None,
                last_update: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            },
            dex_type: DexType::UniswapV2,
            factory_type: FactoryType::UniswapV2,
            token_pair: TokenPair::new_hex(
                &format!("0x{}", hex::encode(token0)),
                &format!("0x{}", hex::encode(token1)),
            ),
            sqrt_price_x96: None,
            current_tick: None,
            executable_liquidity: reserve0.saturating_add(reserve1),
            active_liquidity: reserve0.saturating_add(reserve1),
            recent_swaps: VecDeque::new(),
            recent_volume: 0,
            price_movement_bps: 0,
            last_swap_block: 0,
            last_update_block: 0,
            freshness: PoolFreshness::Fresh,
        };

        Ok(Some(pool))
    }

    pub async fn sync_uniswap_v3_pool(&self, pool_addr: [u8; 20]) -> Result<Option<EnrichedPool>> {
        let pool_hex = hex::encode(pool_addr);

        let slot0_data = "0x3850c7bd";
        let slot0_result = self.rpc_client.call(&pool_hex, slot0_data).await?;
        let (sqrt_price_x96, tick) = UniswapV3PoolParser::parse_slot0(&slot0_result)?;

        let liquidity_data = "0x1a686502";
        let liquidity_result = self.rpc_client.call(&pool_hex, liquidity_data).await?;
        let liquidity = UniswapV3PoolParser::parse_liquidity(&liquidity_result)?;

        let token0_data = "0x0dfe1681";
        let token1_data = "0xd21220a7";
        let fee_data = "0xddca3f43";

        let token0_result = self.rpc_client.call(&pool_hex, token0_data).await?;
        let token1_result = self.rpc_client.call(&pool_hex, token1_data).await?;
        let fee_result = self.rpc_client.call(&pool_hex, fee_data).await?;

        let token0 = parse_address_from_result(&token0_result)?;
        let token1 = parse_address_from_result(&token1_result)?;
        let fee = parse_u24_from_result(&fee_result)?;

        let pool = EnrichedPool {
            base: PoolState {
                address: format!("0x{}", hex::encode(pool_addr)),
                token0: format!("0x{}", hex::encode(token0)),
                token1: format!("0x{}", hex::encode(token1)),
                reserve0: 0,
                reserve1: 0,
                fee_tier: fee,
                liquidity,
                sqrt_price_x96,
                current_tick: Some(tick),
                last_update: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            },
            dex_type: DexType::UniswapV3,
            factory_type: FactoryType::UniswapV3,
            token_pair: TokenPair::new_hex(
                &format!("0x{}", hex::encode(token0)),
                &format!("0x{}", hex::encode(token1)),
            ),
            sqrt_price_x96: Some(sqrt_price_x96),
            current_tick: Some(tick),
            executable_liquidity: liquidity,
            active_liquidity: liquidity,
            recent_swaps: VecDeque::new(),
            recent_volume: 0,
            price_movement_bps: 0,
            last_swap_block: 0,
            last_update_block: 0,
            freshness: PoolFreshness::Fresh,
        };

        Ok(Some(pool))
    }

    pub async fn run_discovery(&self) -> Result<usize> {
        let mut total_discovered = 0;

        for factory_type in &self.config.factories {
            if let Some(factory_addr) = factory_type.address(self.config.chain_id) {
                match factory_type {
                    FactoryType::UniswapV2 => {
                        let pools = self.discover_uniswap_v2_pools(factory_addr).await?;
                        total_discovered += pools.len();

                        for pool_addr in pools {
                            if let Ok(Some(pool)) = self.sync_uniswap_v2_pool(pool_addr).await {
                                let addr_str = format!("0x{}", hex::encode(pool_addr));
                                self.registry.update_pool(addr_str, pool);
                            }
                        }
                    }
                    FactoryType::UniswapV3 => {
                        tracing::warn!("UniswapV3 pool discovery requires event scanning - not fully implemented");
                    }
                    _ => {
                        tracing::debug!("Factory type {:?} not yet implemented", factory_type);
                    }
                }
            }
        }

        tracing::info!("Pool discovery complete: {} total pools", total_discovered);
        Ok(total_discovered)
    }
}

fn parse_address_from_result(result: &str) -> Result<[u8; 20]> {
    let hex = result.trim_start_matches("0x");
    let bytes = hex::decode(hex)
        .map_err(|e| ArbitrageError::Encoding(format!("Failed to decode address: {}", e)))?;

    if bytes.len() < 32 {
        return Err(ArbitrageError::Encoding(
            "Invalid address data length".to_string(),
        ));
    }

    let mut addr = [0u8; 20];
    addr.copy_from_slice(&bytes[12..32]);

    Ok(addr)
}

fn parse_u24_from_result(result: &str) -> Result<u32> {
    let hex = result.trim_start_matches("0x");
    let bytes = hex::decode(hex)
        .map_err(|e| ArbitrageError::Encoding(format!("Failed to decode u24: {}", e)))?;

    if bytes.len() < 32 {
        return Err(ArbitrageError::Encoding(
            "Invalid u24 data length".to_string(),
        ));
    }

    let fee = u32::from_be_bytes([0, bytes[29], bytes[30], bytes[31]]);
    Ok(fee)
}

fn encode_uint256(value: u128) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    let value_bytes = value.to_be_bytes();
    bytes[32 - value_bytes.len()..].copy_from_slice(&value_bytes);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_pair_ordering() {
        let pair1 = TokenPair::new_hex(
            "0x0000000000000000000000000000000000000001",
            "0x0000000000000000000000000000000000000002",
        );
        let pair2 = TokenPair::new_hex(
            "0x0000000000000000000000000000000000000002",
            "0x0000000000000000000000000000000000000001",
        );

        assert_eq!(pair1.token0, "0x0000000000000000000000000000000000000001");
        assert_eq!(pair1.token1, "0x0000000000000000000000000000000000000002");
        assert_eq!(pair1, pair2);
    }

    #[test]
    fn test_pool_freshness() {
        let mut pool = EnrichedPool {
            freshness: PoolFreshness::Fresh,
            ..Default::default()
        };

        assert!(pool.is_fresh());

        pool.freshness = PoolFreshness::Stale;
        assert!(!pool.is_fresh());

        pool.executable_liquidity = 0;
        assert!(!pool.is_usable());
    }

    #[test]
    fn test_factory_addresses() {
        assert!(FactoryType::UniswapV2.address(42161).is_some());
        assert!(FactoryType::UniswapV3.address(42161).is_some());
        assert!(FactoryType::UniswapV2.address(99999).is_none());
    }
}
