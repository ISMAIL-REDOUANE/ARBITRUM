//! # WebSocket Module
//!
//! Binance WebSocket listener for aggTrade stream.

use crate::engine::SharedState;
use crate::error::{ArbitrageError, Result};
use crate::types::PriceEvent;

use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct BinanceListener {
    state: Arc<SharedState>,
    ws_url: String,
    symbols: Vec<String>,
    reconnect_delay: Duration,
    max_attempts: usize,
}

impl BinanceListener {
    pub fn new(state: Arc<SharedState>) -> Self {
        let config = state.config.binance.clone();
        Self {
            state,
            ws_url: config.ws_url.clone(),
            symbols: config.symbols.clone(),
            reconnect_delay: Duration::from_millis(config.reconnect_delay_ms),
            max_attempts: config.max_reconnect_attempts,
        }
    }
    
    fn build_url(&self) -> String {
        let streams: Vec<String> = self.symbols
            .iter()
            .map(|s| format!("{}@aggTrade", s.to_lowercase()))
            .collect();
        
        format!("{}/?streams={}", self.ws_url, streams.join("/"))
    }
    
    pub async fn run(self) -> Result<()> {
        let mut attempts = 0;
        let url = self.build_url();
        
        loop {
            if self.state.is_shutdown() {
                tracing::info!("Binance listener shutting down");
                break;
            }
            
            tracing::info!("Connecting to Binance WebSocket: {}", url);
            
            match tokio_tungstenite::connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    tracing::info!("Connected to Binance WebSocket");
                    attempts = 0;
                    
                    if let Err(e) = self.handle_connection(ws_stream).await {
                        tracing::error!("Connection error: {:?}", e);
                    }
                }
                Err(e) => {
                    attempts += 1;
                    if attempts >= self.max_attempts {
                        return Err(ArbitrageError::WebSocket(format!(
                            "Max attempts ({}) exceeded: {}", 
                            self.max_attempts, e
                        )));
                    }
                    
                    tracing::warn!("Connection failed, retrying: {:?}", e);
                    tokio::time::sleep(self.reconnect_delay).await;
                }
            }
            
            let backoff = Duration::from_millis(
                self.reconnect_delay.as_millis() as u64 * (2u64.pow(attempts.min(5) as u32))
            );
            tokio::time::sleep(backoff).await;
        }
        
        Ok(())
    }
    
    async fn handle_connection<WS>(&self, mut ws_stream: WS) -> Result<()>
    where
        WS: futures_util::StreamExt<Item = std::result::Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
    {
        let mut last_ping = Instant::now();
        
        while !self.state.is_shutdown() {
            tokio::select! {
                msg = ws_stream.next() => {
                    match msg {
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                            if let Err(e) = self.process_message(&text).await {
                                tracing::warn!("Failed to process message: {:?}", e);
                            }
                            last_ping = Instant::now();
                        }
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Close(reason))) => {
                            tracing::info!("Connection closed: {:?}", reason);
                            break;
                        }
                        Some(Err(e)) => {
                            tracing::error!("WebSocket error: {:?}", e);
                            break;
                        }
                        _ => {}
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(30)) => {
                    if last_ping.elapsed() > Duration::from_secs(60) {
                        tracing::warn!("No message received for 60s, reconnecting");
                        break;
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    if self.state.is_shutdown() {
                        break;
                    }
                }
            }
        }
        
        Ok(())
    }
    
    async fn process_message(&self, text: &str) -> Result<()> {
        let start = Instant::now();
        
        let event: BinanceAggTrade = serde_json::from_str(text)
            .map_err(|e| ArbitrageError::JsonParse(e.to_string()))?;
        
        let price_event = PriceEvent::new(
            event.s.clone(),
            event.p.clone(),
            event.q.clone(),
            event.trade_time,
            event.m,
        );
        
        let latency_us = start.elapsed().as_micros() as u64;
        self.state.stats.record_latency(latency_us);
        self.state.stats.record_signal();
        
        if self.state.push_price_event(price_event).is_err() {
            tracing::warn!("Price event channel full, dropping event");
        }
        
        tracing::trace!("Processed {} in {}µs", event.s, latency_us);
        
        Ok(())
    }
}

pub fn spawn_binance_listener(state: Arc<SharedState>) -> Result<tokio::task::JoinHandle<()>> {
    let listener = BinanceListener::new(state);
    
    let handle = tokio::spawn(async move {
        if let Err(e) = listener.run().await {
            tracing::error!("Binance listener error: {:?}", e);
        }
    });
    
    Ok(handle)
}

#[derive(Debug, serde::Deserialize)]
struct BinanceAggTrade {
    #[serde(rename = "e")]
    e: String,
    
    #[serde(rename = "E")]
    event_time: u64,
    
    #[serde(rename = "s")]
    s: String,
    
    #[serde(rename = "a")]
    a: u64,
    
    #[serde(rename = "p")]
    p: String,
    
    #[serde(rename = "q")]
    q: String,
    
    #[serde(rename = "T")]
    trade_time: u64,
    
    #[serde(rename = "m")]
    m: bool,
}
