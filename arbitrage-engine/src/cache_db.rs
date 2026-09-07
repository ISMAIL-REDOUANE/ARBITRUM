//! # REVM CacheDB Module - Optimized
//!
//! In-memory EVM state database for local simulation.
//!
//! Optimizations:
//! - Stack-allocated fixed-size arrays for hot path
//! - Warm cache for DEX pool states
//! - Zero-allocation lookups via stack arrays

use crate::error::Result;
use std::collections::HashMap;

#[derive(Debug)]
pub struct RevmCacheDB {
    chain_id: u64,
    accounts: HashMap<[u8; 20], AccountData>,
    storage: HashMap<([u8; 20], [u8; 32]), [u8; 32]>,
    warm_pools: HashMap<[u8; 20], PoolState>,
}

#[derive(Clone, Debug)]
pub struct AccountData {
    pub balance: u128,
    pub nonce: u64,
    pub code_hash: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct PoolState {
    pub token0: [u8; 20],
    pub token1: [u8; 20],
    pub fee_tier: u32,
    pub liquidity: u128,
    pub sqrt_price: u128,
    pub tick: i32,
}

impl RevmCacheDB {
    pub fn new() -> Result<Self> {
        Ok(Self {
            chain_id: 1,
            accounts: HashMap::new(),
            storage: HashMap::new(),
            warm_pools: HashMap::new(),
        })
    }

    pub fn with_chain_id(chain_id: u64) -> Result<Self> {
        Ok(Self {
            chain_id,
            accounts: HashMap::new(),
            storage: HashMap::new(),
            warm_pools: HashMap::new(),
        })
    }

    pub fn seed_known_addresses(&mut self) -> Result<()> {
        let addresses = [
            hex::decode("af88d065e77c8cc2239327c5edb3a432268e5831").unwrap(),
            hex::decode("82aF49447D8a07e3bd95BD0d56f35241523fBab1").unwrap(),
            hex::decode("BA12222222228d8Ba445958a75a0704d566BF2C8").unwrap(),
            hex::decode("1F98431c8aD98523631AE4a59f267346ea31F984").unwrap(),
            hex::decode("E592427A0AEce92De3Edee1F18E0157C05861564").unwrap(),
        ];

        for addr in addresses {
            let mut key = [0u8; 20];
            key.copy_from_slice(&addr);
            self.accounts.insert(key, AccountData {
                balance: u128::MAX,
                nonce: 0,
                code_hash: [0u8; 32],
            });
        }

        tracing::debug!("Seeded 5 known addresses with MAX_BALANCE");
        Ok(())
    }

    pub fn set_balance(&mut self, address: [u8; 20], balance: u128) {
        if let Some(info) = self.accounts.get_mut(&address) {
            info.balance = balance;
        } else {
            self.accounts.insert(address, AccountData {
                balance,
                nonce: 0,
                code_hash: [0u8; 32],
            });
        }
    }

    pub fn set_storage(&mut self, address: [u8; 20], slot: [u8; 32], value: [u8; 32]) {
        self.storage.insert((address, slot), value);
    }

    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    pub fn contracts_len(&self) -> usize {
        self.accounts.len()
    }

    pub fn insert_contract(&mut self, address: [u8; 20], bytecode: Vec<u8>) {
        use tiny_keccak::{Keccak, Hasher};

        let mut hasher = Keccak::v256();
        hasher.update(&bytecode);
        let mut hash = [0u8; 32];
        hasher.finalize(&mut hash);

        if let Some(info) = self.accounts.get_mut(&address) {
            info.code_hash = hash;
        } else {
            self.accounts.insert(address, AccountData {
                balance: 0,
                nonce: 0,
                code_hash: hash,
            });
        }
    }

    /// Warm a DEX pool state for fast simulation
    pub fn warm_pool(&mut self, pool_address: [u8; 20], state: PoolState) {
        self.warm_pools.insert(pool_address, state);
    }

    /// Get warm pool state - O(1) lookup, stack-allocated return
    #[inline(always)]
    pub fn get_warm_pool(&self, pool_address: [u8; 20]) -> Option<PoolState> {
        self.warm_pools.get(&pool_address).cloned()
    }

    /// Fast balance check using stack array
    #[inline(always)]
    pub fn get_balance_stack(&self, address: &[u8; 20]) -> Option<u128> {
        self.accounts.get(address).map(|a| a.balance)
    }
}

impl Default for RevmCacheDB {
    fn default() -> Self {
        Self::new().expect("Failed to create default CacheDB")
    }
}

impl Clone for RevmCacheDB {
    fn clone(&self) -> Self {
        Self {
            chain_id: self.chain_id,
            accounts: self.accounts.clone(),
            storage: self.storage.clone(),
            warm_pools: self.warm_pools.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_db_storage() {
        let mut db = RevmCacheDB::new().unwrap();
        let address = [0u8; 20];
        let slot = [0u8; 32];
        let value = [1u8; 32];

        db.set_storage(address, slot, value);

        assert!(db.storage.contains_key(&(address, slot)));
    }

    #[test]
    fn test_cache_db_clone() {
        let mut db = RevmCacheDB::new().unwrap();
        let address = [0u8; 20];
        let slot = [0u8; 32];

        db.set_storage(address, slot, [1u8; 32]);

        let db2 = db.clone();
        assert!(db2.storage.contains_key(&(address, slot)));
    }

    #[test]
    fn test_warm_pool() {
        let mut db = RevmCacheDB::new().unwrap();
        let pool = [1u8; 20];
        let state = PoolState {
            token0: [2u8; 20],
            token1: [3u8; 20],
            fee_tier: 3000,
            liquidity: 1_000_000_000_000_000_000,
            sqrt_price: 79228162514264337593543950336u128,
            tick: 0,
        };

        db.warm_pool(pool, state.clone());
        let retrieved = db.get_warm_pool(pool);

        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().fee_tier, 3000);
    }

    #[test]
    fn test_balance_stack() {
        let mut db = RevmCacheDB::new().unwrap();
        let address = [0u8; 20];
        db.set_balance(address, 1_000_000_000_000_000_000u128);

        let balance = db.get_balance_stack(&address);
        assert_eq!(balance, Some(1_000_000_000_000_000_000u128));
    }
}
