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
/// Uses binary search to find the visible range when data is sorted by X,
/// avoiding a full linear scan of the entire dataset.
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

    // Check if X data is sorted (common case for time series).
    // If sorted, use binary search for O(log n) range finding.
    let is_sorted = x.windows(2).all(|w| w[0] <= w[1]);

    let (vis_x, vis_y) = if is_sorted {
        // Binary search for the start and end of the visible range.
        // Include one extra point on each side for line continuity.
        let start = x.partition_point(|&v| v < view_min).saturating_sub(1);
        let end = (x.partition_point(|&v| v <= view_max) + 1).min(x.len());
        let slice_x = &x[start..end];
        let slice_y = &y[start..end];

        if slice_x.len() <= max_points {
            return (slice_x.to_vec(), slice_y.to_vec());
        }
        (slice_x.to_vec(), slice_y.to_vec())
    } else {
        // Unsorted data: linear filter
        let mut vx = Vec::new();
        let mut vy = Vec::new();
        for (&xv, &yv) in x.iter().zip(y.iter()) {
            if xv >= view_min && xv <= view_max {
                vx.push(xv);
                vy.push(yv);
            }
        }
        if vx.len() <= max_points {
            return (vx, vy);
        }
        (vx, vy)
    };

    lttb_downsample(&vis_x, &vis_y, max_points)
}
