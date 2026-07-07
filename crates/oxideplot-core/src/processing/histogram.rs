/// Distribution of the finite values into `nbins` equal-width bins.
pub struct Histogram {
    pub counts: Vec<usize>,
    pub bin_centers: Vec<f64>,
    pub min: f64,
    pub max: f64,
    pub n: usize,
}

pub fn histogram(vals: &[f64], nbins: usize) -> Option<Histogram> {
    let finite: Vec<f64> = vals.iter().copied().filter(|v| v.is_finite()).collect();
    if finite.len() < 2 {
        return None;
    }
    let nbins = nbins.clamp(2, 200);
    let min = finite.iter().copied().fold(f64::INFINITY, f64::min);
    let max = finite.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (max - min).max(1e-12);
    let mut counts = vec![0usize; nbins];
    for &v in &finite {
        let bi = (((v - min) / span * nbins as f64) as usize).min(nbins - 1);
        counts[bi] += 1;
    }
    let bin_centers = (0..nbins)
        .map(|i| min + span * (i as f64 + 0.5) / nbins as f64)
        .collect();
    Some(Histogram { counts, bin_centers, min, max, n: finite.len() })
}

#[cfg(test)]
mod histogram_tests {
    use super::*;

    #[test]
    fn bimodal_two_peaks_empty_middle() {
        // 400 samples near 0, 600 near 25, nothing between.
        let mut v = vec![0.0f64; 400];
        v.extend(vec![25.0f64; 600]);
        let h = histogram(&v, 30).unwrap();
        assert_eq!(h.n, 1000);
        // the middle third of bins should be empty
        let mid = &h.counts[10..20];
        assert!(mid.iter().all(|&c| c == 0), "expected empty middle, got {mid:?}");
        assert!(h.counts[0] >= 400);
        assert!(h.counts[29] >= 600);
    }

    #[test]
    fn too_few_values_is_none() {
        assert!(histogram(&[1.0], 10).is_none());
    }
}
