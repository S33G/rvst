use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct TelemetryEvent {
    pub code: u32,
    pub sample_clock: u64,
    pub value: f64,
    pub detail: u64,
}
impl TelemetryEvent {
    pub fn counter(code: u32, value: u64) -> Self {
        Self {
            code,
            sample_clock: 0,
            value: value as f64,
            detail: value,
        }
    }
}
pub struct RtTelemetry {
    producer: rtrb::Producer<TelemetryEvent>,
    dropped: Arc<AtomicU64>,
}
pub struct TelemetryReader {
    consumer: rtrb::Consumer<TelemetryEvent>,
    dropped: Arc<AtomicU64>,
}
pub fn telemetry_channel(capacity: usize) -> (RtTelemetry, TelemetryReader) {
    let (producer, consumer) = rtrb::RingBuffer::new(capacity.max(1));
    let dropped = Arc::new(AtomicU64::new(0));
    (
        RtTelemetry {
            producer,
            dropped: dropped.clone(),
        },
        TelemetryReader { consumer, dropped },
    )
}
impl RtTelemetry {
    pub fn emit(&mut self, event: TelemetryEvent) -> bool {
        if self.producer.push(event).is_ok() {
            true
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            false
        }
    }
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}
impl TelemetryReader {
    pub fn pop(&mut self) -> Option<TelemetryEvent> {
        self.consumer.pop().ok()
    }
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}
/// Logarithmic callback histogram. Each counter update is bounded and lock-free.
#[derive(Debug)]
pub struct Metrics {
    callbacks: AtomicU64,
    total_ns: AtomicU64,
    max_ns: AtomicU64,
    misses: AtomicU64,
    ui_frames: AtomicU64,
    ui_ns: AtomicU64,
    histogram: [AtomicU64; 64],
}
impl Default for Metrics {
    fn default() -> Self {
        Self {
            callbacks: AtomicU64::new(0),
            total_ns: AtomicU64::new(0),
            max_ns: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            ui_frames: AtomicU64::new(0),
            ui_ns: AtomicU64::new(0),
            histogram: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub callbacks: u64,
    pub mean_callback_ns: f64,
    pub max_callback_ns: u64,
    pub deadline_misses: u64,
    pub p50_upper_ns: u64,
    pub p95_upper_ns: u64,
    pub p99_upper_ns: u64,
    pub ui_frames: u64,
    pub mean_ui_ns: f64,
}
impl Metrics {
    pub fn record_callback(&self, elapsed_ns: u64, budget_ns: u64) {
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        self.total_ns.fetch_add(elapsed_ns, Ordering::Relaxed);
        self.max_ns.fetch_max(elapsed_ns, Ordering::Relaxed);
        if elapsed_ns > budget_ns {
            self.misses.fetch_add(1, Ordering::Relaxed);
        }
        let bucket = (64 - elapsed_ns.leading_zeros()).min(63) as usize;
        self.histogram[bucket].fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_ui_frame(&self, ns: u64) {
        self.ui_frames.fetch_add(1, Ordering::Relaxed);
        self.ui_ns.fetch_add(ns, Ordering::Relaxed);
    }
    pub fn snapshot(&self) -> MetricsSnapshot {
        let count = self.callbacks.load(Ordering::Relaxed);
        let ui = self.ui_frames.load(Ordering::Relaxed);
        let hist = self.histogram.each_ref().map(|v| v.load(Ordering::Relaxed));
        let percentile = |p: f64| {
            let wanted = (count as f64 * p).ceil() as u64;
            let mut n = 0;
            if wanted == 0 {
                return 0;
            }
            for (i, v) in hist.iter().enumerate() {
                n += v;
                if n >= wanted {
                    return if i == 63 {
                        u64::MAX
                    } else {
                        (1u64 << i).saturating_sub(1)
                    };
                }
            }
            u64::MAX
        };
        MetricsSnapshot {
            callbacks: count,
            mean_callback_ns: self.total_ns.load(Ordering::Relaxed) as f64 / count.max(1) as f64,
            max_callback_ns: self.max_ns.load(Ordering::Relaxed),
            deadline_misses: self.misses.load(Ordering::Relaxed),
            p50_upper_ns: percentile(0.5),
            p95_upper_ns: percentile(0.95),
            p99_upper_ns: percentile(0.99),
            ui_frames: ui,
            mean_ui_ns: self.ui_ns.load(Ordering::Relaxed) as f64 / ui.max(1) as f64,
        }
    }
}
