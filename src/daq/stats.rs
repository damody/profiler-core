/// Basic statistics result
#[derive(Debug, Clone)]
pub struct Stats {
    pub mean: f64,
    pub min: f64,
    pub max: f64,
    pub rms: f64,
}

/// Compute basic statistics for a channel's samples.
/// Mean is clamped to 0.0 if negative.
pub fn compute_stats(samples: &[f64]) -> Stats {
    if samples.is_empty() {
        return Stats {
            mean: 0.0,
            min: 0.0,
            max: 0.0,
            rms: 0.0,
        };
    }
    let len = samples.len() as f64;
    let mut sum = 0.0;
    let mut sum_sq = 0.0;
    let mut min = f64::MAX;
    let mut max = f64::MIN;
    for &v in samples {
        sum += v;
        sum_sq += v * v;
        if v < min { min = v; }
        if v > max { max = v; }
    }
    let mean = sum / len;
    let mean = if mean < 0.0 { 0.0 } else { mean };
    let rms = (sum_sq / len).sqrt();
    Stats { mean, min, max, rms }
}

/// Incremental statistics accumulator (Welford-style sums).
pub struct RunningStats {
    pub count: u64,
    pub sum: f64,
    pub sum_sq: f64,
    pub min: f64,
    pub max: f64,
}

impl RunningStats {
    pub fn new() -> Self {
        Self {
            count: 0,
            sum: 0.0,
            sum_sq: 0.0,
            min: f64::MAX,
            max: f64::MIN,
        }
    }

    pub fn update(&mut self, v: f64) {
        self.count += 1;
        self.sum += v;
        self.sum_sq += v * v;
        if v < self.min { self.min = v; }
        if v > self.max { self.max = v; }
    }

    pub fn update_slice(&mut self, data: &[f64]) {
        for &v in data {
            self.update(v);
        }
    }

    pub fn finalize(&self) -> Stats {
        if self.count == 0 {
            return Stats { mean: 0.0, min: 0.0, max: 0.0, rms: 0.0 };
        }
        let n = self.count as f64;
        let mean = self.sum / n;
        let mean = if mean < 0.0 { 0.0 } else { mean };
        let rms = (self.sum_sq / n).sqrt();
        Stats { mean, min: self.min, max: self.max, rms }
    }
}

/// Incremental power accumulator: accumulates sum(V*I) for average power.
pub struct RunningPower {
    pub sum_vi: f64,
    pub count: u64,
}

impl RunningPower {
    pub fn new() -> Self {
        Self { sum_vi: 0.0, count: 0 }
    }

    pub fn update(&mut self, voltage: &[f64], current: &[f64]) {
        let len = voltage.len().min(current.len());
        for i in 0..len {
            self.sum_vi += voltage[i] * current[i];
        }
        self.count += len as u64;
    }

    /// Average power in the same units as input (watts if V*A).
    pub fn finalize(&self) -> f64 {
        if self.count == 0 { 0.0 } else { self.sum_vi / self.count as f64 }
    }
}

/// Compute power: per-sample V*I product average (result in same units as input)
pub fn compute_power(voltage: &[f64], current: &[f64]) -> f64 {
    let len = voltage.len().min(current.len());
    if len == 0 {
        return 0.0;
    }
    let sum: f64 = voltage[..len]
        .iter()
        .zip(current[..len].iter())
        .map(|(v, i)| v * i)
        .sum();
    sum / len as f64
}
