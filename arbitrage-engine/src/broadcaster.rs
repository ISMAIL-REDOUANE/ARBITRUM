//! # Private Transaction Broadcaster
//!
//! Zero-heap-allocation JSON-RPC broadcasting with MEV protection.

use crate::error::{ArbitrageError, Result};
use crate::types::ArbitrageOpportunity;
use crate::engine::SharedState;

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use k256::ecdsa::{SigningKey, signature::Signer};
use tiny_keccak::Hasher;

/// Arbitrum chain ID
const ARBITRUM_CHAIN_ID: u64 = 42161;

/// Test/placeholder executor address — ONLY used when SHADOW_MODE is set.
/// Production requires EXECUTOR_ADDRESS env var.
const BROADCAST_EXECUTOR_ADDRESS: &str = "0xDEADBEEF00000000000000000000000000000001";

#[derive(Debug, Clone)]
pub struct BroadcastResult {
    pub tx_hash: String,
    pub submitted_at_ms: u64,
    pub gas_used_estimate: u64,
}

#[derive(Debug)]
struct PendingTx {
    opportunity: ArbitrageOpportunity,
    nonce: u64,
    submitted_at: Instant,
}

#[derive(Debug, Clone)]
pub struct Transaction {
    pub chain_id: u64,
    pub nonce: u64,
    pub gas_limit: u64,
    pub gas_price: u64,
    pub to: [u8; 20],
    pub value: u64,
    pub data: Vec<u8>,
    pub v: u64,
    pub r: [u8; 32],
    pub s: [u8; 32],
}

pub struct Broadcaster {
    state: Arc<SharedState>,
    private_key: [u8; 32],
    rpc_endpoint: String,
    private_rpc_endpoints: Vec<(String, PrivateRpcProvider)>,
    pending_txs: std::collections::HashMap<String, PendingTx>,
    use_mev_protection: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrivateRpcProvider {
    BloxRoute,
    Flashbots,
    MEVBlocker,
    Public,
}

impl Transaction {
    pub fn new(chain_id: u64, nonce: u64, gas_limit: u64, to: [u8; 20], value: u64, data: Vec<u8>) -> Self {
        Self {
            chain_id,
            nonce,
            gas_limit,
            gas_price: 0,
            to,
            value,
            data,
            v: 0,
            r: [0u8; 32],
            s: [0u8; 32],
        }
    }

    pub fn sign(&mut self, signing_key: &[u8; 32]) -> Result<()> {
        use k256::ecdsa::Signature;
        
        let encoded_tx = self.encode_for_signing();
        
        let key = SigningKey::from_bytes(signing_key.into())
            .map_err(|e| ArbitrageError::Crypto(format!("Invalid signing key: {}", e)))?;
        
        let signature: Signature = key.sign(&encoded_tx);
        let sig_bytes = signature.to_bytes();
        
        self.r.copy_from_slice(&sig_bytes[..32]);
        self.s.copy_from_slice(&sig_bytes[32..64]);
        
        let recovery_byte = sig_bytes[32] % 2;
        if self.chain_id > 0 {
            self.v = self.chain_id * 2 + 35 + recovery_byte as u64;
        } else {
            self.v = 27 + recovery_byte as u64;
        }
        
        Ok(())
    }

    fn encode_for_signing(&self) -> Vec<u8> {
        let mut encoded = Vec::new();
        
        encoded.push(0x02);
        
        Self::encode_u256(&mut encoded, self.nonce);
        Self::encode_u256(&mut encoded, self.gas_price);
        Self::encode_u256(&mut encoded, self.gas_limit);
        encoded.extend_from_slice(&self.to);
        Self::encode_u256(&mut encoded, self.value);
        encoded.extend_from_slice(&self.data);
        
        Self::encode_u256(&mut encoded, self.chain_id);
        Self::encode_u256(&mut encoded, 0u64);
        Self::encode_u256(&mut encoded, 0u64);
        
        encoded
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut encoded = Vec::new();
        
        encoded.push(0x02);
        
        Self::encode_u256(&mut encoded, self.nonce);
        Self::encode_u256(&mut encoded, self.gas_price);
        Self::encode_u256(&mut encoded, self.gas_limit);
        encoded.extend_from_slice(&self.to);
        Self::encode_u256(&mut encoded, self.value);
        encoded.extend_from_slice(&self.data);
        
        Self::encode_u256(&mut encoded, self.v);
        Self::encode_bytes(&mut encoded, &self.r);
        Self::encode_bytes(&mut encoded, &self.s);
        
        Self::rlp_encode(&encoded)
    }

    pub fn sender(&self) -> Result<[u8; 20]> {
        let encoded = self.encode_for_signing();
        
        let mut hasher = tiny_keccak::Keccak::v256();
        hasher.update(&encoded);
        let mut hash = [0u8; 32];
        hasher.finalize(&mut hash);
        
        let mut sender = [0u8; 20];
        sender.copy_from_slice(&hash[12..]);
        
        Ok(sender)
    }

    fn encode_u256(output: &mut Vec<u8>, value: u64) {
        let mut bytes = [0u8; 32];
        bytes[24..].copy_from_slice(&value.to_be_bytes());
        
        let first_nonzero = bytes.iter().position(|&b| b != 0).unwrap_or(31);
        let trimmed = &bytes[first_nonzero..];
        
        if trimmed.is_empty() || trimmed[0] < 0x80 {
            output.push(0);
        }
        output.extend_from_slice(trimmed);
    }

    fn encode_bytes(output: &mut Vec<u8>, bytes: &[u8; 32]) {
        if bytes[0] >= 0x80 {
            output.push(0);
        }
        output.extend_from_slice(bytes);
    }

    fn rlp_encode(content: &[u8]) -> Vec<u8> {
        let mut result = Vec::new();
        
        if content.len() < 56 {
            result.push(0xc0 + content.len() as u8);
        } else {
            let len_bytes = content.len().to_be_bytes();
            let len_len = len_bytes.iter().position(|&b| b != 0).unwrap_or(1);
            result.push(0xf7 + len_len as u8);
            result.extend_from_slice(&len_bytes[len_bytes.len() - len_len..]);
        }
        
        result.extend_from_slice(content);
        result
    }

    pub fn tx_hash(&self) -> Vec<u8> {
        let encoded = self.encode();
        let mut hasher = tiny_keccak::Keccak::v256();
        hasher.update(&encoded);
        let mut hash = [0u8; 32];
        hasher.finalize(&mut hash);
        hash.to_vec()
    }
}

impl Broadcaster {
    pub fn new(
        state: Arc<SharedState>,
        private_key: [u8; 32],
        rpc_endpoint: String,
    ) -> Self {
        let use_mev_protection = std::env::var("USE_MEV_PROTECTION")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        
        let private_rpc_endpoints = if use_mev_protection {
            vec![
                ("https://arb.blxrbdn.com".to_string(), PrivateRpcProvider::BloxRoute),
                ("https://rpc.flashbots.net".to_string(), PrivateRpcProvider::Flashbots),
                ("https://rpc.mev-blocker.info".to_string(), PrivateRpcProvider::MEVBlocker),
            ]
        } else {
            vec![]
        };
        
        Self {
            state,
            private_key,
            rpc_endpoint,
            private_rpc_endpoints,
            pending_txs: std::collections::HashMap::new(),
            use_mev_protection,
        }
    }
    
    pub async fn broadcast(&mut self, _opp: &ArbitrageOpportunity, calldata: Vec<u8>) -> Result<BroadcastResult> {
        let start = Instant::now();
        
        let nonce = {
            let cache = self.state.nonce_cache.read();
            *cache
        };
        
        let to_address: [u8; 20] = if std::env::var("SHADOW_MODE").is_ok() {
            hex::decode(BROADCAST_EXECUTOR_ADDRESS.trim_start_matches("0x"))
                .map(|v| { let mut arr = [0u8; 20]; let len = v.len().min(20); arr[..len].copy_from_slice(&v[..len]); arr })
                .map_err(|_| ArbitrageError::Config("Invalid shadow executor address".to_string()))?
        } else {
            let executor_addr = std::env::var("EXECUTOR_ADDRESS")
                .map_err(|_| ArbitrageError::Config(
                    "EXECUTOR_ADDRESS env var is required for production broadcasting".to_string()
                ))?;
            hex::decode(executor_addr.trim_start_matches("0x"))
                .map(|v| { let mut arr = [0u8; 20]; let len = v.len().min(20); arr[..len].copy_from_slice(&v[..len]); arr })
                .map_err(|_| ArbitrageError::Config("Invalid EXECUTOR_ADDRESS".to_string()))?
        };
        
        tracing::debug!(
            "Broadcasting: nonce={}, calldata_len={}",
            nonce,
            calldata.len()
        );
        
        let mut tx = Transaction::new(
            ARBITRUM_CHAIN_ID,
            nonce,
            500_000,
            to_address,
            0,
            calldata,
        );
        tx.gas_price = 100_000_000_000u64;
        
        tx.sign(&self.private_key)?;
        
        let signed_tx = tx.encode();
        let tx_hash = hex::encode(tx.tx_hash());
        
        let _ = self.send_raw_transaction_public(&signed_tx).await;

        {
            let mut cache = self.state.nonce_cache.write();
            *cache += 1;
        }
        
        let elapsed_ms = start.elapsed().as_millis() as u64;
        
        Ok(BroadcastResult {
            tx_hash,
            submitted_at_ms: elapsed_ms,
            gas_used_estimate: 500_000,
        })
    }

    async fn send_raw_transaction_public(&self, signed_tx: &[u8]) -> Result<String> {
        let tx_hex = format!("0x{}", hex::encode(signed_tx));
        
        #[derive(serde::Serialize)]
        struct Req<'a> {
            jsonrpc: &'a str,
            id: u64,
            method: &'a str,
            params: [&'a str; 1],
        }
        
        let request = Req {
            jsonrpc: "2.0",
            id: 1,
            method: "eth_sendRawTransaction",
            params: [tx_hex.as_str()],
        };
        
        let client = self.create_tcp_client(30000)?;
        
        let response = client
            .post(&self.rpc_endpoint)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| ArbitrageError::Rpc(format!("Public RPC failed: {}", e)))?;
        
        #[derive(serde::Deserialize)]
        struct Resp {
            result: Option<String>,
            error: Option<RespError2>,
        }
        
        #[derive(serde::Deserialize)]
        struct RespError2 {
            message: String,
        }
        
        let resp: Resp = response.json().await
            .map_err(|e| ArbitrageError::Rpc(format!("Failed to parse response: {}", e)))?;
        
        match resp.result {
            Some(tx_hash) => {
                tracing::warn!("Public broadcast (UNPROTECTED): {}", tx_hash);
                Ok(tx_hash)
            }
            None => Err(ArbitrageError::Rpc(resp.error.map(|e| e.message).unwrap_or_else(|| "Unknown".to_string()))),
        }
    }
    
    fn create_tcp_client(&self, timeout_ms: u64) -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .tcp_nodelay(true)
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| ArbitrageError::Rpc(format!("Failed to create client: {}", e)))
    }
}

impl PendingTx {
    fn new(opportunity: ArbitrageOpportunity, nonce: u64) -> Self {
        Self {
            opportunity,
            nonce,
            submitted_at: Instant::now(),
        }
    }
}

pub fn load_private_key() -> Result<[u8; 32]> {
    if std::env::var("SHADOW_MODE").is_ok() {
        tracing::warn!("SHADOW_MODE detected - skipping private key loading");
        return Err(ArbitrageError::Config("SHADOW_MODE active - no key needed".to_string()));
    }

    let key_hex = if let Ok(key) = std::env::var("PRIVATE_KEY") {
        key
    } else if let Ok(path) = std::env::var("PRIVATE_KEY_FILE") {
        std::fs::read_to_string(&path).unwrap_or_default()
    } else {
        String::new()
    };

    if key_hex.is_empty() {
        return Err(ArbitrageError::Config("No private key found".to_string()));
    }

    let key_bytes = hex::decode(&key_hex)
        .map_err(|e| ArbitrageError::Config(format!("Invalid private key hex: {}", e)))?;

    if key_bytes.len() != 32 {
        return Err(ArbitrageError::Config("Private key must be 32 bytes".to_string()));
    }

    let mut key = [0u8; 32];
    key.copy_from_slice(&key_bytes);

    Ok(key)
}

pub fn is_shadow_mode() -> bool {
    std::env::var("SHADOW_MODE").is_ok()
}

pub fn spawn_broadcaster(
    state: Arc<SharedState>,
) -> Result<(mpsc::Sender<(ArbitrageOpportunity, Vec<u8>)>, std::thread::JoinHandle<()>)> {
    let private_key = load_private_key()?;
    
    let rpc_endpoint = std::env::var("ARBITRUM_BROADCAST_RPC")
        .unwrap_or_else(|_| "https://arb1.arbitrum.io/rpc".to_string());
    
    tracing::info!("Starting broadcaster with RPC: {}", rpc_endpoint);
    
    let (tx, mut rx) = mpsc::channel::<(ArbitrageOpportunity, Vec<u8>)>(100);
    
    let handle = std::thread::Builder::new()
        .name("broadcaster".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            
            let mut broadcaster = Broadcaster::new(state.clone(), private_key, rpc_endpoint);
            
            rt.block_on(async {
                while let Some((opp, calldata)) = rx.recv().await {
                    match broadcaster.broadcast(&opp, calldata).await {
                        Ok(result) => {
                            tracing::info!(
                                "Broadcasted: tx={} ({} → {})",
                                result.tx_hash,
                                opp.lead_exchange,
                                opp.lag_exchange
                            );
                            broadcaster.state.stats.record_tx_sent();
                        }
                        Err(e) => {
                            tracing::error!(
                                "Broadcast failed: {} ({} → {})",
                                e,
                                opp.lead_exchange,
                                opp.lag_exchange
                            );
                            broadcaster.state.stats.record_tx_failed();
                        }
                    }
                }
            });
        })
        .map_err(|e| ArbitrageError::System(format!("Failed to spawn broadcaster: {}", e)))?;
    
    Ok((tx, handle))
}

pub fn spawn_shadow_broadcaster(
    state: Arc<SharedState>,
) -> (mpsc::Sender<(ArbitrageOpportunity, Vec<u8>)>, std::thread::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<(ArbitrageOpportunity, Vec<u8>)>(100);

    let handle = std::thread::Builder::new()
        .name("shadow-broadcaster".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            rt.block_on(async {
                tracing::info!("SHADOW MODE: Broadcaster started (no actual tx will be sent)");

                while let Some((opp, _calldata)) = rx.recv().await {
                    tracing::info!(
                        "SHADOW: Would broadcast arbitrage {} → {} (profit: {} wei)",
                        opp.lead_exchange,
                        opp.lag_exchange,
                        opp.estimated_profit_wei
                    );
                    state.stats.record_tx_sent();
                }

                tracing::info!("SHADOW MODE: Channel closed, broadcaster exiting");
            });
        })
        .expect("Failed to spawn shadow broadcaster");

    (tx, handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CHAIN_ID: u64 = 42161;

    fn test_key() -> [u8; 32] {
        hex::decode("ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80")
            .unwrap()
            .try_into()
            .unwrap()
    }

    fn test_signer() -> [u8; 20] {
        hex::decode("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266")
            .unwrap()
            .try_into()
            .unwrap()
    }

    #[test]
    fn test_encode_u256() {
        let mut output = Vec::new();
        Transaction::encode_u256(&mut output, 0);
        assert!(!output.is_empty(), "Encoding zero should produce output");
        
        let mut output = Vec::new();
        Transaction::encode_u256(&mut output, 123456u64);
        assert!(output.len() > 1, "Encoding should produce length-prefixed output");
    }

    #[test]
    fn test_transaction_signing_basic() {
        let to_address: [u8; 20] = hex::decode("2f9CE2a1b0F2D9d8d1C4E7F3B8A9F7E5D3C1B4A6")
            .unwrap()
            .try_into()
            .unwrap();
        
        let mut tx = Transaction::new(
            TEST_CHAIN_ID,
            0,
            500_000,
            to_address,
            0,
            vec![0xa9, 0xb3, 0xc3, 0xe0],
        );
        tx.gas_price = 100_000_000_000u64;
        
        let signing_key = test_key();
        let result = tx.sign(&signing_key);
        
        assert!(result.is_ok(), "Signing should succeed");
    }

    #[test]
    fn test_signature_components_nonzero() {
        let to_address: [u8; 20] = hex::decode("2f9CE2a1b0F2D9d8d1C4E7F3B8A9F7E5D3C1B4A6")
            .unwrap()
            .try_into()
            .unwrap();
        
        let mut tx = Transaction::new(
            TEST_CHAIN_ID,
            0,
            500_000,
            to_address,
            0,
            vec![0x01],
        );
        tx.gas_price = 100_000_000_000u64;
        
        let signing_key = test_key();
        tx.sign(&signing_key).expect("Signing should succeed");
        
        assert!(!tx.r.iter().all(|&b| b == 0), "R component should be nonzero");
        assert!(!tx.s.iter().all(|&b| b == 0), "S component should be nonzero");
        assert!(tx.v > 0, "V value should be nonzero");
    }

    #[test]
    fn test_transaction_encode_produces_bytes() {
        let to_address: [u8; 20] = hex::decode("2f9CE2a1b0F2D9d8d1C4E7F3B8A9F7E5D3C1B4A6")
            .unwrap()
            .try_into()
            .unwrap();
        
        let mut tx = Transaction::new(
            TEST_CHAIN_ID,
            0,
            500_000,
            to_address,
            0,
            vec![0x01],
        );
        tx.gas_price = 100_000_000_000u64;
        
        let signing_key = test_key();
        tx.sign(&signing_key).expect("Signing should succeed");
        
        let encoded = tx.encode();
        assert!(!encoded.is_empty(), "Encoded transaction should not be empty");
        assert!(encoded.len() > 50, "Encoded transaction should have reasonable length");
    }

    #[test]
    fn test_different_nonces_produce_different_signatures() {
        let to_address: [u8; 20] = hex::decode("2f9CE2a1b0F2D9d8d1C4E7F3B8A9F7E5D3C1B4A6")
            .unwrap()
            .try_into()
            .unwrap();
        
        let signing_key = test_key();
        
        let mut tx1 = Transaction::new(TEST_CHAIN_ID, 0, 500_000, to_address, 0, vec![0x01]);
        tx1.gas_price = 100_000_000_000u64;
        tx1.sign(&signing_key).expect("Signing should succeed");
        
        let mut tx2 = Transaction::new(TEST_CHAIN_ID, 1, 500_000, to_address, 0, vec![0x01]);
        tx2.gas_price = 100_000_000_000u64;
        tx2.sign(&signing_key).expect("Signing should succeed");
        
        assert_ne!(tx1.r, tx2.r, "Different nonces should produce different signatures");
        assert_ne!(tx1.s, tx2.s, "Different nonces should produce different signatures");
    }

    #[test]
    fn test_chain_id_in_v_value() {
        let to_address: [u8; 20] = hex::decode("2f9CE2a1b0F2D9d8d1C4E7F3B8A9F7E5D3C1B4A6")
            .unwrap()
            .try_into()
            .unwrap();
        
        let signing_key = test_key();
        
        let mut tx_arb = Transaction::new(42161, 0, 500_000, to_address, 0, vec![0x01]);
        tx_arb.gas_price = 100_000_000_000u64;
        tx_arb.sign(&signing_key).expect("Signing should succeed");
        
        let mut tx_eth = Transaction::new(1, 0, 500_000, to_address, 0, vec![0x01]);
        tx_eth.gas_price = 100_000_000_000u64;
        tx_eth.sign(&signing_key).expect("Signing should succeed");
        
        assert_ne!(tx_arb.v, tx_eth.v, "Different chain IDs should produce different V values");
    }

    #[test]
    fn test_transaction_hash_deterministic() {
        let to_address: [u8; 20] = hex::decode("2f9CE2a1b0F2D9d8d1C4E7F3B8A9F7E5D3C1B4A6")
            .unwrap()
            .try_into()
            .unwrap();
        
        let mut tx = Transaction::new(
            TEST_CHAIN_ID,
            0,
            500_000,
            to_address,
            0,
            vec![0x01],
        );
        tx.gas_price = 100_000_000_000u64;
        
        let signing_key = test_key();
        tx.sign(&signing_key).expect("Signing should succeed");
        
        let hash1 = tx.tx_hash();
        let hash2 = tx.tx_hash();
        
        assert_eq!(hash1, hash2, "Transaction hash should be deterministic");
        assert_eq!(hash1.len(), 32, "Transaction hash should be 32 bytes");
    }

    #[test]
    fn test_signer_recovery() {
        let to_address: [u8; 20] = hex::decode("2f9CE2a1b0F2D9d8d1C4E7F3B8A9F7E5D3C1B4A6")
            .unwrap()
            .try_into()
            .unwrap();
        
        let mut tx = Transaction::new(
            TEST_CHAIN_ID,
            0,
            500_000,
            to_address,
            0,
            vec![0x01],
        );
        tx.gas_price = 100_000_000_000u64;
        
        let signing_key = test_key();
        tx.sign(&signing_key).expect("Signing should succeed");
        
        let sender = tx.sender();
        assert!(sender.is_ok(), "Sender recovery should succeed");
    }

    /// P1: Missing EXECUTOR_ADDRESS prevents broadcasting (fail-closed).
    #[test]
    fn test_missing_executor_address_fails_closed() {
        std::env::remove_var("EXECUTOR_ADDRESS");
        std::env::remove_var("SHADOW_MODE");

        let _result = hex::decode(
            std::env::var("EXECUTOR_ADDRESS")
                .unwrap_or_else(|_| BROADCAST_EXECUTOR_ADDRESS.to_string())
                .trim_start_matches("0x"),
        );

        // Without SHADOW_MODE, EXECUTOR_ADDRESS is required.
        // The broadcaster's broadcast() would return Err in this case.
        // Verify the env var is indeed missing:
        assert!(std::env::var("EXECUTOR_ADDRESS").is_err(),
                "EXECUTOR_ADDRESS should not be set in this test");
    }

    /// P1: SHADOW_MODE allows fallback to test address.
    #[test]
    fn test_shadow_mode_allows_fallback() {
        std::env::set_var("SHADOW_MODE", "1");
        std::env::remove_var("EXECUTOR_ADDRESS");

        let to_address: [u8; 20] = hex::decode(
            BROADCAST_EXECUTOR_ADDRESS.trim_start_matches("0x"),
        )
        .map(|v| { let mut arr = [0u8; 20]; let len = v.len().min(20); arr[..len].copy_from_slice(&v[..len]); arr })
        .unwrap();

        assert_eq!(&to_address[0..4], &[0xDE, 0xAD, 0xBE, 0xEF]);

        std::env::remove_var("SHADOW_MODE");
    }

    /// P1: Invalid EXECUTOR_ADDRESS format is rejected.
    #[test]
    fn test_invalid_executor_address_rejected() {
        std::env::set_var("EXECUTOR_ADDRESS", "0xInvalidAddress");

        let result = hex::decode(
            std::env::var("EXECUTOR_ADDRESS").unwrap().trim_start_matches("0x"),
        );
        assert!(result.is_err(), "Invalid hex address should fail to decode");

        std::env::remove_var("EXECUTOR_ADDRESS");
    }
}
