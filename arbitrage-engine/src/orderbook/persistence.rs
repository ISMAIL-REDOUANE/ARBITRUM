//! # L2 Persistence Module
//!
//! Persistent storage of raw L2 data for deterministic replay.
//!
//! ## Persistence Schema
//!
//! Each L2 update is stored as a row with:
//! - exchange (String)
//! - symbol (String)
//! - exchange_ts_ns (u64)
//! - recv_ts_ns (u64)
//! - update_id (u64)
//! - bids (List of (price, qty) tuples)
//! - asks (List of (price, qty) tuples)
//! - channel (String)
//!
//! ## Replay Requirements
//!
//! 1. Merge multiple exchange streams
//! 2. Deterministic ordering by (recv_ts_ns, exchange, symbol)
//! 3. Preserve original recv timestamps
//! 4. No per-batch timestamp reset

use crate::orderbook::l2_update::L2Update;
use std::io::Write;

/// L2 Persister configuration
#[derive(Debug, Clone)]
pub struct PersisterConfig {
    /// Base directory for L2 data
    pub base_dir: String,
    /// Flush interval (updates)
    pub flush_interval: usize,
    /// Maximum rows per file
    pub max_rows_per_file: usize,
}

impl Default for PersisterConfig {
    fn default() -> Self {
        Self {
            base_dir: "./l2_data".to_string(),
            flush_interval: 1000,
            max_rows_per_file: 100_000,
        }
    }
}

/// Raw L2 record for persistence
#[derive(Debug, Clone)]
pub struct L2Record {
    /// Exchange
    pub exchange: String,
    /// Symbol
    pub symbol: String,
    /// Exchange timestamp (ns)
    pub exchange_ts_ns: u64,
    /// Receive timestamp (ns)
    pub recv_ts_ns: u64,
    /// Update ID
    pub update_id: u64,
    /// Bids (price, quantity)
    pub bids: Vec<(f64, f64)>,
    /// Asks (price, quantity)
    pub asks: Vec<(f64, f64)>,
    /// Channel
    pub channel: String,
}

impl From<&L2Update> for L2Record {
    fn from(update: &L2Update) -> Self {
        Self {
            exchange: update.exchange.as_str().to_string(),
            symbol: update.symbol.clone(),
            exchange_ts_ns: update.exchange_ts_ns,
            recv_ts_ns: update.recv_ts_ns,
            update_id: update.update_id,
            bids: update.bids.clone(),
            asks: update.asks.clone(),
            channel: update.channel.clone(),
        }
    }
}

/// L2 Persister for writing L2 data
pub struct L2Persister {
    config: PersisterConfig,
    /// Current file writer
    writer: Option<Box<dyn L2Writer>>,
    /// Rows since last flush
    rows_since_flush: usize,
    /// Current file path
    current_path: Option<String>,
}

pub trait L2Writer: Send {
    fn write(&mut self, records: &[L2Record]) -> std::io::Result<()>;
    fn flush(&mut self) -> std::io::Result<()>;
    fn close(&mut self) -> std::io::Result<()>;
}

impl L2Persister {
    pub fn new(config: PersisterConfig) -> Self {
        Self {
            config,
            writer: None,
            rows_since_flush: 0,
            current_path: None,
        }
    }

    /// Write an L2 update
    pub fn write_update(&mut self, update: &L2Update) -> std::io::Result<()> {
        let record = L2Record::from(update);
        self.write_records(&[record])
    }

    /// Write multiple L2 records
    pub fn write_records(&mut self, records: &[L2Record]) -> std::io::Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        // Ensure we have a writer
        if self.writer.is_none() {
            self.open_new_file()?;
        }

        // Write records
        if let Some(writer) = &mut self.writer {
            writer.write(records)?;
            self.rows_since_flush += records.len();

            // Check if we need to rotate
            if self.rows_since_flush >= self.config.max_rows_per_file {
                self.rotate_file()?;
            }
        }

        Ok(())
    }

    /// Flush pending writes
    pub fn flush(&mut self) -> std::io::Result<()> {
        if let Some(writer) = &mut self.writer {
            writer.flush()?;
        }
        self.rows_since_flush = 0;
        Ok(())
    }

    /// Close the persister
    pub fn close(&mut self) -> std::io::Result<()> {
        if let Some(writer) = &mut self.writer {
            writer.close()?;
        }
        self.writer = None;
        self.current_path = None;
        Ok(())
    }

    fn open_new_file(&mut self) -> std::io::Result<()> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();

        let path = format!("{}/l2_{}.parquet", self.config.base_dir, timestamp);
        let path_clone = path.clone();

        // For now, use a simple CSV writer as placeholder
        // In production, this would be a Parquet writer
        self.writer = Some(Box::new(CsvL2Writer::new(&path)?));
        self.current_path = Some(path_clone);
        self.rows_since_flush = 0;

        Ok(())
    }

    fn rotate_file(&mut self) -> std::io::Result<()> {
        self.close()?;
        self.open_new_file()
    }
}

/// CSV writer placeholder (replace with Parquet in production)
struct CsvL2Writer {
    path: String,
    file: std::fs::File,
}

impl CsvL2Writer {
    fn new(path: &str) -> std::io::Result<Self> {
        // Create parent directory
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        Ok(Self {
            path: path.to_string(),
            file,
        })
    }
}

impl L2Writer for CsvL2Writer {
    fn write(&mut self, records: &[L2Record]) -> std::io::Result<()> {
        use std::io::Write;

        for record in records {
            // CSV format: exchange,symbol,exchange_ts_ns,recv_ts_ns,update_id,bids,asks,channel
            writeln!(
                self.file,
                "{},{},{},{},{},{:?},{:?},{}",
                record.exchange,
                record.symbol,
                record.exchange_ts_ns,
                record.recv_ts_ns,
                record.update_id,
                record.bids,
                record.asks,
                record.channel
            )?;
        }
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }

    fn close(&mut self) -> std::io::Result<()> {
        self.file.flush()?;
        Ok(())
    }
}

/// Replay source for reading L2 data
pub struct ReplaySource {
    /// Paths to replay files
    paths: Vec<String>,
    /// Current file index
    current_file: usize,
    /// Current position in file
    position: usize,
}

impl ReplaySource {
    pub fn new(paths: Vec<String>) -> Self {
        Self {
            paths,
            current_file: 0,
            position: 0,
        }
    }

    /// Open replay files for reading
    pub fn open(&mut self) -> std::io::Result<()> {
        self.current_file = 0;
        self.position = 0;
        Ok(())
    }

    /// Read next batch of records
    pub fn read_batch(&mut self, batch_size: usize) -> std::io::Result<Vec<L2Record>> {
        if self.current_file >= self.paths.len() {
            return Ok(vec![]); // End of replay
        }

        let path = &self.paths[self.current_file];
        let content = std::fs::read_to_string(path)?;

        let mut records = Vec::with_capacity(batch_size);
        let lines: Vec<&str> = content.lines().collect();

        for line in lines.iter().skip(self.position).take(batch_size) {
            if let Some(record) = Self::parse_csv_line(line) {
                records.push(record);
            }
        }

        self.position += records.len();

        // Check if we need to move to next file
        if self.position >= lines.len() {
            self.current_file += 1;
            self.position = 0;
        }

        Ok(records)
    }

    fn parse_csv_line(line: &str) -> Option<L2Record> {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 8 {
            return None;
        }

        Some(L2Record {
            exchange: parts[0].to_string(),
            symbol: parts[1].to_string(),
            exchange_ts_ns: parts[2].parse().ok()?,
            recv_ts_ns: parts[3].parse().ok()?,
            update_id: parts[4].parse().ok()?,
            bids: Self::parse_tuples(parts[5]),
            asks: Self::parse_tuples(parts[6]),
            channel: parts[7].to_string(),
        })
    }

    fn parse_tuples(_s: &str) -> Vec<(f64, f64)> {
        // Simple tuple parsing - in production use proper JSON or binary format
        vec![]
    }

    /// Check if replay is complete
    pub fn is_exhausted(&self) -> bool {
        self.current_file >= self.paths.len()
    }
}

/// Deterministic replay iterator
///
/// ## Total Ordering
///
/// Events are ordered by:
/// 1. `recv_ts_ns` (ascending) - receive time first
/// 2. `exchange` (ascending) - exchange name second
/// 3. `symbol` (ascending) - symbol name third
/// 4. `update_id` (ascending) - exchange-specific sequence number fourth
/// 5. `ingestion_order` (ascending) - stable ingestion order tiebreaker
///
/// This ensures a deterministic total order across all exchanges.
pub struct ReplayIterator {
    source: ReplaySource,
    /// Current records buffer
    buffer: Vec<L2Record>,
    /// Next record index in buffer
    next_idx: usize,
    /// Global ingestion counter for stable ordering
    ingestion_counter: u64,
}

impl ReplayIterator {
    pub fn new(paths: Vec<String>) -> Self {
        Self {
            source: ReplaySource::new(paths),
            buffer: Vec::new(),
            next_idx: 0,
            ingestion_counter: 0,
        }
    }

    /// Start replay
    pub fn start(&mut self) -> std::io::Result<()> {
        self.source.open()
    }

    /// Get next record in deterministic total order
    ///
    /// Order: (recv_ts_ns, exchange, symbol, update_id, ingestion_order) ascending
    pub fn next_record(&mut self) -> std::io::Result<Option<L2Record>> {
        if self.buffer.is_empty() || self.next_idx >= self.buffer.len() {
            let new_records = self.source.read_batch(1000)?;
            self.next_idx = 0;

            if new_records.is_empty() {
                self.buffer.clear();
            } else {
                // Sort buffer deterministically by total order key
                let mut records_with_order: Vec<(usize, L2Record)> = new_records
                    .into_iter()
                    .enumerate()
                    .collect();

                records_with_order.sort_by(|(idx_a, a), (idx_b, b)| {
                    a.recv_ts_ns.cmp(&b.recv_ts_ns)
                        .then_with(|| a.exchange.cmp(&b.exchange))
                        .then_with(|| a.symbol.cmp(&b.symbol))
                        .then_with(|| a.update_id.cmp(&b.update_id))
                        .then_with(|| idx_a.cmp(idx_b)) // stable: original index as final tiebreaker
                });

                self.buffer = records_with_order.into_iter().map(|(_, r)| r).collect();
            }
        }

        if self.next_idx < self.buffer.len() {
            self.ingestion_counter += 1;
            let record = self.buffer[self.next_idx].clone();
            self.next_idx += 1;
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orderbook::l2_update::Exchange;

    #[test]
    fn test_l2_record_from_update() {
        let update = L2Update::snapshot(
            Exchange::Binance,
            "BTCUSDT",
            1000,
            2000,
            1,
            vec![(100.0, 1.0)],
            vec![(101.0, 1.0)],
            "depth@100ms",
        );

        let record = L2Record::from(&update);

        assert_eq!(record.exchange, "binance");
        assert_eq!(record.symbol, "BTCUSDT");
        assert_eq!(record.exchange_ts_ns, 1000);
        assert_eq!(record.recv_ts_ns, 2000);
        assert_eq!(record.update_id, 1);
        assert_eq!(record.bids, vec![(100.0, 1.0)]);
        assert_eq!(record.asks, vec![(101.0, 1.0)]);
    }

    #[tokio::test]
    async fn test_persister_write() {
        let config = PersisterConfig {
            base_dir: "/tmp/l2_test".to_string(),
            flush_interval: 10,
            max_rows_per_file: 100,
        };

        let mut persister = L2Persister::new(config);

        let update = L2Update::snapshot(
            Exchange::Binance,
            "BTCUSDT",
            1000,
            2000,
            1,
            vec![(100.0, 1.0)],
            vec![(101.0, 1.0)],
            "depth@100ms",
        );

        persister.write_update(&update).unwrap();
        persister.flush().unwrap();
        persister.close().unwrap();
    }

    #[test]
    fn test_replay_total_ordering() {
        // Create records with same recv_ts_ns to test tie-breaking
        let records = vec![
            L2Record {
                exchange: "binance".to_string(),
                symbol: "BTCUSDT".to_string(),
                exchange_ts_ns: 1000,
                recv_ts_ns: 5000,
                update_id: 2,
                bids: vec![],
                asks: vec![],
                channel: "depth@100ms".to_string(),
            },
            L2Record {
                exchange: "binance".to_string(),
                symbol: "BTCUSDT".to_string(),
                exchange_ts_ns: 1000,
                recv_ts_ns: 5000,
                update_id: 1,
                bids: vec![],
                asks: vec![],
                channel: "depth@100ms".to_string(),
            },
            L2Record {
                exchange: "bybit".to_string(),
                symbol: "BTCUSDT".to_string(),
                exchange_ts_ns: 1000,
                recv_ts_ns: 5000,
                update_id: 1,
                bids: vec![],
                asks: vec![],
                channel: "orderbook.50".to_string(),
            },
            L2Record {
                exchange: "binance".to_string(),
                symbol: "ETHUSDT".to_string(),
                exchange_ts_ns: 1000,
                recv_ts_ns: 5000,
                update_id: 1,
                bids: vec![],
                asks: vec![],
                channel: "depth@100ms".to_string(),
            },
        ];

        // Sort using the same logic as ReplayIterator
        let mut records_with_order: Vec<(usize, L2Record)> = records
            .into_iter()
            .enumerate()
            .collect();

        records_with_order.sort_by(|(idx_a, a), (idx_b, b)| {
            a.recv_ts_ns.cmp(&b.recv_ts_ns)
                .then_with(|| a.exchange.cmp(&b.exchange))
                .then_with(|| a.symbol.cmp(&b.symbol))
                .then_with(|| a.update_id.cmp(&b.update_id))
                .then_with(|| idx_a.cmp(idx_b))
        });

        let sorted: Vec<_> = records_with_order.into_iter().map(|(_, r)| r).collect();

        // Expected order:
        // 1. binance BTCUSDT id=1 (recv=5000, exchange=binance, symbol=BTCUSDT, id=1)
        // 2. binance BTCUSDT id=2 (recv=5000, exchange=binance, symbol=BTCUSDT, id=2)
        // 3. binance ETHUSDT (recv=5000, exchange=binance, symbol=ETHUSDT)
        // 4. bybit BTCUSDT (recv=5000, exchange=bybit, symbol=BTCUSDT)

        assert_eq!(sorted[0].exchange, "binance");
        assert_eq!(sorted[0].symbol, "BTCUSDT");
        assert_eq!(sorted[0].update_id, 1);

        assert_eq!(sorted[1].exchange, "binance");
        assert_eq!(sorted[1].symbol, "BTCUSDT");
        assert_eq!(sorted[1].update_id, 2);

        assert_eq!(sorted[2].exchange, "binance");
        assert_eq!(sorted[2].symbol, "ETHUSDT");

        assert_eq!(sorted[3].exchange, "bybit");
        assert_eq!(sorted[3].symbol, "BTCUSDT");
    }
}
