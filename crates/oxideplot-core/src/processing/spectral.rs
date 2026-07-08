use crate::data::loader::{column_to_timestamps, LoadedData};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use std::f64::consts::PI;

/// Infer the sample rate (Hz) from the dataset's first datetime column (1/median
/// dt between rows). Falls back to 1.0 (freq then reads in cycles/sample).
pub fn infer_sample_rate(data: &LoadedData) -> f64 {
    for c in 0..data.columns.len() {
        if let Some((ts, frac)) = column_to_timestamps(&data.column_data[c]) {
            if frac >= 0.5 && ts.len() >= 2 {
                let mut dts: Vec<f64> = ts
                    .windows(2)
                    .map(|w| w[1] - w[0])
                    .filter(|d| d.is_finite() && *d > 0.0)
                    .collect();
                if !dts.is_empty() {
                    dts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    let md = dts[dts.len() / 2];
                    if md > 0.0 {
                        return 1.0 / md;
                    }
                }
            }
        }
    }
    1.0
}

/// Hann-windowed, mean-removed FFT of `vals`, returning one-sided (frequency,
/// power) arrays (DC bin dropped). `fs` is the sample rate in Hz.
pub fn compute_psd(vals: &[f64], fs: f64) -> (Vec<f64>, Vec<f64>) {
    let y: Vec<f64> = vals.iter().copied().filter(|v| v.is_finite()).collect();
    let n = y.len();
    if n < 4 {
        return (vec![], vec![]);
    }
    let mean = y.iter().sum::<f64>() / n as f64;
    let mut buf: Vec<Complex<f64>> = (0..n)
        .map(|i| {
            let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / (n as f64 - 1.0)).cos();
            Complex::new((y[i] - mean) * w, 0.0)
        })
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    let half = n / 2;
    let mut freqs = Vec::with_capacity(half);
    let mut power = Vec::with_capacity(half);
    for k in 1..half {
        freqs.push(k as f64 * fs / n as f64);
        power.push(buf[k].norm_sqr() / n as f64);
    }
    (freqs, power)
}

/// Short-time FFT magnitude matrix (`frames[frame][bin]`, bins = window/2, hop =
/// window/2). Returns the matrix and the bin count.
pub fn compute_spectrogram(vals: &[f64], window: usize) -> (Vec<Vec<f64>>, usize) {
    let y: Vec<f64> = vals.iter().copied().filter(|v| v.is_finite()).collect();
    let w = window.clamp(16, 4096);
    let n = y.len();
    if n < w {
        return (vec![], 0);
    }
    let hop = (w / 2).max(1);
    let bins = w / 2;
    let fft = FftPlanner::new().plan_fft_forward(w);
    let hann: Vec<f64> = (0..w)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / (w as f64 - 1.0)).cos())
        .collect();
    let mut frames: Vec<Vec<f64>> = Vec::new();
    let mut start = 0;
    while start + w <= n {
        let mut buf: Vec<Complex<f64>> = (0..w)
            .map(|i| Complex::new(y[start + i] * hann[i], 0.0))
            .collect();
        fft.process(&mut buf);
        frames.push((0..bins).map(|k| buf[k].norm_sqr().sqrt()).collect());
        start += hop;
    }
    // Include the trailing partial window (zero-padded past the last sample) so
    // the spectrogram covers the full signal rather than dropping up to one
    // window's worth of samples at the end.
    if start < n {
        let mut buf: Vec<Complex<f64>> = (0..w)
            .map(|i| {
                let s = start + i;
                let v = if s < n { y[s] } else { 0.0 };
                Complex::new(v * hann[i], 0.0)
            })
            .collect();
        fft.process(&mut buf);
        frames.push((0..bins).map(|k| buf[k].norm_sqr().sqrt()).collect());
    }
    (frames, bins)
}

#[cfg(test)]
mod spectral_tests {
    use super::*;

    fn argmax(v: &[f64]) -> usize {
        v.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).map(|(i, _)| i).unwrap()
    }

    #[test]
    fn psd_peaks_at_signal_frequency() {
        let fs = 100.0;
        let n = 4000;
        // 5 Hz sine.
        let sig: Vec<f64> = (0..n).map(|i| (2.0 * PI * 5.0 * i as f64 / fs).sin()).collect();
        let (freqs, power) = compute_psd(&sig, fs);
        let peak = freqs[argmax(&power)];
        assert!((peak - 5.0).abs() < 0.5, "peak at {peak}, expected ~5 Hz");
    }

    #[test]
    fn spectrogram_includes_trailing_window() {
        // w=256, hop=128. n=434 → 2 full-window frames (start 0, 128), leaving a
        // tail at start=256 (<434), so a zero-padded frame is appended → 3 total.
        let sig: Vec<f64> = (0..434).map(|i| (i as f64 / 10.0).sin()).collect();
        let (frames, bins) = compute_spectrogram(&sig, 256);
        assert_eq!(bins, 128);
        assert_eq!(frames.len(), 3, "trailing partial window must be included");
        assert!(frames.iter().all(|f| f.len() == 128));
    }

    #[test]
    fn spectrogram_shape() {
        let sig: Vec<f64> = (0..2000).map(|i| (i as f64 / 10.0).sin()).collect();
        let (frames, bins) = compute_spectrogram(&sig, 256);
        assert_eq!(bins, 128);
        assert!(frames.len() > 5);
        assert!(frames.iter().all(|f| f.len() == 128));
    }

    #[test]
    fn sample_rate_from_datetime() {
        // ISO-8601 whole-second stamps at 2-second steps → 0.5 Hz. (Non-1.0 so it
        // is distinguishable from the no-datetime fallback of 1.0.) Uses the same
        // ISO format the MCP verified via column_to_timestamps.
        let stamps: Vec<String> = (0..20)
            .map(|i| format!("2026-07-02T16:00:{:02}Z", i * 2))
            .collect();
        let d = LoadedData {
            columns: vec!["t".into(), "v".into()],
            column_data: vec![stamps, vec!["0".into(); 20]],
            row_count: 20,
        };
        assert!((infer_sample_rate(&d) - 0.5).abs() < 0.05);
    }
}
