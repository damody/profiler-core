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
