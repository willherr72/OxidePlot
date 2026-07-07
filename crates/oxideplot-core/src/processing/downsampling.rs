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

/// Which decimation to use when building the visible render series.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DownsampleMode {
    MinMax,
    Lttb,
    None,
}

impl DownsampleMode {
    pub fn parse(s: &str) -> DownsampleMode {
        match s {
            "lttb" => DownsampleMode::Lttb,
            "none" => DownsampleMode::None,
            _ => DownsampleMode::MinMax,
        }
    }
}

/// Like `downsample_for_view`, but selects the decimation strategy.
///
/// The visible-X window is computed IDENTICALLY to `downsample_for_view`: the same
/// `partition_point` bounds PLUS the ±1 line-continuity padding
/// (`saturating_sub(1)` on the low end, `+ 1` clamped to `len` on the high end) so
/// a line segment crossing into the view from off-screen still draws. This must be
/// byte-identical to `downsample_for_view` because Task 3 swaps the renderer's call
/// site to this function.
pub fn downsample_for_view_mode(
    x: &[f64],
    y: &[f64],
    view_min: f64,
    view_max: f64,
    max_points: usize,
    mode: DownsampleMode,
) -> (Vec<f64>, Vec<f64>) {
    if x.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Same visible window as downsample_for_view, including the ±1 continuity padding.
    let start = x.partition_point(|&v| v < view_min).saturating_sub(1);
    let end = (x.partition_point(|&v| v <= view_max) + 1).min(x.len());
    let xw = &x[start..end];
    let yw = &y[start..end];

    if xw.len() <= max_points || max_points < 3 {
        return (xw.to_vec(), yw.to_vec());
    }
    match mode {
        DownsampleMode::None => (xw.to_vec(), yw.to_vec()),
        DownsampleMode::Lttb => lttb_downsample(xw, yw, max_points),
        DownsampleMode::MinMax => minmax_envelope(xw, yw, max_points / 2),
    }
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

/// Min/max envelope decimation: split into `buckets` equal index ranges and keep
/// each bucket's min-y AND max-y point (in x order). Unlike LTTB this NEVER drops
/// a 1-sample spike or dropout — the extreme in each bucket is always kept.
/// Returns up to 2×buckets points.
pub fn minmax_envelope(fx: &[f64], fy: &[f64], buckets: usize) -> (Vec<f64>, Vec<f64>) {
    let n = fx.len();
    if buckets == 0 || n <= buckets * 2 {
        return (fx.to_vec(), fy.to_vec());
    }
    let mut ox = Vec::with_capacity(buckets * 2);
    let mut oy = Vec::with_capacity(buckets * 2);
    for b in 0..buckets {
        let lo = b * n / buckets;
        let hi = ((b + 1) * n / buckets).min(n);
        if lo >= hi {
            continue;
        }
        let mut imin = lo;
        let mut imax = lo;
        for i in lo..hi {
            if fy[i] < fy[imin] {
                imin = i;
            }
            if fy[i] > fy[imax] {
                imax = i;
            }
        }
        let (a, c) = if imin <= imax { (imin, imax) } else { (imax, imin) };
        ox.push(fx[a]);
        oy.push(fy[a]);
        if c != a {
            ox.push(fx[c]);
            oy.push(fy[c]);
        }
    }
    (ox, oy)
}

#[cfg(test)]
mod envelope_tests {
    use super::*;

    #[test]
    fn minmax_keeps_a_lone_spike() {
        // 1000 samples of 0.0 with a single 1e6 spike at index 640.
        let n = 1000;
        let fx: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let mut fy = vec![0.0f64; n];
        fy[640] = 1e6;
        let (_, oy) = minmax_envelope(&fx, &fy, 100); // 100 buckets → ≤200 points
        assert!(oy.iter().cloned().fold(0.0, f64::max) >= 1e6,
            "min/max envelope must preserve the spike");
        assert!(oy.len() < n, "should have downsampled");
    }

    #[test]
    fn minmax_passthrough_when_small() {
        let fx = [0.0, 1.0, 2.0];
        let fy = [0.0, 1.0, 2.0];
        let (ox, _) = minmax_envelope(&fx, &fy, 100);
        assert_eq!(ox.len(), 3);
    }
}

#[cfg(test)]
mod ds_mode_tests {
    use super::*;

    fn ramp(n: usize) -> (Vec<f64>, Vec<f64>) {
        let x: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let y = x.clone();
        (x, y)
    }

    #[test]
    fn none_returns_windowed_slice_untouched() {
        let (x, y) = ramp(1000);
        // window to x in [100, 200] → 101 in-range points, PLUS downsample_for_view's
        // ±1 line-continuity padding on each edge → x in [99, 201], 103 points, no decimation.
        let (ox, _) = downsample_for_view_mode(&x, &y, 100.0, 200.0, 50, DownsampleMode::None);
        assert_eq!(ox.first().copied(), Some(99.0));
        assert_eq!(ox.last().copied(), Some(201.0));
        assert_eq!(ox.len(), 103);
    }

    #[test]
    fn minmax_keeps_spike_within_budget() {
        let n = 5000;
        let x: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let mut y = vec![0.0f64; n];
        y[2500] = 1e6;
        let (_, oy) = downsample_for_view_mode(&x, &y, 0.0, n as f64, 400, DownsampleMode::MinMax);
        assert!(oy.iter().cloned().fold(0.0, f64::max) >= 1e6, "spike must survive minmax");
        assert!(oy.len() <= 400, "output within the point budget");
    }

    #[test]
    fn lttb_decimates_to_budget() {
        let (x, y) = ramp(5000);
        let (ox, _) = downsample_for_view_mode(&x, &y, 0.0, 5000.0, 400, DownsampleMode::Lttb);
        assert!(ox.len() <= 400 && ox.len() > 3);
    }

    #[test]
    fn parse_modes() {
        assert!(matches!(DownsampleMode::parse("lttb"), DownsampleMode::Lttb));
        assert!(matches!(DownsampleMode::parse("none"), DownsampleMode::None));
        assert!(matches!(DownsampleMode::parse("minmax"), DownsampleMode::MinMax));
        assert!(matches!(DownsampleMode::parse("garbage"), DownsampleMode::MinMax));
    }
}
