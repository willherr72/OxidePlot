/// Math operation types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathOp {
    Add,
    Subtract,
    Multiply,
    Divide,
}

impl MathOp {
    pub fn symbol(&self) -> &'static str {
        match self {
            MathOp::Add => "+",
            MathOp::Subtract => "-",
            MathOp::Multiply => "\u{00d7}",
            MathOp::Divide => "\u{00f7}",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            MathOp::Add => "Add (+)",
            MathOp::Subtract => "Subtract (-)",
            MathOp::Multiply => "Multiply (\u{00d7})",
            MathOp::Divide => "Divide (\u{00f7})",
        }
    }
}

/// Result of a math operation between two series.
pub struct MathResult {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub matched_count: usize,
    pub total_possible: usize,
}

/// Perform a math operation between two data series.
/// Aligns on matching x-values (within tolerance) and applies the operation.
/// Uses binary search for O(n log m) performance instead of O(n*m).
pub fn perform_math(
    x1: &[f64],
    y1: &[f64],
    x2: &[f64],
    y2: &[f64],
    op: MathOp,
    tolerance: f64,
) -> Result<MathResult, String> {
    // Build sorted index of x2 for binary search
    let mut sorted_indices: Vec<usize> = (0..x2.len()).collect();
    sorted_indices.sort_by(|&a, &b| x2[a].partial_cmp(&x2[b]).unwrap_or(std::cmp::Ordering::Equal));
    let sorted_x2: Vec<f64> = sorted_indices.iter().map(|&i| x2[i]).collect();

    let mut common_x = Vec::new();
    let mut common_y = Vec::new();
    let mut matched = 0usize;

    for (i, &xv) in x1.iter().enumerate() {
        // Binary search for closest x value in sorted_x2
        let pos = sorted_x2.partition_point(|&v| v < xv);

        // Check the candidate at pos and pos-1 (the two nearest neighbors)
        let mut best_j = None;
        let mut best_diff = f64::INFINITY;

        for &candidate in &[pos.wrapping_sub(1), pos] {
            if candidate < sorted_x2.len() {
                let diff = (sorted_x2[candidate] - xv).abs();
                if diff < best_diff {
                    best_diff = diff;
                    best_j = Some(sorted_indices[candidate]);
                }
            }
        }

        if let Some(j) = best_j {
            if best_diff <= tolerance {
                let result = match op {
                    MathOp::Add => y1[i] + y2[j],
                    MathOp::Subtract => y1[i] - y2[j],
                    MathOp::Multiply => y1[i] * y2[j],
                    MathOp::Divide => {
                        if y2[j].abs() < f64::EPSILON {
                            f64::NAN
                        } else {
                            y1[i] / y2[j]
                        }
                    }
                };
                common_x.push(xv);
                common_y.push(result);
                matched += 1;
            }
        }
    }

    if matched == 0 {
        return Err("No matching x-values found. Cannot perform math operation.".to_string());
    }

    let total_possible = x1.len().min(x2.len());

    Ok(MathResult {
        x: common_x,
        y: common_y,
        matched_count: matched,
        total_possible,
    })
}

/// Centered simple moving average, window >= 1, clamped at the edges.
pub fn moving_average(ys: &[f64], window: usize) -> Vec<f64> {
    let n = ys.len();
    let w = window.max(1);
    let half = w / 2;
    (0..n).map(|i| {
        let lo = i.saturating_sub(half);
        let hi = (i + half + 1).min(n);
        let slice = &ys[lo..hi];
        slice.iter().sum::<f64>() / slice.len() as f64
    }).collect()
}

/// Numerical dy/dx: central difference interior, forward/backward at the ends.
pub fn derivative(xs: &[f64], ys: &[f64]) -> Vec<f64> {
    let n = ys.len().min(xs.len());
    if n == 0 { return vec![]; }
    if n == 1 { return vec![0.0]; }
    (0..n).map(|i| {
        if i == 0 {
            (ys[1] - ys[0]) / (xs[1] - xs[0])
        } else if i == n - 1 {
            (ys[n - 1] - ys[n - 2]) / (xs[n - 1] - xs[n - 2])
        } else {
            (ys[i + 1] - ys[i - 1]) / (xs[i + 1] - xs[i - 1])
        }
    }).collect()
}

/// Cumulative trapezoidal integral; Y[0] = 0.
pub fn integral(xs: &[f64], ys: &[f64]) -> Vec<f64> {
    let n = ys.len().min(xs.len());
    let mut out = Vec::with_capacity(n);
    let mut acc = 0.0;
    for i in 0..n {
        if i > 0 {
            acc += (xs[i] - xs[i - 1]) * (ys[i] + ys[i - 1]) / 2.0;
        }
        out.push(acc);
    }
    out
}

/// Normalize to [0,1] (min-max) or zero-mean/unit-std (z-score).
pub fn normalize(ys: &[f64], zscore: bool) -> Vec<f64> {
    let finite: Vec<f64> = ys.iter().copied().filter(|v| v.is_finite()).collect();
    if finite.is_empty() { return ys.to_vec(); }
    if zscore {
        let mean = finite.iter().sum::<f64>() / finite.len() as f64;
        let var = finite.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / finite.len() as f64;
        let std = var.sqrt();
        if std == 0.0 { return ys.iter().map(|_| 0.0).collect(); }
        ys.iter().map(|&v| (v - mean) / std).collect()
    } else {
        let min = finite.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = finite.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let range = max - min;
        if range == 0.0 { return ys.iter().map(|_| 0.5).collect(); }
        ys.iter().map(|&v| (v - min) / range).collect()
    }
}

pub fn map_abs(ys: &[f64]) -> Vec<f64> { ys.iter().map(|v| v.abs()).collect() }
pub fn map_ln(ys: &[f64]) -> Vec<f64> { ys.iter().map(|v| v.ln()).collect() }
pub fn map_sqrt(ys: &[f64]) -> Vec<f64> { ys.iter().map(|v| v.sqrt()).collect() }

#[cfg(test)]
mod transform_tests {
    use super::*;

    #[test]
    fn moving_average_centered_window3() {
        let ys = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        // centered window 3 (half=1), clamped at edges:
        // i0:{1,2}=1.5  i1:{1,2,3}=2  i2:{2,3,4}=3  i3:{3,4,5}=4  i4:{4,5}=4.5
        assert_eq!(moving_average(&ys, 3), vec![1.5, 2.0, 3.0, 4.0, 4.5]);
    }
    #[test]
    fn moving_average_window1_is_identity() {
        let ys = vec![1.0, 9.0, 4.0];
        assert_eq!(moving_average(&ys, 1), ys);
    }
    #[test]
    fn derivative_of_linear_is_constant_slope() {
        let xs = vec![0.0, 1.0, 2.0, 3.0];
        let ys = vec![0.0, 2.0, 4.0, 6.0]; // slope 2
        for v in derivative(&xs, &ys) { assert!((v - 2.0).abs() < 1e-9, "got {v}"); }
    }
    #[test]
    fn integral_of_constant_is_cumulative() {
        let xs = vec![0.0, 1.0, 2.0, 3.0];
        let ys = vec![1.0, 1.0, 1.0, 1.0];
        assert_eq!(integral(&xs, &ys), vec![0.0, 1.0, 2.0, 3.0]);
    }
    #[test]
    fn normalize_minmax_maps_to_unit() {
        assert_eq!(normalize(&[10.0, 20.0, 30.0], false), vec![0.0, 0.5, 1.0]);
    }
    #[test]
    fn normalize_zscore_has_zero_mean() {
        let z = normalize(&[1.0, 2.0, 3.0], true);
        let mean: f64 = z.iter().sum::<f64>() / z.len() as f64;
        assert!(mean.abs() < 1e-9);
    }
    #[test]
    fn unary_ops() {
        assert_eq!(map_abs(&[-1.0, 2.0]), vec![1.0, 2.0]);
        assert!(map_ln(&[-1.0])[0].is_nan());
        assert_eq!(map_sqrt(&[4.0, 9.0]), vec![2.0, 3.0]);
    }
}
