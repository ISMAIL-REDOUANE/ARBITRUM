//! # L2 Update Data Model
//!
//! Canonical L2 update representation from exchanges.
//!
//! ## Timestamp Semantics (P0 — Phase 3)
//!
//! - `exchange_ts_ns`: timestamp **supplied by the exchange protocol**
//!   (authoritative event time)
//! - `recv_ts_ns`: local receive timestamp, attached by the adapter at the
//!   moment the frame is read off the socket
//!
//! These are DIFFERENT quantities and are never conflated. Per-exchange
//! protocol field mapping:
//!
//! | Exchange | Channel            | `exchange_ts_ns` source                    |
//! |----------|--------------------|--------------------------------------------|
//! | Binance  | `@depth20@100ms`   | **NONE** — the partial book depth payload  |
//! |          |                    | contains only `lastUpdateId` (no event     |
//! |          |                    | time). Documented limitation:              |
//! |          |                    | `exchange_ts_ns = 0` sentinel; `recv_ts_ns`|
//! |          |                    | drives all liveness/staleness logic.       |
//! | Bybit    | `orderbook.50`     | envelope `ts` (milliseconds → ns)          |
//! | Bitget   | `books15`          | `data[0].ts` (ms string → ns); fallback    |
//! |          |                    | top-level `ts` (ms); if both absent the    |
//! |          |                    | frame is REJECTED (fail-closed), never     |
//! |          |                    | fabricated from the local clock.           |
//!
//! ## Update Type Semantics
//!
//! - Binance `@depth20@100ms`: self-contained snapshot (full top-20
//!   replacement). No delta-sequence semantics are invented.
//! - Bybit `orderbook.50`: first push `type=snapshot` (full replace),
//!   subsequent pushes `type=delta` (official protocol semantics: level
//!   size `"0"` or `""` = delete level). NOT invented — this is the
//!   documented Bybit v5 contract.
//! - Bitget `books15`: every push carries the complete 15-level book
//!   (`action=snapshot` then `action=update`, both full replacements).
//!
//! ## Level Wire Format
//!
//! All three exchanges encode levels as JSON arrays
//! `["price", "quantity"]` — parsed as `(String, String)` tuples.

use serde::{Deserialize, Serialize};

/// Exchange identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Exchange {
    Binance,
    Bybit,
    Bitget,
}

impl Exchange {
    pub fn as_str(&self) -> &'static str {
        match self {
            Exchange::Binance => "binance",
            Exchange::Bybit => "bybit",
            Exchange::Bitget => "bitget",
        }
    }

    pub fn parse_exchange(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "binance" => Some(Exchange::Binance),
            "bybit" => Some(Exchange::Bybit),
            "bitget" => Some(Exchange::Bitget),
            _ => None,
        }
    }
}

impl std::fmt::Display for Exchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Side of the orderbook
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Bid,
    Ask,
}

/// Sentinel for "protocol supplies no exchange timestamp"
/// (currently: Binance partial book depth streams).
pub const NO_EXCHANGE_TS: u64 = 0;

/// Current UNIX time in nanoseconds (local receive clock).
pub fn unix_now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// L2 Update from exchange
///
/// This is the canonical representation that flows through the system:
/// Exchange WS → Adapter → L2Update → SymbolActor → L2Book
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L2Update {
    /// Exchange identifier
    pub exchange: Exchange,
    /// Trading symbol (e.g., "BTCUSDT")
    pub symbol: String,
    /// Exchange-side timestamp (nanoseconds, authoritative).
    /// `NO_EXCHANGE_TS` (0) when the protocol supplies none (documented).
    pub exchange_ts_ns: u64,
    /// Local receive timestamp (nanoseconds)
    pub recv_ts_ns: u64,
    /// Update type
    pub update_type: UpdateType,
    /// Update ID (exchange-specific sequence number)
    pub update_id: u64,
    /// Bids (price, quantity pairs)
    #[serde(default)]
    pub bids: Vec<(f64, f64)>,
    /// Asks (price, quantity pairs)
    #[serde(default)]
    pub asks: Vec<(f64, f64)>,
    /// Channel/type metadata (e.g., "depth20@100ms", "orderbook.50", "books15")
    #[serde(default)]
    pub channel: String,
}

impl L2Update {
    /// Create a new L2 snapshot update
    #[allow(clippy::too_many_arguments)]
    pub fn snapshot(
        exchange: Exchange,
        symbol: &str,
        exchange_ts_ns: u64,
        recv_ts_ns: u64,
        update_id: u64,
        bids: Vec<(f64, f64)>,
        asks: Vec<(f64, f64)>,
        channel: &str,
    ) -> Self {
        Self {
            exchange,
            symbol: symbol.to_string(),
            exchange_ts_ns,
            recv_ts_ns,
            update_type: UpdateType::Snapshot,
            update_id,
            bids,
            asks,
            channel: channel.to_string(),
        }
    }

    /// Create a new L2 incremental (delta) update.
    ///
    /// Used by Bybit `orderbook.50` deltas: each entry is
    /// `(price, quantity)` where `quantity == 0.0` means "delete level"
    /// (official Bybit v5 semantics).
    #[allow(clippy::too_many_arguments)]
    pub fn incremental(
        exchange: Exchange,
        symbol: &str,
        exchange_ts_ns: u64,
        recv_ts_ns: u64,
        update_id: u64,
        bids: Vec<(f64, f64)>,
        asks: Vec<(f64, f64)>,
        channel: &str,
    ) -> Self {
        Self {
            update_type: UpdateType::Incremental,
            ..Self::snapshot(
                exchange,
                symbol,
                exchange_ts_ns,
                recv_ts_ns,
                update_id,
                bids,
                asks,
                channel,
            )
        }
    }

    /// Validate update has finite positive prices and non-negative quantities
    pub fn validate(&self) -> Result<(), L2UpdateError> {
        for (price, qty) in &self.bids {
            if !price.is_finite() || *price <= 0.0 {
                return Err(L2UpdateError::InvalidPrice(*price));
            }
            if !qty.is_finite() || *qty < 0.0 {
                return Err(L2UpdateError::InvalidQuantity(*qty));
            }
        }

        for (price, qty) in &self.asks {
            if !price.is_finite() || *price <= 0.0 {
                return Err(L2UpdateError::InvalidPrice(*price));
            }
            if !qty.is_finite() || *qty < 0.0 {
                return Err(L2UpdateError::InvalidQuantity(*qty));
            }
        }

        Ok(())
    }

    /// Check if update has both bids and asks
    pub fn has_book_data(&self) -> bool {
        !self.bids.is_empty() || !self.asks.is_empty()
    }
}

/// Type of L2 update
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum UpdateType {
    /// Full snapshot replacement
    #[default]
    Snapshot,
    /// Incremental update
    Incremental,
    /// Refresh/heartbeat
    Refresh,
}

/// L2 Update validation error
#[derive(Debug, Clone)]
pub enum L2UpdateError {
    InvalidPrice(f64),
    InvalidQuantity(f64),
    EmptyUpdate,
    StaleUpdate { expected_id: u64, actual_id: u64 },
}

impl std::fmt::Display for L2UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            L2UpdateError::InvalidPrice(p) => write!(f, "Invalid price: {}", p),
            L2UpdateError::InvalidQuantity(q) => write!(f, "Invalid quantity: {}", q),
            L2UpdateError::EmptyUpdate => write!(f, "Update has no book data"),
            L2UpdateError::StaleUpdate { expected_id, actual_id } => {
                write!(f, "Stale update: expected {}, got {}", expected_id, actual_id)
            }
        }
    }
}

impl std::error::Error for L2UpdateError {}

// ─────────────────────────────────────────────────────────────────────────────
// Protocol parsers (fail-closed)
// ─────────────────────────────────────────────────────────────────────────────

/// Protocol parse error. Every variant is explicit — no silent fallbacks.
#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    /// Frame is not valid JSON
    MalformedJson(String),
    /// A required protocol field is missing
    MissingField(&'static str),
    /// A price value is invalid (non-finite, <= 0, unparseable)
    InvalidPrice(String),
    /// A quantity value is invalid (non-finite, < 0, unparseable)
    InvalidQuantity(String),
    /// Frame carries an empty book (no bids and no asks)
    EmptyBook,
    /// Frame is a protocol control message (ping/pong/ack) — not an error,
    /// simply nothing to ingest.
    Ignored(&'static str),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::MalformedJson(e) => write!(f, "Malformed JSON: {}", e),
            ParseError::MissingField(fld) => write!(f, "Missing protocol field: {}", fld),
            ParseError::InvalidPrice(p) => write!(f, "Invalid price: {}", p),
            ParseError::InvalidQuantity(q) => write!(f, "Invalid quantity: {}", q),
            ParseError::EmptyBook => write!(f, "Empty book"),
            ParseError::Ignored(why) => write!(f, "Ignored frame: {}", why),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse a level tuple `["price", "qty"]` strictly (fail-closed).
fn parse_level(
    (price, qty): &(String, String),
) -> Result<(f64, f64), ParseError> {
    let p: f64 = price
        .parse()
        .map_err(|_| ParseError::InvalidPrice(price.clone()))?;
    let q: f64 = if qty.is_empty() {
        0.0 // Bybit delta delete semantics: "" size = remove level
    } else {
        qty.parse()
            .map_err(|_| ParseError::InvalidQuantity(qty.clone()))?
    };
    if !p.is_finite() || p <= 0.0 {
        return Err(ParseError::InvalidPrice(price.clone()));
    }
    if !q.is_finite() || q < 0.0 {
        return Err(ParseError::InvalidQuantity(qty.clone()));
    }
    Ok((p, q))
}

fn parse_levels(levels: &[(String, String)]) -> Result<Vec<(f64, f64)>, ParseError> {
    levels.iter().map(parse_level).collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Binance — partial book depth stream (`@depth20@100ms`)
// ─────────────────────────────────────────────────────────────────────────────

/// Binance partial book depth payload.
///
/// Wire format (combined stream):
/// ```json
/// {"stream":"btcusdt@depth20@100ms",
///  "data":{"lastUpdateId":160,"bids":[["0.0024","10"]],"asks":[["0.0026","100"]]}}
/// ```
///
/// NOTE: the partial book depth payload contains **no exchange timestamp**
/// (only `lastUpdateId`). `exchange_ts_ns` is therefore set to
/// [`NO_EXCHANGE_TS`] — this is a documented protocol limitation, never a
/// fabricated value.
#[derive(Debug, Deserialize)]
pub struct BinancePartialDepth {
    #[serde(rename = "lastUpdateId", default)]
    pub last_update_id: Option<u64>,
    #[serde(default)]
    pub bids: Vec<(String, String)>,
    #[serde(default)]
    pub asks: Vec<(String, String)>,
}

/// Binance diff-depth stream payload (`<symbol>@depth`, kept for
/// completeness/tests — the L2 collector uses the partial depth stream).
#[derive(Debug, Deserialize)]
pub struct BinanceDiffDepth {
    #[serde(rename = "e", default)]
    pub event_type: String,
    #[serde(rename = "E", default)]
    pub event_time: Option<u64>,
    #[serde(rename = "s", default)]
    pub symbol: String,
    #[serde(rename = "U", default)]
    pub first_update_id: Option<u64>,
    #[serde(rename = "u", default)]
    pub final_update_id: Option<u64>,
    #[serde(default)]
    pub bids: Vec<(String, String)>,
    #[serde(default)]
    pub asks: Vec<(String, String)>,
}

/// Parse a Binance frame into L2 updates.
///
/// Accepts:
/// - combined-stream wrapper: `{"stream": "...", "data": {...}}`
/// - bare partial depth payload (symbol must be supplied via
///   `fallback_symbol`)
///
/// `exchange_ts_ns = NO_EXCHANGE_TS` (protocol has no timestamp — documented).
pub fn parse_binance_frame(
    text: &str,
    recv_ts_ns: u64,
    fallback_symbol: Option<&str>,
) -> Result<Vec<L2Update>, ParseError> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| ParseError::MalformedJson(e.to_string()))?;

    // Combined stream wrapper?
    let (stream_name, data) = match value.get("stream").and_then(|s| s.as_str()) {
        Some(stream) => {
            let data = value.get("data").ok_or(ParseError::MissingField("data"))?;
            (Some(stream.to_string()), data.clone())
        }
        None => (None, value),
    };

    // Symbol: from stream name ("btcusdt@depth20@100ms") or fallback.
    let symbol = stream_name
        .as_deref()
        .and_then(|s| s.split('@').next())
        .map(|s| s.to_uppercase())
        .or_else(|| fallback_symbol.map(|s| s.to_uppercase()))
        .ok_or(ParseError::MissingField("stream"))?;

    let depth: BinancePartialDepth = serde_json::from_value(data)
        .map_err(|e| ParseError::MalformedJson(e.to_string()))?;

    let update_id = depth.last_update_id.ok_or(ParseError::MissingField("lastUpdateId"))?;
    let bids = parse_levels(&depth.bids)?;
    let asks = parse_levels(&depth.asks)?;

    if bids.is_empty() && asks.is_empty() {
        return Err(ParseError::EmptyBook);
    }

    Ok(vec![L2Update::snapshot(
        Exchange::Binance,
        &symbol,
        NO_EXCHANGE_TS,
        recv_ts_ns,
        update_id,
        bids,
        asks,
        stream_name.as_deref().unwrap_or("depth20@100ms"),
    )])
}

// ─────────────────────────────────────────────────────────────────────────────
// Bybit — v5 public spot `orderbook.50`
// ─────────────────────────────────────────────────────────────────────────────

/// Bybit v5 orderbook envelope.
///
/// Wire format:
/// ```json
/// {"topic":"orderbook.50.BTCUSDT","type":"snapshot","ts":1672304486867,
///  "data":{"s":"BTCUSDT","b":[["16723.5","0.003"]],"a":[["16723.6","0.717"]],
///          "u":1672304486868,"seq":7968424597}}
/// ```
///
/// `exchange_ts_ns` source: envelope `ts` (milliseconds → ns).
/// `update_id` source: `data.u` (update id), falling back to `data.seq`.
#[derive(Debug, Deserialize)]
pub struct BybitOrderbookMessage {
    #[serde(default)]
    pub topic: String,
    #[serde(rename = "type", default)]
    pub msg_type: String,
    #[serde(default)]
    pub ts: Option<u64>,
    #[serde(default)]
    pub data: Option<BybitOrderbookData>,
    /// Control frames: `{"op":"pong"}` / `{"op":"ping"}` etc.
    #[serde(default)]
    pub op: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BybitOrderbookData {
    #[serde(rename = "s", default)]
    pub symbol: String,
    #[serde(rename = "b", default)]
    pub bids: Vec<(String, String)>,
    #[serde(rename = "a", default)]
    pub asks: Vec<(String, String)>,
    #[serde(rename = "u", default)]
    pub u: Option<u64>,
    #[serde(rename = "seq", default)]
    pub seq: Option<u64>,
}

/// Parse a Bybit v5 frame into L2 updates.
///
/// Control frames (`{"op":"pong"}`, subscription acks) yield `Ok(vec![])`.
/// `type=snapshot` → full replacement; `type=delta` → incremental
/// (documented Bybit v5 semantics, not invented).
pub fn parse_bybit_frame(text: &str, recv_ts_ns: u64) -> Result<Vec<L2Update>, ParseError> {
    let msg: BybitOrderbookMessage = serde_json::from_str(text)
        .map_err(|e| ParseError::MalformedJson(e.to_string()))?;

    // Control / ack frames are ignored (not errors).
    if let Some(op) = &msg.op {
        return match op.as_str() {
            "pong" | "ping" | "subscribe" | "auth" => Ok(vec![]),
            _ => Err(ParseError::Ignored("unknown op frame")),
        };
    }

    let data = msg.data.ok_or(ParseError::MissingField("data"))?;
    let ts = msg.ts.ok_or(ParseError::MissingField("ts"))?;
    let update_id = data.u.or(data.seq).ok_or(ParseError::MissingField("seq"))?;

    let bids = parse_levels(&data.bids)?;
    let asks = parse_levels(&data.asks)?;

    if bids.is_empty() && asks.is_empty() {
        return Err(ParseError::EmptyBook);
    }

    let update = match msg.msg_type.as_str() {
        "snapshot" => L2Update::snapshot(
            Exchange::Bybit,
            &data.symbol,
            ts * 1_000_000,
            recv_ts_ns,
            update_id,
            bids,
            asks,
            &msg.topic,
        ),
        "delta" => L2Update::incremental(
            Exchange::Bybit,
            &data.symbol,
            ts * 1_000_000,
            recv_ts_ns,
            update_id,
            bids,
            asks,
            &msg.topic,
        ),
        _ => return Err(ParseError::Ignored("unknown message type")),
    };
    Ok(vec![update])
}
