/// Largest-Triangle-Three-Buckets (LTTB) downsampling.
/// Takes x,y arrays and target number of output points.
/// Returns (downsampled_x, downsampled_y).
pub fn lttb_downsample(x: &[f64], y: &[f64], target: usize) -> (Vec<f64>, Vec<f64>) {
    let n = x.len();
    if n <= target || target < 3 {
        return (x.to_vec(), y.to_vec());
    }

    let mut out_x = Vec::with_capacity(target);
    let mut out_y = Vec::with_capacity(target);

    // Always keep the first point
    out_x.push(x[0]);
    out_y.push(y[0]);

    let bucket_size = (n - 2) as f64 / (target - 2) as f64;

    let mut prev_idx: usize = 0;

    for i in 0..(target - 2) {
        // Calculate bucket boundaries
        let bucket_start = ((i as f64 + 1.0) * bucket_size) as usize + 1;
        let bucket_end = (((i as f64 + 2.0) * bucket_size) as usize + 1).min(n - 1);

        // Calculate average of next bucket for the triangle
        let next_bucket_start = (((i as f64 + 2.0) * bucket_size) as usize + 1).min(n - 1);
        let next_bucket_end = (((i as f64 + 3.0) * bucket_size) as usize + 1).min(n);

        let mut avg_x = 0.0;
        let mut avg_y = 0.0;
        let next_count = (next_bucket_end - next_bucket_start).max(1);
        for j in next_bucket_start..next_bucket_end.min(n) {
            avg_x += x[j];
            avg_y += y[j];
        }
        avg_x /= next_count as f64;
        avg_y /= next_count as f64;

        // Find the point in current bucket with largest triangle area
        let mut max_area = -1.0f64;
        let mut best_idx = bucket_start;

        let prev_x = x[prev_idx];
        let prev_y = y[prev_idx];

        for j in bucket_start..bucket_end.min(n) {
            // Triangle area (doubled, no need for /2 since we're comparing)
            let area = ((prev_x - avg_x) * (y[j] - prev_y)
                - (prev_x - x[j]) * (avg_y - prev_y))
                .abs();
            if area > max_area {
                max_area = area;
                best_idx = j;
            }
        }

        out_x.push(x[best_idx]);
        out_y.push(y[best_idx]);
        prev_idx = best_idx;
    }

    // Always keep the last point
    out_x.push(x[n - 1]);
    out_y.push(y[n - 1]);

    (out_x, out_y)
}

/// Downsample data for the visible range, applying LTTB when point count exceeds threshold.
/// Returns (display_x, display_y) ready for plotting.
///
/// Assumes `x` is in ascending order (standard time-series contract).
pub fn downsample_for_view(
    x: &[f64],
    y: &[f64],
    view_min: f64,
    view_max: f64,
    max_points: usize,
) -> (Vec<f64>, Vec<f64>) {
    if x.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Binary search for the start and end of the visible range.
    // Include one extra point on each side for line continuity.
    let start = x.partition_point(|&v| v < view_min).saturating_sub(1);
    let end = (x.partition_point(|&v| v <= view_max) + 1).min(x.len());
    let slice_x = &x[start..end];
    let slice_y = &y[start..end];

    if slice_x.len() <= max_points {
        return (slice_x.to_vec(), slice_y.to_vec());
    }

    lttb_downsample(slice_x, slice_y, max_points)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Downsampling a large series to N points returns exactly N points
    /// and preserves the first and last data points.
    #[test]
    fn downsample_keeps_endpoints_and_count() {
        let xs: Vec<f64> = (0..10_000).map(|i| i as f64).collect();
        let ys: Vec<f64> = xs.iter().map(|x| (x * 0.01).sin()).collect();
        let (out_x, out_y) = lttb_downsample(&xs, &ys, 500);
        assert_eq!(out_x.len(), 500);
        assert_eq!(out_y.len(), 500);
        assert_eq!(out_x[0], xs[0]);
        assert_eq!(*out_x.last().unwrap(), *xs.last().unwrap());
        assert_eq!(out_y[0], ys[0]);
        assert_eq!(*out_y.last().unwrap(), *ys.last().unwrap());
    }

    /// When target >= input length, lttb_downsample returns the input unchanged (no-op).
    #[test]
    fn downsample_noop_when_target_exceeds_len() {
        let xs = vec![0.0_f64, 1.0, 2.0];
        let ys = vec![0.0_f64, 1.0, 0.0];
        let (out_x, out_y) = lttb_downsample(&xs, &ys, 100);
        assert_eq!(out_x, xs);
        assert_eq!(out_y, ys);
    }

    /// When target < 3, lttb_downsample returns the input unchanged (degenerate no-op).
    #[test]
    fn downsample_noop_when_target_lt_3() {
        let xs: Vec<f64> = (0..1000).map(|i| i as f64).collect();
        let ys: Vec<f64> = xs.iter().map(|x| x * 2.0).collect();
        // target = 2 triggers the `target < 3` early-return path
        let (out_x, out_y) = lttb_downsample(&xs, &ys, 2);
        assert_eq!(out_x.len(), xs.len());
        assert_eq!(out_y.len(), ys.len());
    }
}
