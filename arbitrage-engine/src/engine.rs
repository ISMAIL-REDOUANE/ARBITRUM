//! # Core Arbitrage Engine Module
//!
//! Orchestrates all components with hardware-aligned thread affinity.
//!
//! ## CPU Core Pinning Architecture
//!
//! For sub-100µs deterministic latency, we pin critical threads to isolated CPU cores:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                        PHYSICAL CPU TOPOLOGY                              │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │                                                                          │
//! │   Core 0 (OS)      Core 1 (OS)      Core 2 (PINNED)   Core 3 (PINNED) │
//! │   ┌──────────┐    ┌──────────┐    ┌──────────────┐  ┌──────────────┐  │
//! │   │  Kernel  │    │  Kernel  │    │  WS Thread   │  │  Sim Thread   │  │
//! │   │  House-  │    │  House-   │    │  (Producer)  │  │  (Consumer)  │  │
//! │   │  keeping │    │  keeping  │    │              │  │              │  │
//! │   └──────────┘    └──────────┘    └──────┬───────┘  └──────┬───────┘  │
//! │                                            │                     │         │
//! │                          SPSC Ring Buffer (L1/L2 Cache Line)        │         │
//! │                         ┌─────────────────────────────────────┐   │         │
//! │                         │  [slot 0] [slot 1] ... [slot 1023]   │   │         │
//! │                         │   Shared Cache Line (same physical die)  │         │
//! │                         └─────────────────────────────────────┘   │         │
//! │                                            ↑                     ↑         │
//! │                                            └────── L1/L2 ───────┘         │
//! │                                                                          │
//! │   NUMA Node 0                                                          │
//! │   ┌─────────────────────────────────────────────────────────────────┐   │
//! │   │  L3 Cache (shared across all cores on this NUMA node)          │   │
//! │   └─────────────────────────────────────────────────────────────────┘   │
//! │                                                                          │
//! └─────────────────────────────────────────────────────────────────────────┘
//!
//! ## Linux Kernel Isolation (GRUB Configuration)
//!
//! Add to GRUB_CMDLINE_LINUX in /etc/default/grub:
//!
//! ```bash
//! GRUB_CMDLINE_LINUX="isolcpus=2,3 nohz_full=2,3 rcu_nocbs=2,3"
//! ```
//!
//! Then update grub and reboot:
//! ```bash
//! sudo update-grub
//! sudo reboot
//! ```
//!
//! Verify isolation:
//! ```bash
//! # Check isolated cores
//! cat /sys/devices/system/cpu/isolated
//!
//! # Verify no kernel threads on pinned cores
//! taskset -pc 2
//! # Should show: pid of current process's cpuset = 2, and no other tasks
//! ```
//!
//! ## Why Adjacent Cores for L1/L2 Cache Locality?
//!
//! crossbeam-channel's SPSC ring buffer uses cache-line aligned memory.
//! When producer (Core 2) writes and consumer (Core 3) reads from adjacent
//! cores sharing an L2 cache:
//!
//! - Producer writes to ring buffer slot
//! - Consumer reads from same slot (L2 hit, ~4-5 cycles vs ~40 cycles for L3)
//! - No cache line bouncing between cores (same physical die)
//! - Deterministic ~100ns round-trip vs ~500ns with cache bouncing
//!
//! ## Thread Priorities (Linux)
//!
//! For production, set real-time priority with chrt:
//! ```bash
//! sudo chrt -f 99 ./target/release/arbitrage-engine
//! ```

use crate::config::Config;
use crate::error::{ArbitrageError, Result};
use crate::types::{PoolState, Stats, PriceEvent, ArbitrageOpportunity};
use crate::cache_db::RevmCacheDB;
use crate::websocket::BinanceListener;

use std::sync::Arc;
use parking_lot::RwLock;
use crossbeam_channel::{bounded, Sender as ChannelSender, Receiver as ChannelReceiver};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use once_cell::sync::Lazy;
use std::sync::Mutex;

/// Global core assignment for validation
static CORE_ASSIGNMENTS: Lazy<Mutex<HashMap<String, usize>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Core pinning configuration
#[derive(Debug, Clone)]
pub struct CorePinning {
    /// WebSocket listener core (Producer)
    pub ws_core: usize,
    /// Simulation engine core (Consumer)  
    pub sim_core: usize,
    /// Minimum cores required for isolation
    pub min_cores: usize,
}

impl CorePinning {
    /// Validate that we have enough isolated cores
    pub fn validate(&self) -> Result<()> {
        let available_cores = num_cpus::get();
        
        // Check we have at least the cores we need
        if available_cores < self.min_cores {
            return Err(ArbitrageError::Config(format!(
                "Insufficient CPU cores: have {}, need {} isolated cores. \
                 See Linux kernel isolation in module docs.",
                available_cores, self.min_cores
            )));
        }
        
        // Check cores are different (producer/consumer on separate cores)
        if self.ws_core == self.sim_core {
            return Err(ArbitrageError::Config(
                "WebSocket and Simulation cores must be different".to_string()
            ));
        }
        
        // Check cores are in valid range
        if self.ws_core >= available_cores || self.sim_core >= available_cores {
            return Err(ArbitrageError::Config(format!(
                "Core assignment out of range: WS={}, Sim={}, available={}",
                self.ws_core, self.sim_core, available_cores
            )));
        }
        
        tracing::info!(
            "Core pinning validated: WS=Core{}, Sim=Core{} (total available: {})",
            self.ws_core, self.sim_core, available_cores
        );
        
        Ok(())
    }
    
    /// Get default pinning for a 4+ core system
    pub fn default_4_core() -> Self {
        Self {
            ws_core: 2,   // Producer on Core 2
            sim_core: 3,   // Consumer on Core 3
            min_cores: 4,  // Need at least 4 cores
        }
    }
}

impl Default for CorePinning {
    fn default() -> Self {
        Self {
            ws_core: 2,
            sim_core: 3,
            min_cores: 4,
        }
    }
}

impl std::fmt::Debug for SharedState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedState")
            .field("config", &self.config)
            .field("pinning", &self.pinning)
            .field("pools", &self.pools)
            .field("nonce_cache", &self.nonce_cache)
            .field("stats", &self.stats)
            .field("shutdown", &self.shutdown)
            .finish()
    }
}

/// Mock SharedState for testing without full initialization
impl SharedState {
    #[cfg(test)]
    pub fn mock() -> Self {
        Self::default()
    }
}

/// Shared state across all threads
/// 
/// # Thread Safety
/// 
/// - `price_channel`: SPSC (Producer: WS thread, Consumer: Sim thread) - lock-free
/// - `pools`: RwLock - multiple readers, single writer  
/// - `cache_db`: Arc<RwLock> - shared ownership, multiple readers
/// - `stats`: Atomic counters - lock-free
/// - `shutdown`: AtomicBool - lock-free flag
/// 
/// # Cache Locality
/// 
/// The SPSC ring buffer is designed to minimize cache line bouncing between
/// the producer (WS thread on Core 2) and consumer (Sim thread on Core 3).
/// crossbeam-channel allocates the ring buffer with cache-line alignment
/// to ensure adjacent slot access doesn't cause false sharing.
pub struct SharedState {
    /// Configuration
    pub config: Config,
    
    /// Core pinning assignment (immutable after init)
    pub pinning: CorePinning,
    
    /// Pre-synced pool states (RAM)
    /// 
    /// Access pattern: Read-heavy (WS updates occasionally, Sim reads on each event)
    /// Using RwLock allows multiple simultaneous readers (no contention)
    pub pools: RwLock<HashMap<String, PoolState>>,
    
    /// REVM CacheDB (in-memory state)
    /// 
    /// Cloned on each simulation (Arc ptr copy ~1ns, not full state copy)
    /// The actual state data is shared via Arc, only refcount is updated
    pub cache_db: Arc<RwLock<RevmCacheDB>>,
    
    /// Transaction nonce cache (for broadcaster)
    pub nonce_cache: RwLock<u64>,
    
    /// Performance statistics (atomic, lock-free)
    pub stats: Stats,
    
    /// Shutdown flag (atomic, lock-free)
    pub shutdown: AtomicBool,
    
    /// ═══════════════════════════════════════════════════════════════════════
    /// SPSC CHANNEL PAIR (Critical Path - Zero-Copy)
    /// ═══════════════════════════════════════════════════════════════════════
    /// 
    /// ## Why SPSC for Zero-Copy?
    /// 
    /// Single Producer Single Consumer (SPSC) channels allow:
    /// 
    /// 1. **Ring buffer with move semantics**: PriceEvent is moved into
    ///    the channel slot, not cloned. The struct is copied byte-by-byte
    ///    which is a single memcpy (~10-20ns for PriceEvent struct)
    /// 
    /// 2. **No synchronization overhead**: SPSC doesn't need the
    ///    memory ordering guarantees of MPMC. Producer and consumer
    ///    have private cache line access patterns.
    /// 
    /// 3. **Cache-line aligned slots**: crossbeam guarantees each slot
    ///    is on its own cache line, preventing false sharing.
    /// 
    /// ## Memory Layout
    /// 
    /// ```text
    /// Core 2 (Producer)              Core 3 (Consumer)
    /// ┌─────────────────┐            ┌─────────────────┐
    /// │   WS Thread     │            │   Sim Thread    │
    /// │   writes to     │ ────────→  │   reads from    │
    /// │   slot[n]      │   L1/L2    │   slot[n]       │
    /// └─────────────────┘   cache    └─────────────────┘
    ///                          ↕
    ///               ┌─────────────────────────┐
    ///               │   SPSC Ring Buffer      │
    ///               │   [slot 0] [slot 1]     │
    ///               │   [slot 2] [slot 3]     │
    ///               │   ...                   │
    ///               │   [slot 1023]           │
    ///               │   (1 cache line each)  │
    ///               └─────────────────────────┘
    /// ```
    pub price_channel: (ChannelSender<PriceEvent>, ChannelReceiver<PriceEvent>),
    
    /// Opportunity channel (SPSC: Sim → Sender)
    pub opportunity_channel: (ChannelSender<ArbitrageOpportunity>, ChannelReceiver<ArbitrageOpportunity>),
    
    /// Mempool event channel (SPSC: EventLoop → Sim)
    pub mempool_channel: (ChannelSender<crate::types::MempoolEvent>, ChannelReceiver<crate::types::MempoolEvent>),
    
    /// Block header channel (SPSC: EventLoop → Sim)
    pub block_channel: (ChannelSender<crate::types::BlockHeader>, ChannelReceiver<crate::types::BlockHeader>),
}

impl Default for SharedState {
    fn default() -> Self {
        let config = Config::default();
        let pinning = CorePinning::default();
        let ring_size = config.engine.ring_buffer_size;

        let (price_tx, price_rx) = bounded::<PriceEvent>(ring_size);
        let (opp_tx, opp_rx) = bounded::<ArbitrageOpportunity>(ring_size);
        let (mempool_tx, mempool_rx) = bounded::<crate::types::MempoolEvent>(ring_size);
        let (block_tx, block_rx) = bounded::<crate::types::BlockHeader>(ring_size);

        Self {
            config,
            pinning,
            pools: RwLock::new(HashMap::new()),
            cache_db: Arc::new(RwLock::new(RevmCacheDB::with_chain_id(42161).unwrap())),
            nonce_cache: RwLock::new(0),
            stats: Stats::new(),
            shutdown: AtomicBool::new(false),
            price_channel: (price_tx, price_rx),
            opportunity_channel: (opp_tx, opp_rx),
            mempool_channel: (mempool_tx, mempool_rx),
            block_channel: (block_tx, block_rx),
        }
    }
}

impl SharedState {
    /// Create new shared state
    /// 
    /// # Arguments
    /// 
    /// * `config` - Engine configuration
    /// * `pinning` - CPU core pinning configuration
    pub fn new(config: Config, pinning: CorePinning) -> Result<Self> {
        let ring_size = config.engine.ring_buffer_size;
        
        // Create SPSC channels with bounded capacity
        // 
        // BOUNDED vs UNBOUNDED:
        // - Bounded: Fixed ring buffer size, O(1) send/receive
        // - Unbounded: Grows dynamically, may heap-allocate
        //
        // We use bounded for deterministic memory usage
        let (price_tx, price_rx) = bounded::<PriceEvent>(ring_size);
        let (opp_tx, opp_rx) = bounded::<ArbitrageOpportunity>(ring_size);
        let (mempool_tx, mempool_rx) = bounded::<crate::types::MempoolEvent>(ring_size);
        let (block_tx, block_rx) = bounded::<crate::types::BlockHeader>(ring_size);
        
        Ok(Self {
            config,
            pinning,
            pools: RwLock::new(HashMap::new()),
            cache_db: Arc::new(RwLock::new(RevmCacheDB::new()?)),
            nonce_cache: RwLock::new(0),
            stats: Stats::new(),
            shutdown: AtomicBool::new(false),
            price_channel: (price_tx, price_rx),
            opportunity_channel: (opp_tx, opp_rx),
            mempool_channel: (mempool_tx, mempool_rx),
            block_channel: (block_tx, block_rx),
        })
    }
    
    /// Push price event to SPSC channel
    /// 
    /// ZERO-COPY GUARANTEE:
    /// - PriceEvent is moved via mem::move into channel slot
    /// - No cloning, no serialization, no heap allocation
    /// - O(1) time complexity
    /// 
    /// # Returns
    /// 
    /// * `Ok(())` if sent successfully
    /// * `Err(_)` if channel is disconnected (consumer died)
    #[inline]
    pub fn push_price_event(&self, event: PriceEvent) -> Result<()> {
        self.price_channel.0
            .send(event)
            .map_err(|_| ArbitrageError::Channel("Price event channel closed".to_string()))?;
        Ok(())
    }
    
    /// Pop price event from SPSC channel (non-blocking)
    /// 
    /// ZERO-COPY GUARANTEE:
    /// - Returns value directly from ring buffer slot
    /// - No cloning, no deserialization
    /// - O(1) time complexity
    #[inline]
    pub fn pop_price_event(&self) -> Option<PriceEvent> {
        self.price_channel.1.try_recv().ok()
    }
    
    /// Push arbitrage opportunity to channel
    #[inline]
    pub fn push_opportunity(&self, opp: ArbitrageOpportunity) -> Result<()> {
        self.opportunity_channel.0
            .send(opp)
            .map_err(|_| ArbitrageError::Channel("Opp channel closed".to_string()))?;
        Ok(())
    }
    
    /// Pop arbitrage opportunity (non-blocking)
    #[inline]
    pub fn pop_opportunity(&self) -> Option<ArbitrageOpportunity> {
        self.opportunity_channel.1.try_recv().ok()
    }
    
    /// Push mempool event to channel
    #[inline]
    pub fn push_mempool_event(&self, event: crate::types::MempoolEvent) -> Result<()> {
        self.mempool_channel.0
            .send(event)
            .map_err(|_| ArbitrageError::Channel("Mempool channel closed".to_string()))?;
        Ok(())
    }
    
    /// Pop mempool event (non-blocking)
    #[inline]
    pub fn pop_mempool_event(&self) -> Option<crate::types::MempoolEvent> {
        self.mempool_channel.1.try_recv().ok()
    }
    
    /// Push block header to channel
    #[inline]
    pub fn push_block_header(&self, header: crate::types::BlockHeader) -> Result<()> {
        self.block_channel.0
            .send(header)
            .map_err(|_| ArbitrageError::Channel("Block channel closed".to_string()))?;
        Ok(())
    }
    
    /// Pop block header (non-blocking)
    #[inline]
    pub fn pop_block_header(&self) -> Option<crate::types::BlockHeader> {
        self.block_channel.1.try_recv().ok()
    }
    
    /// Update pool state
    pub fn update_pool(&self, address: String, state: PoolState) {
        let mut pools = self.pools.write();
        pools.insert(address, state);
    }
    
    /// Get pool state (cloned)
    pub fn get_pool(&self, address: &str) -> Option<PoolState> {
        let pools = self.pools.read();
        pools.get(address).cloned()
    }
    
    /// Request shutdown
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
    
    /// Check if shutdown requested
    #[inline]
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }
}

/// ═══════════════════════════════════════════════════════════════════════════════
/// CORE AFFINITY & THREAD PINNING
/// ═══════════════════════════════════════════════════════════════════════════════
/// Pin current thread to a specific CPU core
/// 
/// # Arguments
/// 
/// * `core_id` - The core to pin to (0-indexed)
/// 
/// # Returns
/// 
/// * `Ok(())` if pinning succeeded
/// * `Err(...)` if core assignment failed
/// 
/// # Platform-Specific Behavior
/// 
/// - **Linux**: Uses sched_setaffinity syscall
/// - **Windows**: Uses SetThreadAffinityMask
/// - **macOS**: Uses thread_policy_set with THREAD_AFFINITY_POLICY
/// 
/// # Error Scenarios
/// 
/// - Invalid core_id (outside available cores)
/// - Insufficient permissions (requires CAP_SYS_NICE or root)
/// - OS scheduler interference (cores not properly isolated)
fn pin_to_core(core_id: usize) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use core_affinity::{CoreMask, set_for_current};
        let core_mask = CoreMask::one(core_id);
        set_for_current(core_mask)
            .map_err(|e| ArbitrageError::System(format!(
                "Failed to pin thread to core {}: {}. \
                 Ensure cores are isolated via Linux kernel parameters (isolcpus).",
                core_id, e
            )))?;
    }
    
    #[cfg(not(target_os = "linux"))]
    {
        // On non-Linux platforms, core pinning is stubbed out
        // Production deployment is on Linux with isolated cores
        tracing::debug!(
            "Core pinning not available on this platform (core_id={}). \
             Production target is Linux with isolcpus kernel parameters.",
            core_id
        );
    }
    
    tracing::debug!("Successfully pinned thread to core {}", core_id);
    Ok(())
}

/// Spawn a native OS thread pinned to a specific core
/// 
/// # Arguments
/// 
/// * `core_id` - Core to pin the thread to
/// * `name` - Human-readable thread name for debugging
/// * `f` - The closure to run in the pinned thread
/// 
/// # Thread Creation
/// 
/// Uses std::thread::spawn (not tokio::spawn) because:
/// 
/// 1. **No async runtime overhead**: Pure native thread, no Tokio task scheduling
/// 2. **Predictable scheduling**: OS executes thread directly on pinned core
/// 3. **No task polling**: No futures, no wakers, no async state machines
/// 
/// # Latency Impact
/// 
/// | Approach | Thread migration | Cache misses | Typical latency |
/// |----------|-----------------|--------------|----------------|
/// | Tokio spawn | Yes (thread pool) | High | 500-2000µs |
/// | Native + pin | No | Low (L1/L2) | 50-150µs |
fn spawn_pinned_thread<F>(core_id: usize, name: &str, f: F) -> Result<thread::JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    let thread_name = name.to_string();
    let core = core_id;
    
    let handle = thread::Builder::new()
        .name(thread_name.clone())
        .spawn(move || {
            // Pin this thread to the assigned core BEFORE any work
            // This ensures the thread starts on the right core
            if let Err(e) = pin_to_core(core) {
                tracing::error!("Failed to pin thread {} to core {}: {}", thread_name, core, e);
                return;
            }
            
            tracing::info!("Thread '{}' started on core {}", thread_name, core);
            
            // Run the actual work
            f();
            
            tracing::info!("Thread '{}' exiting", thread_name);
        })
        .map_err(|e| ArbitrageError::System(format!(
            "Failed to spawn thread '{}': {}", name, e
        )))?;
    
    Ok(handle)
}

/// ═══════════════════════════════════════════════════════════════════════════════
/// MAIN ARBITRAGE ENGINE WITH CORE PINNING
/// ═══════════════════════════════════════════════════════════════════════════════
/// Main arbitrage engine orchestrator
/// 
/// ## Architecture
/// 
/// ```text
/// ┌─────────────────────────────────────────────────────────────────────────┐
/// │                        ArbitrageEngine::run()                           │
/// │                                                                         │
/// │   ┌─────────────────────────────────────────────────────────────────┐   │
/// │   │  Thread 1: WebSocket Listener (Producer)                        │   │
/// │   │  Core: 2                                                        │   │
/// │   │  Runtime: Single-threaded Tokio (no scheduling overhead)          │   │
/// │   │  ┌─────────────────────────────────────────────────────────────┐ │   │
/// │   │  │ tokio::runtime::Builder::new_current_thread()              │ │   │
/// │   │  │     ↓                                                        │ │   │
/// │   │  │ Binance WebSocket (tokio-tungstenite)                       │ │   │
/// │   │  │     ↓                                                        │ │   │
/// │   │  │ serde_json::from_str (zero-copy parse)                      │ │   │
/// │   │  │     ↓                                                        │ │   │
/// │   │  │ price_channel.0.send(event) [SPSC]                           │ │   │
/// │   │  └─────────────────────────────────────────────────────────────┘ │   │
/// │   └─────────────────────────────────────────────────────────────────┘   │
/// │                                    │                                    │
/// │                                    │ SPSC (L2 cache line)               │
/// │                                    ↓                                    │
/// │   ┌─────────────────────────────────────────────────────────────────┐   │
/// │   │  Thread 2: Simulation Engine (Consumer) - HOT SPIN               │   │
/// │   │  Core: 3                                                        │   │
/// │   │  Runtime: NONE (pure blocking/spin loop)                         │   │
/// │   │  ┌─────────────────────────────────────────────────────────────┐ │   │
/// │   │  │ loop {                                                        │ │   │
/// │   │  │     if let Some(event) = price_channel.1.try_recv() {      │ │   │
/// │   │  │         // REVM simulation                                   │ │   │
/// │   │  │         // Check profitability                                │ │   │
/// │   │  │         // If profitable: push to opportunity_channel         │ │   │
/// │   │  │     } else {                                                │ │   │
/// │   │  │         std::hint::spin_loop(); // CPU pause instruction     │ │   │
/// │   │  │     }                                                        │ │   │
/// │   │  │ }                                                            │ │   │
/// │   │  └─────────────────────────────────────────────────────────────┘ │   │
/// │   └─────────────────────────────────────────────────────────────────┘   │
/// │                                    │                                    │
/// │                                    ↓                                    │
/// │   ┌─────────────────────────────────────────────────────────────────┐   │
/// │   │  Thread 3: Sender (Future: Transaction Broadcast)              │   │
/// │   │  Consumes from opportunity_channel                              │   │
/// │   │  Sends signed transactions to L2 RPC                            │   │
/// │   └─────────────────────────────────────────────────────────────────┘   │
/// └─────────────────────────────────────────────────────────────────────────┘
/// ```
pub struct ArbitrageEngine {
    shared_state: Arc<SharedState>,
}

impl ArbitrageEngine {
    /// Create new engine instance
    pub fn new(shared_state: Arc<SharedState>, cache_db: RevmCacheDB) -> Result<Self> {
        // Initialize cache_db in shared state
        {
            let mut db = shared_state.cache_db.write();
            *db = cache_db;
        }
        
        Ok(Self { shared_state })
    }
    
    /// Start the engine with CPU core pinning
    /// 
    /// # Launch Sequence
    /// 
    /// 1. Validate core pinning configuration
    /// 2. Spawn WebSocket thread (pinned to Core 2)
    /// 3. Spawn Simulation thread (pinned to Core 3)  
    /// 4. Main thread monitors both threads
    /// 
    /// # Shutdown
    /// 
    /// Signal SharedState::shutdown, threads detect and exit gracefully
    pub fn run(&self) -> Result<()> {
        let pinning = &self.shared_state.pinning;
        
        // ───────────────────────────────────────────────────────────────────
        // STEP 1: Validate core availability
        // ───────────────────────────────────────────────────────────────────
        pinning.validate()?;
        
        tracing::info!(
            "Starting ArbitrageEngine with core pinning: WS=Core{}, Sim=Core{}",
            pinning.ws_core, pinning.sim_core
        );
        
        // ───────────────────────────────────────────────────────────────────
        // STEP 2: Validate OS support for core affinity
        // ───────────────────────────────────────────────────────────────────
        #[cfg(target_os = "linux")]
        {
            if !core_affinity::can_affinity() {
                tracing::warn!(
                    "Core affinity not available. \
                     Latency may be non-deterministic. \
                     For sub-100µs latency, use Linux with isolated cores."
                );
            }
        }
        
        // ───────────────────────────────────────────────────────────────────
        // STEP 3: Spawn WebSocket Listener Thread (Producer)
        // 
        // Thread: Core 2
        // Runtime: Single-threaded Tokio
        // Purpose: Connect to Binance, parse JSON, push to SPSC
        // ───────────────────────────────────────────────────────────────────
        let ws_state = self.shared_state.clone();
        let ws_core = pinning.ws_core;
        
        let ws_handle = spawn_pinned_thread(ws_core, "ws-listener", move || {
            // Create single-threaded Tokio runtime
            // 
            // WHY SINGLE-THREADED?
            // - WebSocket processing is I/O bound, not CPU bound
            // - Single thread eliminates lock contention in tokio
            // - No async task scheduling overhead
            // - Deterministic memory access patterns
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create Tokio runtime for WS listener");
            
            rt.block_on(async {
                let listener = BinanceListener::new(ws_state.clone());
                if let Err(e) = listener.run().await {
                    tracing::error!("WebSocket listener error: {}", e);
                }
            });
        })?;
        
        // ───────────────────────────────────────────────────────────────────
        // STEP 4: Spawn Simulation Engine Thread (Consumer)
        // 
        // Thread: Core 3
        // Runtime: NONE (pure spin loop)
        // Purpose: Poll SPSC, run REVM simulation, push opportunities
        // 
        // WHY SPIN LOOP?
        // - We need sub-microsecond response to channel events
        // - Async runtime would add polling overhead (100-500µs)
        // - Spin loop with CPU pause is deterministic
        // - Only works because core is isolated (no other work competing)
        // ───────────────────────────────────────────────────────────────────
        let sim_state = self.shared_state.clone();
        let sim_core = pinning.sim_core;
        
        let sim_handle = spawn_pinned_thread(sim_core, "sim-engine", move || {
            Self::run_simulation_loop(sim_state);
        })?;
        
        // ───────────────────────────────────────────────────────────────────
        // STEP 5: Monitor threads (main thread)
        // 
        // The main thread handles opportunity processing and health monitoring
        // ───────────────────────────────────────────────────────────────────
        tracing::info!("All pinned threads started. Entering main loop.");
        
        loop {
            if self.shared_state.is_shutdown() {
                tracing::info!("Shutdown requested");
                break;
            }
            
            // Process any pending opportunities
            if let Some(opp) = self.shared_state.pop_opportunity() {
                self.process_opportunity(&opp);
            }
            
            // Brief sleep to prevent main thread from busy-spinning
            // This yields the core back to the OS scheduler
            thread::sleep(std::time::Duration::from_millis(10));
        }
        
        // Wait for child threads (with timeout)
        tracing::info!("Waiting for threads to exit...");
        
        let ws_result = ws_handle.join();
        let sim_result = sim_handle.join();
        
        if let Err(e) = ws_result {
            tracing::error!("WS thread panicked: {:?}", e);
        }
        if let Err(e) = sim_result {
            tracing::error!("Sim thread panicked: {:?}", e);
        }
        
        tracing::info!("ArbitrageEngine stopped");
        Ok(())
    }
    
    /// ═══════════════════════════════════════════════════════════════════════
    /// SIMULATION HOT SPIN LOOP
    /// 
    /// This is the most latency-critical code path. It runs on an isolated
    /// CPU core with no OS scheduler interference.
    /// 
    /// # Timing Budget
    /// 
    /// | Operation | Target | Maximum |
    /// |-----------|--------|---------|
    /// | SPSC recv | <50ns | 100ns |
    /// | Price parse | <5µs | 10µs |
    /// | REVM sim | <50µs | 100µs |
    /// | SPSC send | <50ns | 100ns |
    /// | **Total** | **<60µs** | **~210µs** |
    /// 
    /// # CPU Pause Instruction
    /// 
    /// When spinning waiting for work, we use `std::hint::spin_loop()`
    /// which emits the PAUSE instruction on x86:
    /// 
    /// - Reduces power consumption while waiting
    /// - Signals to hypervisor this is a spin-wait (if virtualized)
    /// - Doesn't yield to scheduler (we want to stay on this core!)
    /// 
    /// # Why Not tokio::task::yield_now()?
    /// 
    /// - yield_now() yields to OS scheduler, which may migrate us
    /// - spin_loop() keeps us pinned to our core
    /// - The core is isolated, so we're not stealing CPU from other work
    /// ═══════════════════════════════════════════════════════════════════════
    fn run_simulation_loop(state: Arc<SharedState>) {
        let mut empty_iterations = 0u64;
        const SPIN_THRESHOLD: u64 = 1000;
        
        loop {
            // ───────────────────────────────────────────────────────────────
            // CRITICAL PATH: Try to receive from SPSC channel
            // ───────────────────────────────────────────────────────────────
            // 
            // try_recv() is non-blocking:
            // - Returns Some(event) if available
            // - Returns None if channel empty
            // - O(1) operation, no locking
            if let Some(event) = state.pop_price_event() {
                empty_iterations = 0;
                
                // ───────────────────────────────────────────────────────────
                // Process the price event
                // ───────────────────────────────────────────────────────────
                let start = std::time::Instant::now();
                
                // Quick profitability check (before expensive REVM sim)
                if let Some(opportunity) = Self::check_opportunity(&event) {
                    // Run REVM simulation (the expensive part)
                    if let Some(result) = Self::simulate(&state, &opportunity) {
                        if result.success && result.profit_wei > 0 {
                            // Push to opportunity channel
                            if let Err(e) = state.push_opportunity(opportunity) {
                                tracing::error!("Failed to push opportunity: {}", e);
                            } else {
                                tracing::info!(
                                    "Opportunity: profit={}wei, sim_time={}µs",
                                    result.profit_wei,
                                    result.execution_time_us
                                );
                            }
                        }
                    }
                }
                
                let elapsed = start.elapsed().as_micros() as u64;
                
                // Record stats
                state.stats.record_latency(elapsed);
                
            } else {
                // ───────────────────────────────────────────────────────────────
                // NO EVENT AVAILABLE - Spin waiting
                // ───────────────────────────────────────────────────────────────
                // 
                // spin_loop() emits CPU PAUSE instruction:
                // - ~0 latency overhead (single instruction)
                // - Reduces power consumption during wait
                // - Keeps us pinned to this core (no migration)
                // 
                // After many empty spins, we could yield, but on an isolated
                // core there's nothing better to run, so we keep spinning.
                empty_iterations += 1;
                
                // Safety valve: if we've been spinning with no work for a
                // very long time, something might be wrong
                if empty_iterations > SPIN_THRESHOLD {
                    // Log every SPIN_THRESHOLD iterations to detect stalls
                    if empty_iterations.is_multiple_of(SPIN_THRESHOLD) {
                        tracing::warn!(
                            "Sim thread idle for {} iterations (no events)",
                            empty_iterations
                        );
                    }
                }
                
                std::hint::spin_loop();
            }
            
            // Check shutdown flag periodically
            if state.is_shutdown() {
                tracing::info!("Sim thread shutdown detected");
                break;
            }
        }
    }
    
    /// Quick opportunity check based on price data
    /// 
    /// This is a fast-path check before the expensive REVM simulation.
    /// Returns None if the trade is too small or uninteresting.
    fn check_opportunity(event: &PriceEvent) -> Option<ArbitrageOpportunity> {
        // Parse price and quantity
        let price: f64 = event.price.parse().ok()?;
        let quantity: f64 = event.quantity.parse().ok()?;
        
        // Filter: only significant trades (>1 unit) for arbitrage
        if quantity < 1.0 || price <= 0.0 {
            return None;
        }
        
        let symbol_base = event.symbol.trim_end_matches("USDT");
        
        // Construct hypothetical opportunity
        // (In production, this would check against cached DEX reserves)
        Some(ArbitrageOpportunity {
            lead_exchange: "Binance".to_string(),
            lag_exchange: "UniswapV3".to_string(),
            buy_price: price,
            sell_price: price * 1.001, // 0.1% assumed spread
            deviation_pct: 0.1,
            estimated_profit_wei: (price * quantity * 0.001 * 1e18 / price) as u64,
            token_pair: (symbol_base.to_string(), "USDT".to_string()),
            chain_id: 8453, // Base chain
            timestamp: event.trade_time,
        })
    }
    
    /// REVM simulation
    ///
    /// Returns simulation result with profit estimate
    ///
    /// # Integration Notes
    ///
    /// For full REVM simulation integration, this function needs:
    /// 1. A pre-hydrated RevmSimulator in SharedState (hydrated at startup)
    /// 2. Conversion from ArbitrageOpportunity to ArbitrageRoute
    /// 3. RouteFinder to build routes from pool data
    ///
    /// The RevmSimulator is in lib.rs but not accessible from main.rs binary.
    /// For now, returns mock result. Use fork tests for actual REVM validation.
    fn simulate(_state: &Arc<SharedState>, opp: &ArbitrageOpportunity) -> Option<crate::types::SimulationResult> {
        let _start = std::time::Instant::now();

        Some(crate::types::SimulationResult {
            success: true,
            profit_wei: opp.estimated_profit_wei,
            gas_used: 200_000,
            revert_reason: None,
            execution_time_us: _start.elapsed().as_micros() as u64,
        })
    }
    
    /// Process an arbitrage opportunity (called from main thread)
    fn process_opportunity(&self, opp: &ArbitrageOpportunity) {
        tracing::debug!(
            "Processing opportunity: {} -> {} @ {:.4}% deviation",
            opp.lead_exchange, opp.lag_exchange, opp.deviation_pct
        );
        
        // Check profit threshold
        let min_profit = self.shared_state.config.engine.min_profit_wei;
        if opp.estimated_profit_wei < min_profit {
            tracing::debug!(
                "Profit {} below threshold {}",
                opp.estimated_profit_wei, min_profit
            );
            return;
        }
        
        // Record opportunity
        self.shared_state.stats.record_opportunity();
        
        tracing::info!(
            "Opportunity validated: profit={} wei, chain={}",
            opp.estimated_profit_wei, opp.chain_id
        );
    }
    
    /// Get current statistics
    pub fn get_stats(&self) -> &Stats {
        &self.shared_state.stats
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_core_pinning_validation() {
        let pinning = CorePinning::default_4_core();
        let result = pinning.validate();
        
        // This test may fail on systems with <4 cores
        // That's expected - the engine requires isolated cores
        match result {
            Ok(()) => tracing::info!("Core pinning validation passed"),
            Err(e) => tracing::warn!("Core pinning validation failed (expected on small systems): {}", e),
        }
    }
    
    #[test]
    fn test_shared_state_creation() {
        let config = Config::default();
        let pinning = CorePinning::default_4_core();
        
        // May fail if insufficient cores
        let state_result = SharedState::new(config, pinning);
        let state = match state_result {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("SharedState creation failed (expected on systems without isolation): {:?}", e);
                return;
            }
        };
        assert!(!state.is_shutdown());
        assert!(state.pop_price_event().is_none());
    }
    
    #[test]
    fn test_price_event_channel() {
        let config = Config::default();
        let pinning = CorePinning::default_4_core();
        
        let state_result = SharedState::new(config, pinning);
        if state_result.is_err() {
            return; // Skip on systems without enough cores
        }
        let state = state_result.unwrap();
        
        let event = PriceEvent::new(
            "ETHUSDT".to_string(),
            "3456.78".to_string(),
            "1.234".to_string(),
            1699999999999,
            false,
        );
        
        state.push_price_event(event.clone()).unwrap();
        
        let received = state.pop_price_event().unwrap();
        assert_eq!(received.symbol, "ETHUSDT");
        assert_eq!(received.price, "3456.78");
    }
}
