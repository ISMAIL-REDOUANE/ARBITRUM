//! # Configuration Module
//!
//! Loads and manages configuration from `config.toml` or environment variables.

use serde::Deserialize;
use std::path::Path;
use anyhow::{Context, Result};

/// Main configuration structure
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Engine settings
    pub engine: EngineConfig,
    
    /// Binance WebSocket settings
    pub binance: BinanceConfig,
    
    /// Chain configurations (Base, Arbitrum, etc.)
    pub chains: ChainConfigs,
    
    /// DEX pool configurations
    pub dex: DexConfig,
    
    /// Risk parameters
    pub risk: RiskConfig,
}

/// Engine runtime configuration
#[derive(Debug, Clone, Deserialize)]
pub struct EngineConfig {
    /// Minimum profit threshold in wei
    pub min_profit_wei: u64,
    
    /// Maximum gas limit per arbitrage transaction
    pub max_gas_limit: u64,
    
    /// Ring buffer capacity for SPSC channels
    pub ring_buffer_size: usize,
    
    /// Simulation timeout in milliseconds
    pub simulation_timeout_ms: u64,
    
    /// Maximum concurrent simulations
    pub max_concurrent_simulations: usize,
}

/// Binance WebSocket configuration
#[derive(Debug, Clone, Deserialize)]
pub struct BinanceConfig {
    /// WebSocket endpoint URL
    pub ws_url: String,
    
    /// Trading symbols to monitor (e.g., ["ethusdt", "wbtcusdt"])
    pub symbols: Vec<String>,
    
    /// Reconnection delay in milliseconds
    pub reconnect_delay_ms: u64,
    
    /// Maximum reconnection attempts
    pub max_reconnect_attempts: usize,
}

/// Chain-specific configurations
#[derive(Debug, Clone, Deserialize)]
pub struct ChainConfigs {
    /// Base chain configuration
    pub base: ChainConfig,
    
    /// Arbitrum configuration
    pub arbitrum: ChainConfig,
}

/// Individual chain configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ChainConfig {
    /// Chain ID
    pub chain_id: u64,
    
    /// RPC WebSocket URL (for event syncing)
    pub rpc_ws_url: String,
    
    /// RPC HTTP URL (for queries - should NOT be used in hot path)
    pub rpc_http_url: String,
    
    /// Contract addresses
    pub addresses: ChainAddresses,
}

/// Chain-specific contract addresses
#[derive(Debug, Clone, Deserialize)]
pub struct ChainAddresses {
    /// Arbitrage engine contract address
    pub engine: String,
    
    /// Balancer Vault address
    pub balancer_vault: String,
    
    /// Uniswap V2 Router
    pub uniswap_v2_router: String,
    
    /// Uniswap V3 Router
    pub uniswap_v3_router: String,
    
    /// Aerodrome Router (Base)
    pub aerodrome_router: Option<String>,
    
    /// SushiSwap Router
    pub sushiswap_router: String,
    
    /// WETH address
    pub weth: String,
    
    /// USDC address
    pub usdc: String,
    
    /// USDT address
    pub usdt: String,
}

/// DEX pool configurations
#[derive(Debug, Clone, Deserialize)]
pub struct DexConfig {
    /// Uniswap V2 pools to monitor
    pub uniswap_v2_pools: Vec<PoolConfig>,
    
    /// Uniswap V3 pools to monitor
    pub uniswap_v3_pools: Vec<PoolConfig>,
    
    /// Aerodrome pools (Base)
    pub aerodrome_pools: Vec<PoolConfig>,
    
    /// SushiSwap pools
    pub sushiswap_pools: Vec<PoolConfig>,
}

/// Individual pool configuration
#[derive(Debug, Clone, Deserialize)]
pub struct PoolConfig {
    /// Pool contract address
    pub address: String,
    
    /// Token0 address
    pub token0: String,
    
    /// Token1 address
    pub token1: String,
    
    /// Fee tier (in basis points)
    pub fee_tier: u32,
    
    /// Pool type: "volatile" or "stable"
    pub pool_type: String,
}

/// Risk management configuration
#[derive(Debug, Clone, Deserialize)]
pub struct RiskConfig {
    /// Maximum position size per trade (in token units)
    pub max_position_size: u64,
    
    /// Maximum daily trade count
    pub max_daily_trades: u64,
    
    /// Maximum daily loss (stops trading if exceeded)
    pub max_daily_loss_wei: u64,
    
    /// Cooldown between trades in milliseconds
    pub trade_cooldown_ms: u64,
}

impl Config {
    /// Load configuration from file
    pub fn load() -> Result<Self> {
        Self::load_from_path("config.toml")
    }

    /// Load configuration from a specific path
    pub fn load_from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        
        if !path.exists() {
            tracing::warn!("Config file not found at {:?}, using defaults", path);
            return Ok(Self::default());
        }
        
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config from {:?}", path))?;
        
        toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config from {:?}", path))
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            engine: EngineConfig {
                min_profit_wei: 10_000_000_000_000_000, // 0.01 ETH
                max_gas_limit: 5_000_000,
                ring_buffer_size: 1024,
                simulation_timeout_ms: 500,
                max_concurrent_simulations: 4,
            },
            binance: BinanceConfig {
                ws_url: "wss://stream.binance.com:9443/ws".to_string(),
                symbols: vec!["ethusdt".to_string(), "wbtcusdt".to_string()],
                reconnect_delay_ms: 1000,
                max_reconnect_attempts: 10,
            },
            chains: ChainConfigs {
                base: ChainConfig {
                    chain_id: 8453,
                    rpc_ws_url: "".to_string(),
                    rpc_http_url: "".to_string(),
                    addresses: ChainAddresses {
                        engine: "".to_string(),
                        balancer_vault: "0xBA12222222228d8Ba445958a75a0704d566BF2C8".to_string(),
                        uniswap_v2_router: "".to_string(),
                        uniswap_v3_router: "0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45".to_string(),
                        aerodrome_router: Some("0x0B404b975d461A45E3Aa6b96809746b40A76239F".to_string()),
                        sushiswap_router: "".to_string(),
                        weth: "0x4200000000000000000000000000000000000006".to_string(),
                        usdc: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".to_string(),
                        usdt: "0xfde4C96c8593536E31F2271D6E3a2E7eB3a8A6fC".to_string(),
                    },
                },
                arbitrum: ChainConfig {
                    chain_id: 42161,
                    rpc_ws_url: "".to_string(),
                    rpc_http_url: "".to_string(),
                    addresses: ChainAddresses {
                        engine: "".to_string(),
                        balancer_vault: "0xBA12222222228d8Ba445958a75a0704d566BF2C8".to_string(),
                        uniswap_v2_router: "".to_string(),
                        uniswap_v3_router: "0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45".to_string(),
                        aerodrome_router: None,
                        sushiswap_router: "0x1b02dA8Cb0d097cB7d838B2E0cE7D3B1f5a6c9E8".to_string(),
                        weth: "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1".to_string(),
                        usdc: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831".to_string(),
                        usdt: "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9".to_string(),
                    },
                },
            },
            dex: DexConfig {
                uniswap_v2_pools: vec![],
                uniswap_v3_pools: vec![
                    PoolConfig {
                        address: "0x0d9f2c9d0949f5B2D43c70DC6B2f6293b7d98D2".to_string(),
                        token0: "0x514910771AF9Ca656af840dff83E8264EcF986CA".to_string(),
                        token1: "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1".to_string(),
                        fee_tier: 3000,
                        pool_type: "volatile".to_string(),
                    },
                    PoolConfig {
                        address: "0xA4755824613D25F5b43Fa7C5F78737124B5E8dbd".to_string(),
                        token0: "0x539bdE0d7Dbd336b79148AA742883198BBF10242".to_string(),
                        token1: "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1".to_string(),
                        fee_tier: 3000,
                        pool_type: "volatile".to_string(),
                    },
                    PoolConfig {
                        address: "0xC31F56C3e7A7278EC8a0f1639B1670EF9e09C8C8".to_string(),
                        token0: "0xB50721BCf8d664c02f4FF7688735E9F73d69FcF1".to_string(),
                        token1: "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1".to_string(),
                        fee_tier: 3000,
                        pool_type: "volatile".to_string(),
                    },
                    PoolConfig {
                        address: "0x1F72136979234956B0D9A6D2C8F6a7C2b9B4F7d".to_string(),
                        token0: "0xB50721BCf8d664c02f4FF7688735E9F73d69FcF1".to_string(),
                        token1: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831".to_string(),
                        fee_tier: 500,
                        pool_type: "volatile".to_string(),
                    },
                ],
                aerodrome_pools: vec![],
                sushiswap_pools: vec![],
            },
            risk: RiskConfig {
                max_position_size: 1_000_000_000,
                max_daily_trades: 1000,
                max_daily_loss_wei: 1_000_000_000_000_000_000, // 1 ETH
                trade_cooldown_ms: 100,
            },
        }
    }
}
