//! # Symbol Metadata Resolution (P0 — Phase 4)
//!
//! Authoritative `(exchange, symbol)` metadata resolution.
//!
//! ## Contract
//!
//! Every L2 adapter MUST resolve `(exchange, symbol)` to authoritative
//! metadata **before** normalizing any market data:
//!
//! - `tick_size` — minimum price increment (quote units)
//! - `price_precision` — number of price decimal places (= price scale used
//!   by `L2Book` for scaled-integer price representation)
//! - `quantity_precision` — number of quantity decimal places
//!
//! ## Fail-Closed Policy
//!
//! There is **NO universal fallback** (no `tick_scale = 10` guess, no
//! exchange-wide default). If a pair is not present in the curated table,
//! resolution fails and the symbol is **rejected**: the adapter refuses to
//! start for that symbol rather than silently guessing.
//!
//! The same resolved metadata is used consistently by:
//! `parser → L2Book → OBI → persistence → replay`
//! (the pipeline passes one `SymbolMetadata` instance down to all stages).
//!
//! ## Provenance
//!
//! Values below are snapshots of official exchange spot-market
//! specifications (PRICE_FILTER.tickSize / LOT_SIZE.stepSize on Binance;
//! priceFilter.tickSize / lotSize.qtyStep on Bybit; quoteScale / baseScale
//! on Bitget). They MUST be refreshed from `exchangeInfo`-style endpoints
//! for LIVE readiness (tracked as P1 — metadata refresh job). For research
//! and paper trading the static authoritative table is deterministic and
//! safe.

use crate::orderbook::l2_update::Exchange;

/// Authoritative metadata for one `(exchange, symbol)` pair.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolMetadata {
    /// Exchange
    pub exchange: Exchange,
    /// Exchange name (lowercase), e.g. `"binance"`
    pub exchange_str: String,
    /// Symbol in canonical uppercase form, e.g. `"BTCUSDT"`
    pub symbol: String,
    /// Minimum price increment in quote currency units
    pub tick_size: f64,
    /// Price decimal places (used as the L2Book price scale)
    pub price_precision: u32,
    /// Quantity decimal places
    pub quantity_precision: u32,
}

impl SymbolMetadata {
    fn new(
        exchange: Exchange,
        symbol: &str,
        tick_size: f64,
        price_precision: u32,
        quantity_precision: u32,
    ) -> Self {
        Self {
            exchange,
            exchange_str: exchange.as_str().to_string(),
            symbol: symbol.to_string(),
            tick_size,
            price_precision,
            quantity_precision,
        }
    }

    /// Price scale (decimal places) used by `L2Book` for deterministic
    /// scaled-integer price representation.
    pub fn price_scale(&self) -> u32 {
        self.price_precision
    }
}

/// Resolve authoritative metadata for `(exchange, symbol)`.
///
/// Returns `None` (fail-closed) for any pair not in the curated table.
/// Callers MUST reject the symbol instead of guessing.
pub fn resolve(exchange: Exchange, symbol: &str) -> Option<SymbolMetadata> {
    let s = symbol.to_uppercase();
    match exchange {
        Exchange::Binance => resolve_binance(&s),
        Exchange::Bybit => resolve_bybit(&s),
        Exchange::Bitget => resolve_bitget(&s),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Curated authoritative tables (spec snapshot; see module docs)
// ─────────────────────────────────────────────────────────────────────────────

/// Binance spot: tickSize / pricePrecision / quantityPrecision
/// Source: Binance exchangeInfo PRICE_FILTER.tickSize, LOT_SIZE.stepSize.
fn resolve_binance(s: &str) -> Option<SymbolMetadata> {
    match s {
        // tick 0.01, qty step 0.00001
        "BTCUSDT" => Some(SymbolMetadata::new(Exchange::Binance, s, 0.01, 2, 5)),
        // tick 0.01, qty step 0.00001
        "ETHUSDT" => Some(SymbolMetadata::new(Exchange::Binance, s, 0.01, 2, 5)),
        // tick 0.0001, qty step 0.1
        "XRPUSDT" => Some(SymbolMetadata::new(Exchange::Binance, s, 0.0001, 4, 1)),
        _ => None,
    }
}

/// Bybit spot v5: tickSize / qtyStep
/// Source: Bybit v5 market instrument-info.
fn resolve_bybit(s: &str) -> Option<SymbolMetadata> {
    match s {
        // tick 0.1, qty step 0.001
        "BTCUSDT" => Some(SymbolMetadata::new(Exchange::Bybit, s, 0.1, 1, 3)),
        // tick 0.01, qty step 0.01
        "ETHUSDT" => Some(SymbolMetadata::new(Exchange::Bybit, s, 0.01, 2, 2)),
        _ => None,
    }
}

/// Bitget spot v2: quoteScale (price decimals) / baseScale (qty decimals)
/// Source: Bitget v2 public currencies/market spec.
fn resolve_bitget(s: &str) -> Option<SymbolMetadata> {
    match s {
        // price decimals 2, qty decimals 6
        "BTCUSDT" => Some(SymbolMetadata::new(Exchange::Bitget, s, 0.01, 2, 6)),
        // price decimals 2, qty decimals 4
        "ETHUSDT" => Some(SymbolMetadata::new(Exchange::Bitget, s, 0.01, 2, 4)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binance_btcusdt_authoritative() {
        let meta = resolve(Exchange::Binance, "BTCUSDT").expect("BTCUSDT must resolve");
        assert_eq!(meta.tick_size, 0.01);
        assert_eq!(meta.price_precision, 2);
        assert_eq!(meta.quantity_precision, 5);
        assert_eq!(meta.price_scale(), 2);
        assert_eq!(meta.exchange_str, "binance");
    }

    #[test]
    fn test_same_symbol_different_metadata_per_exchange() {
        let binance = resolve(Exchange::Binance, "BTCUSDT").unwrap();
        let bybit = resolve(Exchange::Bybit, "BTCUSDT").unwrap();
        let bitget = resolve(Exchange::Bitget, "BTCUSDT").unwrap();
        // Authoritative per-exchange values differ (Binance 0.01, Bybit 0.1)
        assert_ne!(binance.tick_size, bybit.tick_size);
        assert_eq!(bitget.tick_size, 0.01);
        assert_ne!(binance.price_precision, bybit.price_precision);
    }

    #[test]
    fn test_fail_closed_unknown_symbol() {
        assert!(resolve(Exchange::Binance, "UNKNOWNPAIR").is_none());
        assert!(resolve(Exchange::Bybit, "MADEUPUSDT").is_none());
        assert!(resolve(Exchange::Bitget, "NOTREALUSDT").is_none());
    }

    #[test]
    fn test_case_insensitive_resolution() {
        let meta = resolve(Exchange::Binance, "btcusdt").expect("lowercase must resolve");
        assert_eq!(meta.symbol, "BTCUSDT");
    }

    #[test]
    fn test_no_universal_fallback_scale() {
        // Every resolved pair must have a real price precision, and the
        // known-different pairs must not collapse to a single universal scale.
        let scales: Vec<u32> = [
            (Exchange::Binance, "BTCUSDT"),
            (Exchange::Bybit, "BTCUSDT"),
            (Exchange::Binance, "XRPUSDT"),
        ]
        .iter()
        .filter_map(|(e, s)| resolve(*e, s))
        .map(|m| m.price_scale())
        .collect();
        assert!(scales.contains(&2));
        assert!(scales.contains(&1));
        assert!(scales.contains(&4));
    }
}