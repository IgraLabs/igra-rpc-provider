//! Metrics collection and aggregation for transaction performance analysis.
//!
//! This module provides two complementary metrics systems:
//! - **Sliding window metrics**: Track recent performance over a time window (e.g., last 60s)
//! - **Cumulative metrics**: Aggregate all-time statistics since process start
//!
//! # Performance Characteristics
//! - Uses HDR Histogram for accurate percentile calculations (p50, p90, p95, p99)
//! - Sliding window uses VecDeque for O(1) push/pop operations
//! - Fixed-size min-heap tracks top N slowest transactions efficiently
//! - Saturating arithmetic prevents overflow in long-running processes

use hdrhistogram::Histogram;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::time::{Duration, Instant};

/// A single RPC transaction sample with timing and identification.
#[derive(Clone)]
pub struct RpcSample {
    pub at: Instant,
    pub latency_micros: u64,
    pub id: Option<String>,
    pub hash: Option<String>,
}

#[derive(Clone)]
pub struct MiningSample {
    pub at: Instant,
    pub duration_micros: u64,
}

#[derive(Clone)]
pub struct ErrorSample {
    pub at: Instant,
    pub message: String,
}

#[derive(Clone)]
pub struct CapacitySample {
    pub at: Instant,
    pub capacity: u64,
}

pub struct SlidingWindowMetrics {
    window: Duration,
    rpc_samples: VecDeque<RpcSample>,
    mining_samples: VecDeque<MiningSample>,
    error_samples: VecDeque<ErrorSample>,
    capacity_samples: VecDeque<CapacitySample>,
}

impl SlidingWindowMetrics {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            rpc_samples: VecDeque::new(),
            mining_samples: VecDeque::new(),
            error_samples: VecDeque::new(),
            capacity_samples: VecDeque::new(),
        }
    }

    pub fn add_rpc_success(&mut self, sample: RpcSample) {
        self.rpc_samples.push_back(sample);
    }

    pub fn add_mining_success(&mut self, duration_micros: u64) {
        self.mining_samples.push_back(MiningSample {
            at: Instant::now(),
            duration_micros,
        });
    }

    pub fn add_error(&mut self, message: String) {
        self.error_samples.push_back(ErrorSample {
            at: Instant::now(),
            message,
        });
    }

    pub fn add_capacity(&mut self, capacity: u64) {
        self.capacity_samples.push_back(CapacitySample {
            at: Instant::now(),
            capacity,
        });
    }

    pub fn purge_expired(&mut self, now: Instant) {
        while let Some(front) = self.rpc_samples.front() {
            if now.duration_since(front.at) > self.window {
                self.rpc_samples.pop_front();
            } else {
                break;
            }
        }
        while let Some(front) = self.mining_samples.front() {
            if now.duration_since(front.at) > self.window {
                self.mining_samples.pop_front();
            } else {
                break;
            }
        }
        while let Some(front) = self.error_samples.front() {
            if now.duration_since(front.at) > self.window {
                self.error_samples.pop_front();
            } else {
                break;
            }
        }
        while let Some(front) = self.capacity_samples.front() {
            if now.duration_since(front.at) > self.window {
                self.capacity_samples.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn summarize(&self, top_slowest: usize) -> Option<WindowSummary> {
        if self.rpc_samples.is_empty() {
            return None;
        }
        // Build histogram for RPC latencies (microseconds)
        // Use dynamic max bound based on data; fallback to 100_000 µs = 100 ms
        let mut max_val = 100_000u64;
        for s in &self.rpc_samples {
            if s.latency_micros > max_val {
                max_val = s.latency_micros;
            }
        }
        let mut hist = Histogram::<u64>::new_with_max(max_val.max(1), 3).ok()?;
        for s in &self.rpc_samples {
            let _ = hist.record(s.latency_micros);
        }
        let p10 = hist.value_at_quantile(0.10);
        let p25 = hist.value_at_quantile(0.25);
        let p50 = hist.value_at_quantile(0.50);
        let p90 = hist.value_at_quantile(0.90);
        let p95 = hist.value_at_quantile(0.95);
        let p99 = hist.value_at_quantile(0.99);
        let min = hist.min();
        let max = hist.max();
        let mean = hist.mean();

        #[allow(clippy::cast_possible_truncation)]
        let success = self.rpc_samples.len() as u64;
        #[allow(clippy::cast_possible_truncation)]
        let errors = self.error_samples.len() as u64;
        let tps = if p50 == 0 {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            let p50_f64 = p50 as f64;
            1_000_000.0f64 / p50_f64 // 1 / p50 seconds; p50 is in µs
        };

        // Mining histogram (optional)
        let (
            mining_success,
            mining_p10,
            mining_p25,
            mining_p50,
            mining_p90,
            mining_p95,
            mining_p99,
            mining_min,
            mining_max,
            mining_mean,
        ) = if self.mining_samples.is_empty() {
            (None, None, None, None, None, None, None, None, None, None)
        } else {
            let mut max_val = 100_000u64;
            for s in &self.mining_samples {
                if s.duration_micros > max_val {
                    max_val = s.duration_micros;
                }
            }
            let mut mhist = Histogram::<u64>::new_with_max(max_val.max(1), 3).ok()?;
            for s in &self.mining_samples {
                let _ = mhist.record(s.duration_micros);
            }
            let ms = self.mining_samples.len() as u64;
            (
                Some(ms),
                Some(mhist.value_at_quantile(0.10)),
                Some(mhist.value_at_quantile(0.25)),
                Some(mhist.value_at_quantile(0.50)),
                Some(mhist.value_at_quantile(0.90)),
                Some(mhist.value_at_quantile(0.95)),
                Some(mhist.value_at_quantile(0.99)),
                Some(mhist.min()),
                Some(mhist.max()),
                Some(mhist.mean()),
            )
        };

        // Capacity min/avg
        let (capacity_min, capacity_avg) = if self.capacity_samples.is_empty() {
            (None, None)
        } else {
            let mut min_val = u64::MAX;
            let mut sum: u128 = 0;
            for c in &self.capacity_samples {
                if c.capacity < min_val {
                    min_val = c.capacity;
                }
                sum = sum.saturating_add(u128::from(c.capacity));
            }
            #[allow(clippy::cast_possible_truncation, clippy::arithmetic_side_effects)]
            let avg = (sum / (self.capacity_samples.len() as u128)) as u64;
            (Some(min_val), Some(avg))
        };

        // Top errors by frequency
        let mut freq: HashMap<&str, usize> = HashMap::new();
        for e in &self.error_samples {
            let entry = freq.entry(e.message.as_str()).or_insert(0);
            #[allow(clippy::arithmetic_side_effects)]
            {
                *entry += 1;
            }
        }
        let mut top_errors: Vec<(String, usize)> =
            freq.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        top_errors.sort_by_key(|b| std::cmp::Reverse(b.1));
        if top_errors.len() > 5 {
            top_errors.truncate(5);
        }

        // Top slowest RPC successes
        let mut slowest: Vec<(Option<String>, Option<String>, u64)> = self
            .rpc_samples
            .iter()
            .map(|s| (s.id.clone(), s.hash.clone(), s.latency_micros))
            .collect();
        slowest.sort_by_key(|b| std::cmp::Reverse(b.2));
        if slowest.len() > top_slowest {
            slowest.truncate(top_slowest);
        }

        Some(WindowSummary {
            success,
            errors,
            p10,
            p25,
            p50,
            p90,
            p95,
            p99,
            min,
            max,
            mean,
            tps,
            capacity_min,
            capacity_avg,
            top_errors,
            slowest,
            mining_success,
            mining_p10,
            mining_p25,
            mining_p50,
            mining_p90,
            mining_p95,
            mining_p99,
            mining_min,
            mining_max,
            mining_mean,
        })
    }
}

pub struct CumulativeMetrics {
    hist: Histogram<u64>,
    successes: u64,
    errors: u64,
    capacity_min: Option<u64>,
    capacity_sum: u128,
    capacity_count: u64,
    top_slowest: FixedMinHeap<SlowItem>,
    started_at: Instant,
    // Mining cumulative
    mining_hist: Option<Histogram<u64>>,
    mining_successes: u64,
}

impl CumulativeMetrics {
    pub fn new(top_n: usize) -> Self {
        // Start with generous bound to avoid overflow; will recreate if needed
        let hist = Histogram::<u64>::new_with_max(60_000_000, 3).unwrap_or_else(|_| {
            // Fallback to 100ms if creation fails
            Histogram::<u64>::new_with_max(100_000, 3).expect("histogram creation failed")
        });
        Self {
            hist,
            successes: 0,
            errors: 0,
            capacity_min: None,
            capacity_sum: 0,
            capacity_count: 0,
            top_slowest: FixedMinHeap::new(top_n),
            started_at: Instant::now(),
            mining_hist: None,
            mining_successes: 0,
        }
    }

    pub fn record_success(
        &mut self,
        latency_micros: u64,
        id: Option<String>,
        hash: Option<String>,
    ) {
        if self.hist.record(latency_micros).is_err() {
            // Recreate with larger bound and try again (best-effort)
            let new_max = latency_micros.saturating_mul(2).max(self.hist.max());
            if let Ok(new_hist) = Histogram::<u64>::new_with_max(new_max, 3) {
                // We cannot migrate old counts; keep the old hist as-is and continue recording new
                self.hist = new_hist;
                let _ = self.hist.record(latency_micros);
            }
        }
        self.successes = self.successes.saturating_add(1);
        self.top_slowest.push(SlowItem {
            latency_micros,
            id,
            hash,
        });
    }

    pub fn record_error(&mut self) {
        self.errors = self.errors.saturating_add(1);
    }

    pub fn record_capacity(&mut self, capacity: u64) {
        self.capacity_min = Some(match self.capacity_min {
            Some(m) => m.min(capacity),
            None => capacity,
        });
        self.capacity_sum = self.capacity_sum.saturating_add(u128::from(capacity));
        self.capacity_count = self.capacity_count.saturating_add(1);
    }

    pub fn record_mining_success(&mut self, duration_micros: u64) {
        if self.mining_hist.is_none() {
            // Initialize with a generous bound similar to RPC hist
            self.mining_hist = Some(
                Histogram::<u64>::new_with_max(60_000_000, 3).unwrap_or_else(|_| {
                    Histogram::<u64>::new_with_max(100_000, 3).expect("histogram creation failed")
                }),
            );
        }
        if let Some(ref mut mhist) = self.mining_hist {
            if mhist.record(duration_micros).is_err() {
                // Recreate with larger bound and try again (best-effort)
                let new_max = duration_micros.saturating_mul(2).max(mhist.max());
                if let Ok(new_hist) = Histogram::<u64>::new_with_max(new_max, 3) {
                    *mhist = new_hist;
                    let _ = mhist.record(duration_micros);
                }
            }
        }
        self.mining_successes = self.mining_successes.saturating_add(1);
    }

    pub fn summarize(&self) -> Option<CumulativeSummary> {
        if self.successes == 0 {
            return None;
        }
        let p10 = self.hist.value_at_quantile(0.10);
        let p25 = self.hist.value_at_quantile(0.25);
        let p50 = self.hist.value_at_quantile(0.50);
        let p90 = self.hist.value_at_quantile(0.90);
        let p95 = self.hist.value_at_quantile(0.95);
        let p99 = self.hist.value_at_quantile(0.99);
        let min = self.hist.min();
        let max = self.hist.max();
        let mean = self.hist.mean();
        let tps = if p50 == 0 {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            let p50_f64 = p50 as f64;
            1_000_000.0f64 / p50_f64
        };
        let capacity_avg = if self.capacity_count == 0 {
            None
        } else {
            #[allow(clippy::cast_possible_truncation, clippy::arithmetic_side_effects)]
            Some((self.capacity_sum / u128::from(self.capacity_count)) as u64)
        };
        // Mining cumulative summary (optional)
        let (
            mining_success,
            mining_p10,
            mining_p25,
            mining_p50,
            mining_p90,
            mining_p95,
            mining_p99,
            mining_min,
            mining_max,
            mining_mean,
        ) = self
            .mining_hist
            .as_ref()
            .map(|mhist| {
                (
                    Some(self.mining_successes),
                    Some(mhist.value_at_quantile(0.10)),
                    Some(mhist.value_at_quantile(0.25)),
                    Some(mhist.value_at_quantile(0.50)),
                    Some(mhist.value_at_quantile(0.90)),
                    Some(mhist.value_at_quantile(0.95)),
                    Some(mhist.value_at_quantile(0.99)),
                    Some(mhist.min()),
                    Some(mhist.max()),
                    Some(mhist.mean()),
                )
            })
            .unwrap_or((None, None, None, None, None, None, None, None, None, None));
        Some(CumulativeSummary {
            success: self.successes,
            errors: self.errors,
            p10,
            p25,
            p50,
            p90,
            p95,
            p99,
            min,
            max,
            mean,
            tps,
            capacity_min: self.capacity_min,
            capacity_avg,
            slowest: self.top_slowest.to_sorted_desc(),
            elapsed_secs: self.started_at.elapsed().as_secs(),
            mining_success,
            mining_p10,
            mining_p25,
            mining_p50,
            mining_p90,
            mining_p95,
            mining_p99,
            mining_min,
            mining_max,
            mining_mean,
        })
    }
}

pub struct WindowSummary {
    pub success: u64,
    pub errors: u64,
    pub p10: u64,
    pub p25: u64,
    pub p50: u64,
    pub p90: u64,
    pub p95: u64,
    pub p99: u64,
    pub min: u64,
    pub max: u64,
    pub mean: f64,
    pub tps: f64,
    pub capacity_min: Option<u64>,
    pub capacity_avg: Option<u64>,
    pub top_errors: Vec<(String, usize)>,
    pub slowest: Vec<(Option<String>, Option<String>, u64)>,
    // Optional mining stats for the window
    pub mining_success: Option<u64>,
    pub mining_p10: Option<u64>,
    pub mining_p25: Option<u64>,
    pub mining_p50: Option<u64>,
    pub mining_p90: Option<u64>,
    pub mining_p95: Option<u64>,
    pub mining_p99: Option<u64>,
    pub mining_min: Option<u64>,
    pub mining_max: Option<u64>,
    pub mining_mean: Option<f64>,
}

pub struct CumulativeSummary {
    pub success: u64,
    pub errors: u64,
    pub p10: u64,
    pub p25: u64,
    pub p50: u64,
    pub p90: u64,
    pub p95: u64,
    pub p99: u64,
    pub min: u64,
    pub max: u64,
    pub mean: f64,
    pub tps: f64,
    pub capacity_min: Option<u64>,
    pub capacity_avg: Option<u64>,
    pub slowest: Vec<SlowItem>,
    pub elapsed_secs: u64,
    // Optional mining stats cumulative
    pub mining_success: Option<u64>,
    pub mining_p10: Option<u64>,
    pub mining_p25: Option<u64>,
    pub mining_p50: Option<u64>,
    pub mining_p90: Option<u64>,
    pub mining_p95: Option<u64>,
    pub mining_p99: Option<u64>,
    pub mining_min: Option<u64>,
    pub mining_max: Option<u64>,
    pub mining_mean: Option<f64>,
}

#[derive(Clone)]
pub struct SlowItem {
    pub latency_micros: u64,
    pub id: Option<String>,
    pub hash: Option<String>,
}

impl Eq for SlowItem {}
impl PartialEq for SlowItem {
    fn eq(&self, other: &Self) -> bool {
        self.latency_micros == other.latency_micros
    }
}
impl PartialOrd for SlowItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for SlowItem {
    fn cmp(&self, other: &Self) -> Ordering {
        self.latency_micros.cmp(&other.latency_micros)
    }
}

/// Fixed-size min-heap (keeps top N largest items by latency)
pub struct FixedMinHeap<T: Ord> {
    heap: BinaryHeap<std::cmp::Reverse<T>>,
    cap: usize,
}

impl<T: Ord + Clone> FixedMinHeap<T> {
    pub fn new(cap: usize) -> Self {
        Self {
            heap: BinaryHeap::with_capacity(cap),
            cap,
        }
    }
    pub fn push(&mut self, item: T) {
        if self.cap == 0 {
            return;
        }
        if self.heap.len() < self.cap {
            self.heap.push(std::cmp::Reverse(item));
        } else if let Some(mut smallest) = self.heap.peek_mut() {
            if item > smallest.0 {
                *smallest = std::cmp::Reverse(item);
            }
        }
    }
    pub fn to_sorted_desc(&self) -> Vec<T> {
        let mut v: Vec<T> = self.heap.iter().map(|r| r.0.clone()).collect();
        v.sort(); // ascending
        v.reverse(); // descending
        v
    }
}
