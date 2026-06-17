/// Compute nice grid line positions for an axis range.
/// Returns (value, is_major) pairs.
pub fn compute_grid_lines(min: f64, max: f64) -> Vec<(f64, bool)> {
    let range = max - min;
    if range <= 0.0 || !range.is_finite() {
        return Vec::new();
    }

    let raw_step = range / 8.0;
    let order = 10f64.powf(raw_step.log10().floor());
    let normalized = raw_step / order;

    let nice_step = if normalized <= 1.0 {
        order
    } else if normalized <= 2.0 {
        2.0 * order
    } else if normalized <= 5.0 {
        5.0 * order
    } else {
        10.0 * order
    };

    let minor_step = nice_step / 5.0;

    let start = (min / minor_step).floor() as i64;
    let end = (max / minor_step).ceil() as i64;

    let mut lines = Vec::new();
    for i in start..=end {
        let val = i as f64 * minor_step;
        if val >= min && val <= max {
            let is_major = ((val / nice_step).round() * nice_step - val).abs() < nice_step * 0.01;
            lines.push((val, is_major));
        }
    }
    lines
}

/// Format a numeric value for axis tick labels.
pub fn format_tick_value(val: f64) -> String {
    if val.abs() >= 1e6 || (val != 0.0 && val.abs() < 1e-3) {
        format!("{val:.2e}")
    } else if val == 0.0 {
        "0".to_string()
    } else {
        let s = format!("{val:.6}");
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid_lines_within_range() {
        let lines = compute_grid_lines(0.0, 100.0);
        assert!(!lines.is_empty());
        for (val, _major) in lines { assert!((0.0..=100.0).contains(&val)); }
    }
}
