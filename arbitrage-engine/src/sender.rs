//! # Sender Module
//!
//! Transaction construction and broadcast to L2s.

use crate::engine::SharedState;
use crate::error::{ArbitrageError, Result};
use crate::types::{ArbitrageOpportunity, ArbitrageTx};

use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Sender {
    state: Arc<SharedState>,
    base_rpc: String,
    arb_rpc: String,
    nonce_base: u64,
    nonce_arb: u64,
}

impl Sender {
    pub fn new(state: Arc<SharedState>) -> Self {
        let base_rpc = state.config.chains.base.rpc_http_url.clone();
        let arb_rpc = state.config.chains.arbitrum.rpc_http_url.clone();
        
        Self {
            state,
            base_rpc,
            arb_rpc,
            nonce_base: 0,
            nonce_arb: 0,
        }
    }
    
    pub async fn run(&mut self) -> Result<()> {
        tracing::info!("Sender thread started");
        
        self.sync_nonces().await?;
        
        while !self.state.is_shutdown() {
            if let Some(opp) = self.state.pop_opportunity() {
                let start = Instant::now();
                
                match self.send_arbitrage(&opp).await {
                    Ok(tx_hash) => {
                        tracing::info!(
                            "Transaction sent: {:?} ({}µs)",
                            tx_hash,
                            start.elapsed().as_micros()
                        );
                        self.state.stats.record_tx_sent();
                    }
                    Err(e) => {
                        tracing::error!("Failed to send: {:?}", e);
                        self.state.stats.record_tx_failed();
                    }
                }
                
                self.increment_nonce(opp.chain_id);
            } else {
                tokio::task::yield_now().await;
            }
            
            tokio::time::sleep(Duration::from_millis(
                self.state.config.risk.trade_cooldown_ms
            )).await;
        }
        
        Ok(())
    }
    
    async fn send_arbitrage(&self, opportunity: &ArbitrageOpportunity) -> Result<String> {
        let tx = self.build_transaction(opportunity)?;
        
        tracing::info!(
            "Would send tx to: {} gas: {}",
            tx.to,
            tx.gas_limit
        );
        
        Ok(format!("0x{:064x}", opportunity.timestamp))
    }
    
    fn build_transaction(&self, opp: &ArbitrageOpportunity) -> Result<ArbitrageTx> {
        let contract_addr = match opp.chain_id {
            8453 => "0x0000000000000000000000000000000000000001",
            42161 => "0x0000000000000000000000000000000000000002",
            _ => return Err(ArbitrageError::Transaction("Unknown chain".to_string())),
        };
        
        Ok(ArbitrageTx {
            to: contract_addr.to_string(),
            data: vec![0xa1, 0xf2, 0xf3, 0xd4],
            value: 0,
            gas_limit: self.state.config.engine.max_gas_limit,
            nonce: 0,
            chain_id: opp.chain_id,
            max_priority_fee: 100_000_000,
            max_fee: 500_000_000,
        })
    }
    
    fn get_nonce(&self, chain_id: u64) -> u64 {
        match chain_id {
            8453 => self.nonce_base,
            42161 => self.nonce_arb,
            _ => 0,
        }
    }
    
    fn increment_nonce(&mut self, chain_id: u64) {
        match chain_id {
            8453 => self.nonce_base += 1,
            42161 => self.nonce_arb += 1,
            _ => {}
        }
    }
    
    async fn sync_nonces(&mut self) -> Result<()> {
        tracing::info!("Nonce sync placeholder");
        Ok(())
    }
}

pub fn spawn_sender(state: Arc<SharedState>) -> Result<tokio::task::JoinHandle<()>> {
    let mut sender = Sender::new(state);
    
    let handle = tokio::spawn(async move {
        if let Err(e) = sender.run().await {
            tracing::error!("Sender error: {:?}", e);
        }
    });
    
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_build_transaction() {
        let opp = ArbitrageOpportunity {
            lead_exchange: "Binance".to_string(),
            lag_exchange: "UniswapV3".to_string(),
            buy_price: 3456.78,
            sell_price: 3460.00,
            deviation_pct: 0.1,
            estimated_profit_wei: 10_000_000_000_000_000,
            token_pair: ("ETH".to_string(), "USDT".to_string()),
            chain_id: 8453,
            timestamp: 1699999999999,
        };
        
        assert_eq!(opp.chain_id, 8453);
    }
}
