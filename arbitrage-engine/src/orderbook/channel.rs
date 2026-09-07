//! # Bounded L2 Channel (P0 — Phase 5)
//!
//! Bounded channel between WS adapters and SymbolActors.
//!
//! ## Overflow Policy — "newest snapshot wins"
//!
//! Every L2 message on these feeds is a **self-contained snapshot** (full
//! top-N book replacement), so dropping an older queued snapshot loses no
//! information as long as the newest one is preserved. When the channel is
//! full, `send()` evicts the OLDEST queued snapshot and enqueues the newest.
//!
//! ## No Silent Drops
//!
//! Every eviction increments BOTH counters:
//! - `l2_dropped`  — snapshots dropped
//! - `l2_overflow` — overflow events recorded
//!
//! The counters are exposed via [`L2Channel::stats`] and mirrored into
//! `AdapterHealth` by the adapters, so sustained overflow degrades L2
//! health (see `health.rs`).
//!
//! ## Safety
//!
//! - Capacity is explicit at construction (from pipeline config).
//! - `send()` never blocks the WS receive loop (lock held for microseconds).
//! - `close()` is idempotent; `recv()` drains remaining items, then returns
//!   `None` — this is what guarantees lossless drain during shutdown.
//! - This is a channel, NOT shared book state: the SymbolActor still owns
//!   the mutable `L2Book` exclusively (no `Arc<Mutex<L2Book>>`).

use crate::orderbook::l2_update::L2Update;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

/// Outcome of a non-blocking send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    /// Snapshot enqueued.
    Accepted,
    /// Channel full: oldest snapshot evicted, newest enqueued.
    /// Counted as `l2_dropped` + `l2_overflow`.
    DroppedOldest,
}

struct Inner {
    queue: VecDeque<L2Update>,
    closed: bool,
}

/// Bounded, newest-wins L2 snapshot channel.
pub struct L2Channel {
    inner: Mutex<Inner>,
    notify: Notify,
    capacity: usize,
    /// Snapshots dropped due to overflow
    l2_dropped: AtomicU64,
    /// Overflow events (one per dropped snapshot)
    l2_overflow: AtomicU64,
    /// Total snapshots accepted into the queue
    accepted: AtomicU64,
}

impl L2Channel {
    /// Create a channel with an explicit capacity (from configuration).
    pub fn new(capacity: usize) -> Arc<Self> {
        assert!(capacity > 0, "L2 channel capacity must be > 0");
        Arc::new(Self {
            inner: Mutex::new(Inner {
                queue: VecDeque::with_capacity(capacity),
                closed: false,
            }),
            notify: Notify::new(),
            capacity,
            l2_dropped: AtomicU64::new(0),
            l2_overflow: AtomicU64::new(0),
            accepted: AtomicU64::new(0),
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Non-blocking send with newest-wins overflow policy.
    pub fn send(&self, update: L2Update) -> Result<SendOutcome, ChannelClosed> {
        let outcome = {
            let mut inner = self.inner.lock();
            if inner.closed {
                return Err(ChannelClosed);
            }
            if inner.queue.len() >= self.capacity {
                // Newest snapshot wins: evict the oldest queued snapshot.
                let _evicted = inner.queue.pop_front();
                self.l2_dropped.fetch_add(1, Ordering::Relaxed);
                self.l2_overflow.fetch_add(1, Ordering::Relaxed);
                inner.queue.push_back(update);
                SendOutcome::DroppedOldest
            } else {
                inner.queue.push_back(update);
                self.accepted.fetch_add(1, Ordering::Relaxed);
                SendOutcome::Accepted
            }
        };
        // Wake one consumer (outside the lock).
        self.notify.notify_one();
        Ok(outcome)
    }

    /// Async receive. Returns `None` once the channel is closed AND drained.
    pub async fn recv(self: &Arc<Self>) -> Option<L2Update> {
        loop {
            // Register interest BEFORE checking state to avoid missed wakeups.
            let notified = self.notify.notified();
            {
                let mut inner = self.inner.lock();
                if let Some(update) = inner.queue.pop_front() {
                    return Some(update);
                }
                if inner.closed {
                    return None;
                }
            }
            notified.await;
        }
    }

    /// Synchronous non-blocking receive (tests / polling).
    pub fn try_recv(&self) -> Option<L2Update> {
        self.inner.lock().queue.pop_front()
    }

    /// Close the channel. Idempotent. Wakes all consumers.
    pub fn close(&self) {
        {
            let mut inner = self.inner.lock();
            if inner.closed {
                return;
            }
            inner.closed = true;
        }
        self.notify.notify_waiters();
    }

    pub fn is_closed(&self) -> bool {
        self.inner.lock().closed
    }

    pub fn len(&self) -> usize {
        self.inner.lock().queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// (l2_dropped, l2_overflow, accepted)
    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.l2_dropped.load(Ordering::Relaxed),
            self.l2_overflow.load(Ordering::Relaxed),
            self.accepted.load(Ordering::Relaxed),
        )
    }
}

/// Error returned when sending into a closed channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelClosed;

impl std::fmt::Display for ChannelClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "L2 channel closed")
    }
}

impl std::error::Error for ChannelClosed {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orderbook::l2_update::Exchange;

    fn snap(id: u64) -> L2Update {
        L2Update::snapshot(
            Exchange::Binance,
            "BTCUSDT",
            1000 + id,
            2000 + id,
            id,
            vec![(100.0, 1.0)],
            vec![(101.0, 1.0)],
            "depth20@100ms",
        )
    }

    #[test]
    fn test_capacity_explicit_and_enforced() {
        let ch = L2Channel::new(4);
        assert_eq!(ch.capacity(), 4);
        for id in 0..4 {
            assert_eq!(ch.send(snap(id)).unwrap(), SendOutcome::Accepted);
        }
        assert_eq!(ch.len(), 4);
        assert_eq!(ch.stats(), (0, 0, 4));
    }

    #[test]
    fn test_newest_wins_overflow_policy() {
        let ch = L2Channel::new(2);
        ch.send(snap(1)).unwrap();
        ch.send(snap(2)).unwrap();
        // Third send evicts snapshot 1 (oldest), keeps 2 and 3.
        assert_eq!(ch.send(snap(3)).unwrap(), SendOutcome::DroppedOldest);
        assert_eq!(ch.stats(), (1, 1, 2));

        let first = ch.try_recv().unwrap();
        assert_eq!(first.update_id, 2, "oldest (id=1) must be evicted");
        let second = ch.try_recv().unwrap();
        assert_eq!(second.update_id, 3, "newest (id=3) must be preserved");
        assert!(ch.try_recv().is_none());
    }

    #[test]
    fn test_drop_counters_recorded_not_silent() {
        let ch = L2Channel::new(1);
        ch.send(snap(1)).unwrap();
        for id in 2..=10 {
            let _ = ch.send(snap(id)).unwrap(); // each evicts previous
        }
        let (dropped, overflow, _) = ch.stats();
        assert_eq!(dropped, 9, "l2_dropped must count every eviction");
        assert_eq!(overflow, 9, "l2_overflow must count every overflow");
        // Only the newest survives.
        let last = ch.try_recv().unwrap();
        assert_eq!(last.update_id, 10);
    }

    #[test]
    fn test_close_is_idempotent_and_drains() {
        let ch = L2Channel::new(4);
        ch.send(snap(1)).unwrap();
        ch.close();
        ch.close(); // idempotent
        assert!(ch.is_closed());
        // Draining still works after close.
        assert!(ch.try_recv().is_some());
        assert!(ch.try_recv().is_none());
        // Sends after close fail explicitly.
        assert_eq!(ch.send(snap(2)), Err(ChannelClosed));
    }

    #[tokio::test]
    async fn test_async_recv_drains_then_returns_none_on_close() {
        let ch = L2Channel::new(8);
        for id in 0..3 {
            ch.send(snap(id)).unwrap();
        }
        ch.close();

        let mut received = Vec::new();
        while let Some(update) = ch.recv().await {
            received.push(update.update_id);
        }
        assert_eq!(received, vec![0, 1, 2], "drain must be lossless");
    }

    #[tokio::test]
    async fn test_async_recv_wakes_on_send() {
        let ch = L2Channel::new(8);
        let ch2 = ch.clone();
        let handle = tokio::spawn(async move {
            // Will block until a send arrives.
            ch2.recv().await
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        ch.send(snap(42)).unwrap();
        let got = handle.await.unwrap();
        assert_eq!(got.unwrap().update_id, 42);
    }
}
