/// Statistics for a data series
#[derive(Debug, Clone)]
pub struct SeriesStats {
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub peak_to_peak: f64,
    pub mean: f64,
    pub median: f64,
    pub std_dev: f64,
}

impl SeriesStats {
    /// Compute statistics from y-values, filtering out NaN.
    pub fn compute(y: &[f64]) -> Option<Self> {
        let mut vals: Vec<f64> = y.iter().copied().filter(|v| v.is_finite()).collect();
        if vals.is_empty() {
            return None;
        }

        let count = vals.len();
        let min = vals.iter().copied().fold(f64::INFINITY, f64::min);
        let max = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let peak_to_peak = max - min;
        let mean = vals.iter().sum::<f64>() / count as f64;

        vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = if count % 2 == 0 {
            (vals[count / 2 - 1] + vals[count / 2]) / 2.0
        } else {
            vals[count / 2]
        };

        let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / count as f64;
        let std_dev = variance.sqrt();

        Some(SeriesStats {
            count,
            min,
            max,
            peak_to_peak,
            mean,
            median,
            std_dev,
        })
    }

    /// Format as a multi-line report string.
    pub fn report(&self, label: &str) -> String {
        format!(
            "{}:\n  Count: {}\n  Min: {:.3}\n  Max: {:.3}\n  Peak-to-Peak: {:.3}\n  Mean: {:.3}\n  Median: {:.3}\n  Std Dev: {:.3}\n",
            label, self.count, self.min, self.max, self.peak_to_peak, self.mean, self.median, self.std_dev
        )
    }
}
