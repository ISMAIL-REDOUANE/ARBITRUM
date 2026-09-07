//! # TSC (Time Stamp Counter) Telemetry Module
//!
//! Hardware-cycle-accurate latency measurement using `rdtsc`.
//!
//! ## Why TSC Instead of OS Clocks?
//!
//! | Clock Source | Resolution | Syscall Overhead | Jitter |
//! |--------------|------------|------------------|--------|
//! | `std::time::Instant` | ~1ns | ~50-100ns | High |
//! | `QueryPerformanceCounter` | ~100ns | ~1µs | Medium |
//! | `rdtsc` | ~1 cycle | **~0** | **~0** |
//!
//! For sub-100µs latency targeting, OS clock overhead alone could consume
//! 1-5% of our budget with significant variance.
//!
//! ## TSC Characteristics
//!
//! - **Invariant TSC**: On modern x86_64 (Nehalem+), the TSC runs at a
//!   constant rate regardless of CPU frequency changes (P-states, Turbo Boost)
//! - **Serializing**: `lfence rdtsc` ensures all instructions
//!   before it complete before reading the counter
//! - **Low overhead**: ~20-30 cycles (~10-15ns on 3GHz)

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// TSC calibration state
static TSC_KHZ: AtomicU64 = AtomicU64::new(0);
static TSC_CALIBRATED: AtomicU64 = AtomicU64::new(0);

/// Number of samples to average for calibration
const CALIBRATION_SAMPLES: usize = 10;

/// Initialize TSC calibration
/// 
/// Must be called at startup (before pinned threads) to establish
/// the conversion factor between cycles and nanoseconds.
pub fn calibrate() {
    #[cfg(target_arch = "x86_64")]
    {
        if is_tsc_calibrated() {
            return;
        }
        
        let mut total_cycles: u64 = 0;
        let mut total_ns: u64 = 0;
        
        for _ in 0..CALIBRATION_SAMPLES {
            let start_tsc = unsafe { rdtsc_raw() };
            let start_instant = Instant::now();
            
            // Sleep for ~10ms using OS timer
            std::thread::sleep(Duration::from_millis(10));
            
            let end_tsc = unsafe { rdtsc_raw() };
            let elapsed = start_instant.elapsed();
            
            total_cycles += end_tsc - start_tsc;
            total_ns += elapsed.as_nanos() as u64;
        }
        
        // Calculate TSC frequency in kHz
        // cycles per millisecond = cycles / (ns / 1_000_000)
        let khz = if total_ns > 0 {
            (total_cycles * 1_000_000) / total_ns
        } else {
            3_000_000 // Default 3GHz fallback
        };
        
        TSC_KHZ.store(khz, Ordering::Relaxed);
        TSC_CALIBRATED.store(1, Ordering::Relaxed);
        
        tracing::info!(
            "TSC calibrated: {} MHz ({} cycles/ms)",
            khz / 1000,
            khz
        );
    }
    
    #[cfg(not(target_arch = "x86_64"))]
    {
        tracing::warn!("TSC calibration skipped: not on x86_64");
    }
}

/// Check if TSC has been calibrated
#[inline]
pub fn is_tsc_calibrated() -> bool {
    TSC_CALIBRATED.load(Ordering::Relaxed) == 1
}

/// Get TSC frequency in kHz
#[inline]
pub fn tsc_khz() -> u64 {
    TSC_KHZ.load(Ordering::Relaxed)
}

/// Raw TSC read (unsafe, low-level)
#[cfg(target_arch = "x86_64")]
unsafe fn rdtsc_raw() -> u64 {
    std::arch::x86_64::_rdtsc()
}

#[cfg(not(target_arch = "x86_64"))]
unsafe fn rdtsc_raw() -> u64 {
    0
}

/// Read the current TSC value (serializing)
/// 
/// Uses `lfence` + `rdtsc` on x86_64 for proper serialization:
/// - `lfence` ensures all prior instructions complete
/// - `rdtsc` reads the timestamp
#[inline]
pub fn rdtsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            std::arch::x86_64::_mm_lfence();
            std::arch::x86_64::_rdtsc()
        }
    }
    
    #[cfg(not(target_arch = "x86_64"))]
    {
        let instant = Instant::now();
        instant.elapsed().as_nanos() as u64
    }
}

/// Convert cycles to nanoseconds
#[inline]
pub fn cycles_to_ns(cycles: u64) -> u64 {
    let khz = TSC_KHZ.load(Ordering::Relaxed);
    if khz == 0 {
        return 0;
    }
    // cycles * 1_000_000 / khz = nanoseconds
    cycles * 1_000_000 / khz
}

/// Convert cycles to microseconds (fractions as float)
#[inline]
pub fn cycles_to_us_f64(cycles: u64) -> f64 {
    let khz = TSC_KHZ.load(Ordering::Relaxed);
    if khz == 0 {
        return 0.0;
    }
    cycles as f64 / khz as f64
}

/// TSC measurement guard
///
/// RAII guard that captures end TSC on drop.
/// Usage:
/// ```rust
/// use lead_lag_arbitrage::tsc::TscGuard;
/// let _guard = TscGuard::new(); // captures start
/// // ... critical path ...
/// // on drop: captures end, calculates latency
/// ```
pub struct TscGuard {
    start: u64,
    telemetry: Option<Arc<TelemetryChannel>>,
}

impl Default for TscGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl TscGuard {
    /// Create new guard and capture start TSC
    pub fn new() -> Self {
        Self {
            start: rdtsc(),
            telemetry: None,
        }
    }
    
    /// Create with telemetry channel for async logging
    pub fn with_telemetry(telemetry: Arc<TelemetryChannel>) -> Self {
        Self {
            start: rdtsc(),
            telemetry: Some(telemetry),
        }
    }
    
    /// Get elapsed cycles so far (without ending)
    #[inline]
    pub fn elapsed_cycles(&self) -> u64 {
        rdtsc().wrapping_sub(self.start)
    }
    
    /// Get elapsed nanoseconds so far
    #[inline]
    pub fn elapsed_ns(&self) -> u64 {
        cycles_to_ns(self.elapsed_cycles())
    }
}

impl Drop for TscGuard {
    fn drop(&mut self) {
        let end = rdtsc();
        let cycles = end.wrapping_sub(self.start);
        
        if let Some(ref telemetry) = self.telemetry {
            // Non-blocking send to telemetry thread
            telemetry.push_latency(cycles);
        }
    }
}

/// Telemetry data point
#[derive(Debug, Clone)]
pub struct LatencySample {
    pub cycles: u64,
    pub ns: u64,
    pub timestamp: u64,
}

impl LatencySample {
    pub fn new(cycles: u64) -> Self {
        let ns = cycles_to_ns(cycles);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        Self { cycles, ns, timestamp }
    }
}

/// Channel for async telemetry logging
/// 
/// Uses a lock-free SPSC channel to send latency samples
/// to a background thread that formats and logs them.
pub struct TelemetryChannel {
    samples: crossbeam_channel::Sender<LatencySample>,
}

impl TelemetryChannel {
    /// Create new telemetry channel
    pub fn new(capacity: usize) -> (Self, crossbeam_channel::Receiver<LatencySample>) {
        let (tx, rx) = crossbeam_channel::bounded(capacity);
        (Self { samples: tx }, rx)
    }
    
    /// Push a latency sample (non-blocking)
    #[inline]
    pub fn push_latency(&self, cycles: u64) {
        let sample = LatencySample::new(cycles);
        // Try non-blocking send - if channel is full, drop the sample
        let _ = self.samples.try_send(sample);
    }
}

/// Shared telemetry statistics
pub struct TelemetryStats {
    total_samples: AtomicU64,
    avg_cycles: AtomicU64,
    min_cycles: AtomicU64,
    max_cycles: AtomicU64,
}

impl Default for TelemetryStats {
    fn default() -> Self {
        Self::new()
    }
}

impl TelemetryStats {
    pub fn new() -> Self {
        Self {
            total_samples: AtomicU64::new(0),
            avg_cycles: AtomicU64::new(0),
            min_cycles: AtomicU64::new(u64::MAX),
            max_cycles: AtomicU64::new(0),
        }
    }
    
    pub fn record_sample(&self, _avg: u64, min: u64, max: u64) {
        self.total_samples.fetch_add(1, Ordering::Relaxed);
        
        // Update running min
        let current_min = self.min_cycles.load(Ordering::Relaxed);
        if min < current_min {
            self.min_cycles.store(min, Ordering::Relaxed);
        }
        
        // Update running max
        let current_max = self.max_cycles.load(Ordering::Relaxed);
        if max > current_max {
            self.max_cycles.store(max, Ordering::Relaxed);
        }
    }
    
    pub fn total(&self) -> u64 {
        self.total_samples.load(Ordering::Relaxed)
    }
    
    pub fn avg_ns(&self) -> u64 {
        cycles_to_ns(self.avg_cycles.load(Ordering::Relaxed))
    }
    
    pub fn min_ns(&self) -> u64 {
        cycles_to_ns(self.min_cycles.load(Ordering::Relaxed))
    }
    
    pub fn max_ns(&self) -> u64 {
        cycles_to_ns(self.max_cycles.load(Ordering::Relaxed))
    }
}

/// Spawn the telemetry logging thread
/// 
/// This thread consumes latency samples from the channel
/// and logs them asynchronously (never blocks the hot path).
pub fn spawn_telemetry_logger(
    rx: crossbeam_channel::Receiver<LatencySample>,
    stats: Arc<TelemetryStats>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("telemetry".to_string())
        .spawn(move || {
            // Process samples in batches for efficiency
            let mut batch = Vec::with_capacity(100);
            
            loop {
                // Non-blocking drain of channel
                while let Ok(sample) = rx.try_recv() {
                    batch.push(sample);
                    
                    // Process in batches of 100
                    if batch.len() >= 100 {
                        process_batch(&batch, &stats);
                        batch.clear();
                    }
                }
                
                // If no samples, yield to avoid busy-waiting
                if batch.is_empty() {
                    std::thread::yield_now();
                }
            }
        })
        .expect("Failed to spawn telemetry thread")
}

fn process_batch(samples: &[LatencySample], stats: &Arc<TelemetryStats>) {
    if samples.is_empty() {
        return;
    }
    
    // Calculate statistics
    let total_cycles: u64 = samples.iter().map(|s| s.cycles).sum();
    let avg_cycles = total_cycles / samples.len() as u64;
    
    // Find min/max
    let min_sample = samples.iter().min_by_key(|s| s.cycles).unwrap();
    let max_sample = samples.iter().max_by_key(|s| s.cycles).unwrap();
    
    // Update running statistics (thread-safe)
    stats.record_sample(avg_cycles, min_sample.cycles, max_sample.cycles);
    
    // Log summary
    tracing::debug!(
        "Tick-to-Trade batch: avg={}ns, min={}ns, max={}ns ({} samples)",
        cycles_to_ns(avg_cycles),
        cycles_to_ns(min_sample.cycles),
        cycles_to_ns(max_sample.cycles),
        samples.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_calibrate() {
        calibrate();
        assert!(is_tsc_calibrated());
        assert!(tsc_khz() > 0);
    }
    
    #[test]
    fn test_rdtsc_different() {
        calibrate();
        let t1 = rdtsc();
        let t2 = rdtsc();
        // TSC should always increase (or wrap, but unlikely in test)
        assert!(t2 >= t1);
    }
    
    #[test]
    fn test_cycles_to_ns() {
        calibrate();
        let cycles = tsc_khz(); // 1ms worth of cycles since tsc_khz is cycles/ms
        let ns = cycles_to_ns(cycles);
        // Should be approximately 1,000,000 ns (1ms) with wide tolerance for VM/noisy hardware
        assert!(ns > 800_000 && ns < 1_300_000, "Expected ~1ms but got {} ns (cycles: {})", ns, cycles);
    }
}
