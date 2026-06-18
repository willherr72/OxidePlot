//! Resample a series onto a uniform x-grid by linear / nearest / natural-cubic
//! interpolation. Pure + native-tested; the wasm layer wraps `resample`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method { Linear, Nearest, Cubic }

/// Index `i` with `sx[i] <= x <= sx[i+1]`, clamped to `[0, len-2]`.
fn bracket(sx: &[f64], x: f64) -> usize {
    match sx.binary_search_by(|v| v.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Less)) {
        Ok(i) => i.min(sx.len() - 2),
        Err(i) => i.saturating_sub(1).min(sx.len() - 2),
    }
}

fn interp_linear(sx: &[f64], sy: &[f64], x: f64) -> f64 {
    let i = bracket(sx, x);
    let (x0, x1, y0, y1) = (sx[i], sx[i + 1], sy[i], sy[i + 1]);
    if x1 == x0 { y0 } else { y0 + (y1 - y0) * (x - x0) / (x1 - x0) }
}

fn interp_nearest(sx: &[f64], sy: &[f64], x: f64) -> f64 {
    let i = bracket(sx, x);
    if (x - sx[i]).abs() <= (sx[i + 1] - x).abs() { sy[i] } else { sy[i + 1] }
}

/// Natural cubic spline second derivatives (m[0]=m[n-1]=0) via the Thomas algorithm.
fn cubic_second_derivs(sx: &[f64], sy: &[f64]) -> Vec<f64> {
    let n = sx.len();
    let mut m = vec![0.0; n];
    if n < 3 { return m; }
    let mut c = vec![0.0; n]; // modified superdiagonal
    let mut d = vec![0.0; n]; // modified rhs
    for i in 1..n - 1 {
        let h0 = sx[i] - sx[i - 1];
        let h1 = sx[i + 1] - sx[i];
        let a = h0;
        let b = 2.0 * (h0 + h1);
        let cc = h1;
        let dd = 6.0 * ((sy[i + 1] - sy[i]) / h1 - (sy[i] - sy[i - 1]) / h0);
        let denom = b - a * c[i - 1];
        c[i] = cc / denom;
        d[i] = (dd - a * d[i - 1]) / denom;
    }
    for i in (1..n - 1).rev() {
        m[i] = d[i] - c[i] * m[i + 1];
    }
    m
}

fn interp_cubic(sx: &[f64], sy: &[f64], m: &[f64], x: f64) -> f64 {
    let i = bracket(sx, x);
    let h = sx[i + 1] - sx[i];
    if h == 0.0 { return sy[i]; }
    let a = (sx[i + 1] - x) / h;
    let b = (x - sx[i]) / h;
    a * sy[i] + b * sy[i + 1] + ((a * a * a - a) * m[i] + (b * b * b - b) * m[i + 1]) * (h * h) / 6.0
}

/// Resample `(xs,ys)` onto `n` evenly-spaced x over `[min,max]` of the finite,
/// ascending source points. Returns `(grid_xs, interp_ys)`.
pub fn resample(xs: &[f64], ys: &[f64], n: usize, method: Method) -> (Vec<f64>, Vec<f64>) {
    let (sx, sy): (Vec<f64>, Vec<f64>) = xs.iter().zip(ys.iter())
        .filter(|(x, y)| x.is_finite() && y.is_finite())
        .map(|(&x, &y)| (x, y))
        .unzip();
    if sx.len() < 2 || n < 2 {
        return (xs.to_vec(), ys.to_vec());
    }
    let x0 = sx[0];
    let x1 = sx[sx.len() - 1];
    let m = if method == Method::Cubic { Some(cubic_second_derivs(&sx, &sy)) } else { None };
    let grid: Vec<f64> = (0..n).map(|i| x0 + (x1 - x0) * (i as f64) / ((n - 1) as f64)).collect();
    let out: Vec<f64> = grid.iter().map(|&x| match method {
        Method::Linear => interp_linear(&sx, &sy, x),
        Method::Nearest => interp_nearest(&sx, &sy, x),
        Method::Cubic => interp_cubic(&sx, &sy, m.as_ref().unwrap(), x),
    }).collect();
    (grid, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linear_resample_of_line() {
        // y = 2x sampled at 0,2,4 → resampled to 5 pts over [0,4] = 0,1,2,3,4
        let (gx, gy) = resample(&[0.0,2.0,4.0], &[0.0,4.0,8.0], 5, Method::Linear);
        assert_eq!(gx, vec![0.0,1.0,2.0,3.0,4.0]);
        for (x,y) in gx.iter().zip(gy.iter()) { assert!((y - 2.0*x).abs() < 1e-9, "x={x} y={y}"); }
    }
    #[test]
    fn nearest_picks_closest_sample() {
        // samples at x=0(y=10), x=10(y=20); grid point near 0 → 10, near 10 → 20
        let (gx, gy) = resample(&[0.0,10.0], &[10.0,20.0], 11, Method::Nearest);
        assert_eq!(gx.len(), 11);
        assert_eq!(gy[0], 10.0);            // x=0
        assert_eq!(*gy.last().unwrap(), 20.0); // x=10
        assert_eq!(gy[1], 10.0);            // x=1 nearer 0
        assert_eq!(gy[9], 20.0);            // x=9 nearer 10
    }
    #[test]
    fn cubic_passes_through_sample_points() {
        let sx = vec![0.0, 1.0, 2.0, 3.0];
        let sy = vec![0.0, 1.0, 8.0, 27.0]; // y=x^3 samples
        // resample to 4 pts == the original x grid → values match the samples
        let (gx, gy) = resample(&sx, &sy, 4, Method::Cubic);
        assert_eq!(gx, sx);
        for (got, want) in gy.iter().zip(sy.iter()) { assert!((got - want).abs() < 1e-9, "got {got} want {want}"); }
    }
    #[test]
    fn resample_count_and_endpoints() {
        let (gx, _) = resample(&[1.0, 5.0, 9.0], &[0.0, 1.0, 0.0], 100, Method::Linear);
        assert_eq!(gx.len(), 100);
        assert_eq!(gx[0], 1.0);
        assert_eq!(*gx.last().unwrap(), 9.0);
    }
    #[test]
    fn degenerate_returns_input() {
        let (gx, gy) = resample(&[1.0], &[2.0], 50, Method::Linear);
        assert_eq!((gx, gy), (vec![1.0], vec![2.0]));
    }
}
