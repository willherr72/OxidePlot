use kiddo::KdTree;
use kiddo::SquaredEuclidean;

/// Wrapper around a 2D KD-tree for fast nearest-point hover lookup.
pub struct HoverTree {
    tree: KdTree<f64, 2>,
}

impl HoverTree {
    /// Build a KD-tree from x,y data points.
    /// Only includes finite (non-NaN, non-Inf) points.
    /// The item stored for each point is its original index (as u64).
    pub fn build(x: &[f64], y: &[f64]) -> Self {
        let mut tree: KdTree<f64, 2> = KdTree::new();

        for (i, (&xv, &yv)) in x.iter().zip(y.iter()).enumerate() {
            if xv.is_finite() && yv.is_finite() {
                tree.add(&[xv, yv], i as u64);
            }
        }

        // If no valid points were added, insert a dummy so queries don't panic
        if tree.size() == 0 {
            tree.add(&[0.0, 0.0], 0);
        }

        Self { tree }
    }

    /// Find the nearest point to (qx, qy).
    /// Returns (original_index, distance) where distance is Euclidean distance.
    pub fn nearest(&self, qx: f64, qy: f64) -> (usize, f64) {
        let result = self.tree.nearest_one::<SquaredEuclidean>(&[qx, qy]);
        let idx = result.item as usize;
        let dist = result.distance.sqrt();
        (idx, dist)
    }
}
