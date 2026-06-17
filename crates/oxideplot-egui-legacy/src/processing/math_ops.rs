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
