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

/// Pearson correlation over rows where both series are finite. None if < 2 pairs
/// or a series has zero variance.
pub fn pearson(a: &[f64], b: &[f64]) -> Option<f64> {
    let mut n = 0usize;
    let (mut sx, mut sy) = (0.0, 0.0);
    for (&x, &y) in a.iter().zip(b.iter()) {
        if x.is_finite() && y.is_finite() {
            n += 1;
            sx += x;
            sy += y;
        }
    }
    if n < 2 {
        return None;
    }
    let (mx, my) = (sx / n as f64, sy / n as f64);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (&x, &y) in a.iter().zip(b.iter()) {
        if x.is_finite() && y.is_finite() {
            let (dx, dy) = (x - mx, y - my);
            sxy += dx * dy;
            sxx += dx * dx;
            syy += dy * dy;
        }
    }
    let denom = (sxx * syy).sqrt();
    if denom == 0.0 {
        None
    } else {
        Some(sxy / denom)
    }
}

/// Median and MAD (median absolute deviation) of the finite values.
pub fn median_mad(vals: &[f64]) -> Option<(f64, f64)> {
    let mut v: Vec<f64> = vals.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let med = v[v.len() / 2];
    let mut d: Vec<f64> = v.iter().map(|x| (x - med).abs()).collect();
    d.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some((med, d[d.len() / 2]))
}

#[cfg(test)]
mod moved_tests {
    use super::*;

    #[test]
    fn pearson_correlated_and_anti() {
        let a = [1.0, 2.0, 3.0, 4.0, 5.0];
        let b = [2.0, 4.0, 6.0, 8.0, 10.0]; // 2a → r = +1
        let c = [5.0, 4.0, 3.0, 2.0, 1.0];  // −a → r = −1
        assert!((pearson(&a, &b).unwrap() - 1.0).abs() < 1e-9);
        assert!((pearson(&a, &c).unwrap() + 1.0).abs() < 1e-9);
    }

    #[test]
    fn pearson_zero_variance_is_none() {
        let a = [1.0, 1.0, 1.0];
        let b = [1.0, 2.0, 3.0];
        assert!(pearson(&a, &b).is_none());
    }

    #[test]
    fn median_mad_basic() {
        // sorted 1,2,3,4,100 → median 3; abs devs 2,1,0,1,97 → sorted 0,1,1,2,97 → MAD 1
        let (m, d) = median_mad(&[3.0, 1.0, 4.0, 2.0, 100.0]).unwrap();
        assert_eq!(m, 3.0);
        assert_eq!(d, 1.0);
    }
}
