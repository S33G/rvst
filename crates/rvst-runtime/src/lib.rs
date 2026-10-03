//! Allocation-free processing helpers. Construct and destroy channels off the audio thread.
mod recording;
mod telemetry;
pub use recording::*;
pub use telemetry::*;

#[derive(Debug, Clone, Copy)]
pub struct LinearSmoother {
    current: f64,
    target: f64,
    delta: f64,
    remaining: u32,
}
impl LinearSmoother {
    pub fn new(value: f64) -> Self {
        Self {
            current: value,
            target: value,
            delta: 0.0,
            remaining: 0,
        }
    }
    pub fn set_target(&mut self, target: f64, samples: u32) {
        if !target.is_finite() {
            return;
        }
        self.target = target;
        self.remaining = samples;
        if samples == 0 {
            self.current = target;
            self.delta = 0.0;
        } else {
            self.delta = (target - self.current) / f64::from(samples);
        }
    }
    pub fn next_sample(&mut self) -> f64 {
        if self.remaining > 0 {
            self.remaining -= 1;
            self.current = if self.remaining == 0 {
                self.target
            } else {
                self.current + self.delta
            };
        }
        self.current
    }
    pub fn current(&self) -> f64 {
        self.current
    }
}
