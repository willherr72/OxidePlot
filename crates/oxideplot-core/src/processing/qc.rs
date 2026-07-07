use crate::data::loader::{column_to_f64, column_to_timestamps, resolve_col, LoadedData};
use crate::processing::statistics::median_mad;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    High,
    Medium,
    Low,
}
impl Severity {
    pub fn rank(self) -> u8 {
        match self {
            Severity::High => 0,
            Severity::Medium => 1,
            Severity::Low => 2,
        }
    }
}

/// A QC finding. Optional fields are omitted from JSON when None, so each
/// finding kind serializes to exactly the fields it uses (matching the MCP).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<[usize; 2]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_gap_row: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onset_row: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub culprit: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected: Option<Vec<String>>,
}

impl Finding {
    /// Minimal constructor; callers set optional fields with struct-update.
    fn new(severity: Severity, kind: &str, detail: String) -> Self {
        Finding {
            severity,
            kind: kind.to_string(),
            column: None,
            detail,
            row: None,
            rows: None,
            first_gap_row: None,
            onset_row: None,
            culprit: None,
            affected: None,
        }
    }
}

/// Group a sorted list of row indices into runs (start, end, count), joining
/// indices no more than `max_gap` apart.
pub(crate) fn group_runs(rows: &[usize], max_gap: usize) -> Vec<(usize, usize, usize)> {
    let mut runs = Vec::new();
    if rows.is_empty() {
        return runs;
    }
    let (mut start, mut prev, mut count) = (rows[0], rows[0], 1usize);
    for &r in &rows[1..] {
        if r <= prev + max_gap + 1 {
            prev = r;
            count += 1;
        } else {
            runs.push((start, prev, count));
            start = r;
            prev = r;
            count = 1;
        }
    }
    runs.push((start, prev, count));
    runs
}

/// Narrow a changepoint (level `m0` → `m1`) to the transition row within
/// `[lo, hi)` — the first row where a trailing-window mean crosses the midpoint.
pub(crate) fn localize_changepoint(vals: &[f64], lo: usize, hi: usize, m0: f64, m1: f64) -> usize {
    let mid = (m0 + m1) * 0.5;
    let ascending = m1 >= m0;
    let win = ((hi - lo) / 6).clamp(5, 300);
    for r in lo..hi {
        let a = r.saturating_sub(win).max(lo);
        let s: Vec<f64> = vals[a..=r].iter().copied().filter(|v| v.is_finite()).collect();
        if s.is_empty() {
            continue;
        }
        let m = s.iter().sum::<f64>() / s.len() as f64;
        if (ascending && m >= mid) || (!ascending && m <= mid) {
            return r;
        }
    }
    (lo + hi) / 2
}

/// Rough channel role from its name: 0 = raw source (`raw…`), 2 = derived output
/// (calibrated/calculated/…), 1 = neutral. Used to trace a fault to its source.
pub(crate) fn channel_role(name: &str) -> u8 {
    let n = name.to_ascii_lowercase();
    if n.starts_with("raw") {
        return 0;
    }
    const DERIVED: &[&str] = &[
        "calibrated",
        "calculated",
        "computed",
        "corrected",
        "derived",
        "adjusted",
    ];
    if DERIVED.iter().any(|d| n.contains(d)) {
        return 2;
    }
    1
}

/// Robust level shift at `onset`: (median_after − median_before, |shift| / MAD-noise)
/// over a window `w` either side. Used to spot a co-occurring shift in a raw
/// channel that didn't independently cross the changepoint threshold.
pub(crate) fn shift_ratio_at(vals: &[f64], onset: usize, w: usize) -> Option<(f64, f64)> {
    let n = vals.len();
    if onset == 0 || onset >= n {
        return None;
    }
    let lo = onset.saturating_sub(w);
    let hi = (onset + w).min(n);
    let before: Vec<f64> = vals[lo..onset].iter().copied().filter(|v| v.is_finite()).collect();
    let after: Vec<f64> = vals[onset..hi].iter().copied().filter(|v| v.is_finite()).collect();
    let (mb, db) = median_mad(&before)?;
    let (ma, da) = median_mad(&after)?;
    let noise = (db.max(da) * 1.4826).max(1e-9);
    let shift = ma - mb;
    Some((shift, shift.abs() / noise))
}

/// Longest run of consecutive identical raw cells (flags a frozen/stuck channel).
pub(crate) fn longest_constant_run(cells: &[String]) -> usize {
    let mut best = 0usize;
    let mut cur = 0usize;
    let mut prev: Option<&str> = None;
    for c in cells {
        if prev == Some(c.as_str()) {
            cur += 1;
        } else {
            cur = 1;
            prev = Some(c.as_str());
        }
        best = best.max(cur);
    }
    best
}

/// Scan a dataset for data-quality problems and return findings sorted by
/// severity (high → medium → low). Ported from the MCP `health_check` tool.
pub fn health_check(
    data: &LoadedData,
    numeric_cols: &[bool],
    lineage: Option<&HashMap<String, Vec<String>>>,
) -> Vec<Finding> {
    let n_rows = data.row_count;
    let mut findings: Vec<Finding> = Vec::new();
    // Detected changepoints (col idx, onset row, shift) — grouped + traced later.
    let mut changepoints: Vec<(usize, usize, f64)> = Vec::new();

    // Dataset-level: time-index gaps (first datetime column).
    for c in 0..data.columns.len() {
        if let Some((ts, frac)) = column_to_timestamps(&data.column_data[c]) {
            if frac >= 0.5 && ts.len() >= 3 {
                let dts: Vec<f64> = ts.windows(2).map(|w| w[1] - w[0]).collect();
                let mut sorted: Vec<f64> =
                    dts.iter().copied().filter(|d| d.is_finite() && *d > 0.0).collect();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                if !sorted.is_empty() {
                    let median = sorted[sorted.len() / 2];
                    let (mut n_gaps, mut lost, mut first) = (0usize, 0.0, None);
                    for (i, &d) in dts.iter().enumerate() {
                        if d.is_finite() && d > median * 3.0 && d > median + 1e-6 {
                            n_gaps += 1;
                            lost += d - median;
                            if first.is_none() {
                                first = Some(i);
                            }
                        }
                    }
                    if n_gaps > 0 {
                        findings.push(Finding {
                            column: Some(data.columns[c].clone()),
                            first_gap_row: first,
                            ..Finding::new(
                                Severity::Medium,
                                "time_gaps",
                                format!("{n_gaps} gap(s), ~{:.0}s of samples missing", lost),
                            )
                        });
                    }
                }
            }
            break;
        }
    }

    // Per numeric column checks.
    for c in 0..data.columns.len() {
        if !numeric_cols[c] {
            continue;
        }
        let name = &data.columns[c];
        let cells = &data.column_data[c];
        // Skip a datetime index column (it's the time axis, not a data channel).
        if column_to_timestamps(cells)
            .map(|(_, f)| f >= 0.5)
            .unwrap_or(false)
        {
            continue;
        }
        let (vals, _) = column_to_f64(cells);
        let finite: Vec<f64> = vals.iter().copied().filter(|v| v.is_finite()).collect();
        let n_finite = finite.len();
        let n_missing = n_rows.saturating_sub(n_finite);
        let distinct = cells.iter().collect::<std::collections::HashSet<_>>().len();
        let const_run = longest_constant_run(cells);

        // Dead / constant.
        if n_finite > 0 && distinct <= 1 {
            findings.push(Finding {
                column: Some(name.clone()),
                ..Finding::new(
                    Severity::High,
                    "dead",
                    "single distinct value (dead/constant)".to_string(),
                )
            });
        } else if n_finite > 0 {
            let n_zero = finite.iter().filter(|&&v| v == 0.0).count();
            if n_zero as f64 / n_finite as f64 > 0.95 {
                findings.push(Finding {
                    column: Some(name.clone()),
                    ..Finding::new(
                        Severity::High,
                        "dead",
                        format!("{:.0}% zero", 100.0 * n_zero as f64 / n_finite as f64),
                    )
                });
            }
        }
        // Frozen (long constant run but not fully dead).
        if distinct > 1 && (const_run as f64) > (n_rows as f64 * 0.05).max(30.0) {
            findings.push(Finding {
                column: Some(name.clone()),
                ..Finding::new(
                    Severity::Medium,
                    "frozen",
                    format!("stuck for {const_run} consecutive rows"),
                )
            });
        }
        // Missing.
        if n_rows > 0 && n_missing as f64 / n_rows as f64 > 0.05 {
            findings.push(Finding {
                column: Some(name.clone()),
                ..Finding::new(
                    Severity::Medium,
                    "missing",
                    format!("{:.1}% missing ({n_missing} rows)", 100.0 * n_missing as f64 / n_rows as f64),
                )
            });
        }
        // Outliers via robust z-score (median / MAD): isolated short runs are
        // glitches; a long contiguous run is a regime (e.g. the stationary
        // survey start), not a glitch — reported separately at lower severity.
        if n_finite >= 20 {
            if let Some((median, mad)) = median_mad(&finite) {
                if mad > 0.0 {
                    let rows: Vec<usize> = vals
                        .iter()
                        .enumerate()
                        .filter(|(_, &v)| {
                            v.is_finite() && (v - median).abs() / (1.4826 * mad) > 10.0
                        })
                        .map(|(r, _)| r)
                        .collect();
                    let mut glitch_ranges: Vec<[usize; 2]> = Vec::new();
                    let mut glitch_count = 0usize;
                    for (rs, re, count) in group_runs(&rows, 2) {
                        if count <= 4 {
                            glitch_count += count;
                            if glitch_ranges.len() < 10 {
                                glitch_ranges.push([rs, re]);
                            }
                        } else {
                            findings.push(Finding {
                                column: Some(name.clone()),
                                rows: Some(vec![[rs, re]]),
                                ..Finding::new(
                                    Severity::Medium,
                                    "outlier_regime",
                                    format!("{count} consecutive out-of-range samples (a regime, not a glitch)"),
                                )
                            });
                        }
                    }
                    if glitch_count > 0 {
                        findings.push(Finding {
                            column: Some(name.clone()),
                            rows: Some(glitch_ranges),
                            ..Finding::new(
                                Severity::High,
                                "glitch",
                                format!("{glitch_count} isolated out-of-range sample(s)"),
                            )
                        });
                    }
                }
            }
        }
        // Changepoint: an adjacent-segment MEDIAN jump large vs the local
        // MAD-noise (robust — a lone spike can't move the median), then
        // localised to the actual transition row (finer than the segment grid).
        if n_rows >= 60 {
            let n_seg = (n_rows / 200).clamp(8, 40);
            let mut meds: Vec<Option<f64>> = Vec::new();
            let mut mads: Vec<f64> = Vec::new();
            let mut bounds: Vec<(usize, usize)> = Vec::new();
            for seg in 0..n_seg {
                let lo = seg * n_rows / n_seg;
                let hi = ((seg + 1) * n_rows / n_seg).min(n_rows);
                let sl: Vec<f64> = vals
                    .get(lo..hi)
                    .unwrap_or(&[])
                    .iter()
                    .copied()
                    .filter(|v| v.is_finite())
                    .collect();
                match median_mad(&sl) {
                    Some((m, d)) => {
                        meds.push(Some(m));
                        mads.push(d);
                    }
                    None => meds.push(None),
                }
                bounds.push((lo, hi));
            }
            mads.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let scale = (if mads.is_empty() {
                0.0
            } else {
                mads[mads.len() / 2] * 1.4826
            })
            .max(1e-9);
            for seg in 1..n_seg {
                if let (Some(a), Some(b)) = (meds[seg - 1], meds[seg]) {
                    if (b - a).abs() > 6.0 * scale {
                        let onset =
                            localize_changepoint(&vals, bounds[seg - 1].0, bounds[seg].1, a, b);
                        changepoints.push((c, onset, b - a));
                        break;
                    }
                }
            }
        }
    }

    // --- Channel-lineage tracing: cluster co-occurring changepoints into one
    // event and attribute it to the likely raw source (naming → explicit
    // lineage hint → co-occurrence scan of raw channels at the onset). ---
    changepoints.sort_by_key(|cp| cp.1);
    let tol = (n_rows / 50).max(20);
    let mut clusters: Vec<Vec<(usize, usize, f64)>> = Vec::new();
    for cp in changepoints {
        match clusters.last_mut() {
            Some(last) if cp.1 <= last.last().unwrap().1 + tol => last.push(cp),
            _ => clusters.push(vec![cp]),
        }
    }
    let raw_cols: Vec<usize> = (0..data.columns.len())
        .filter(|&c| numeric_cols[c] && channel_role(&data.columns[c]) == 0)
        .collect();

    for cl in &clusters {
        let onset = cl.iter().map(|c| c.1).min().unwrap();
        let cols_in: Vec<usize> = cl.iter().map(|c| c.0).collect();
        let mut culprit: Vec<usize> = Vec::new();
        // (a) raw-named columns already in the cluster.
        for &c in &cols_in {
            if channel_role(&data.columns[c]) == 0 && !culprit.contains(&c) {
                culprit.push(c);
            }
        }
        // (b) explicit lineage hint on any derived column in the cluster.
        if let Some(map) = lineage {
            for &c in &cols_in {
                if let Some(sources) = map.get(&data.columns[c]) {
                    for src in sources {
                        if let Some(si) = resolve_col(data, src) {
                            if !culprit.contains(&si) {
                                culprit.push(si);
                            }
                        }
                    }
                }
            }
        }
        // (c) co-occurrence scan: a raw channel that shifted at the onset.
        if culprit.is_empty() {
            let w = tol.max(30);
            let mut best: Option<(usize, f64)> = None;
            for &rc in &raw_cols {
                if cols_in.contains(&rc) {
                    continue;
                }
                let (rvals, _) = column_to_f64(&data.column_data[rc]);
                if let Some((_, ratio)) = shift_ratio_at(&rvals, onset, w) {
                    if ratio > 5.0 && best.map_or(true, |b| ratio > b.1) {
                        best = Some((rc, ratio));
                    }
                }
            }
            if let Some((rc, _)) = best {
                culprit.push(rc);
            }
        }

        let source_traced = culprit.iter().any(|c| !cols_in.contains(c));
        if cols_in.len() == 1 && !source_traced {
            let name = &data.columns[cols_in[0]];
            findings.push(Finding {
                column: Some(name.clone()),
                row: Some(onset),
                ..Finding::new(
                    Severity::Medium,
                    "changepoint",
                    format!("median shifts {:.4} at ~row {onset}", cl[0].2),
                )
            });
        } else {
            let affected: Vec<&String> = cols_in.iter().map(|&c| &data.columns[c]).collect();
            let culprit_names: Vec<&String> =
                culprit.iter().map(|&c| &data.columns[c]).collect();
            let detail = if culprit_names.is_empty() {
                format!(
                    "coincident level shift across {} channels at ~row {onset} (source unclear)",
                    affected.len()
                )
            } else {
                format!(
                    "level shift at ~row {onset}; likely source: {}; affects {} channel(s)",
                    culprit_names
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    affected.len()
                )
            };
            let severity = if culprit_names.is_empty() {
                Severity::Medium
            } else {
                Severity::High
            };
            findings.push(Finding {
                onset_row: Some(onset),
                culprit: Some(culprit_names.iter().map(|s| s.to_string()).collect()),
                affected: Some(affected.iter().map(|s| s.to_string()).collect()),
                ..Finding::new(severity, "regime_change_event", detail)
            });
        }
    }

    findings.sort_by_key(|f| f.severity.rank());
    findings
}

#[cfg(test)]
mod qc_tests {
    use super::*;

    // Build a dataset from parallel numeric columns of equal length.
    fn ds(cols: Vec<(&str, Vec<f64>)>) -> (LoadedData, Vec<bool>) {
        let n = cols[0].1.len();
        let columns: Vec<String> = cols.iter().map(|(nm, _)| nm.to_string()).collect();
        let column_data: Vec<Vec<String>> = cols
            .iter()
            .map(|(_, v)| v.iter().map(|x| x.to_string()).collect())
            .collect();
        let numeric = vec![true; columns.len()];
        (LoadedData { columns, column_data, row_count: n }, numeric)
    }

    #[test]
    fn flags_dead_glitch_and_regime() {
        let n = 600;
        let good: Vec<f64> = (0..n).map(|i| (i as f64 / 30.0).sin()).collect();
        let dead = vec![0.0f64; n];
        let mut glitchy: Vec<f64> = (0..n).map(|i| (i as f64 / 12.0).sin()).collect();
        glitchy[250] = 1e6; // single isolated spike
        // 100 static-rail rows at the start over a noisy baseline (baseline must
        // vary so MAD>0 and the rail-out rows read as >10-sigma outliers — a
        // perfectly-constant baseline yields MAD=0 and the outlier path is skipped).
        let static_start: Vec<f64> =
            (0..n).map(|i| if i < 100 { 50.0 } else { (i as f64 / 17.0).sin() }).collect();
        let (d, nc) = ds(vec![
            ("good", good),
            ("dead", dead),
            ("glitchy", glitchy),
            ("static_start", static_start),
        ]);
        let f = health_check(&d, &nc, None);
        let has = |kind: &str, col: &str| {
            f.iter().any(|x| x.kind == kind && x.column.as_deref() == Some(col))
        };
        assert!(has("dead", "dead"));
        assert!(has("glitch", "glitchy"));
        // 100 contiguous outliers reclassify as a regime, NOT a glitch:
        assert!(has("outlier_regime", "static_start"));
        assert!(!has("glitch", "static_start"));
        // a lone spike must NOT manufacture a changepoint on 'glitchy':
        assert!(!f.iter().any(|x| x.kind == "changepoint" && x.column.as_deref() == Some("glitchy")));
    }

    #[test]
    fn traces_regime_to_raw_source() {
        let n = 1000;
        let carrier: Vec<f64> = (0..n).map(|i| (i as f64 / 25.0).sin()).collect();
        let raw_ax2: Vec<f64> =
            (0..n).map(|i| carrier[i] + if i > 500 { 10.0 } else { 0.0 }).collect();
        let cal_az: Vec<f64> = (0..n).map(|i| raw_ax2[i] * 0.7).collect();
        let (d, nc) = ds(vec![("raw_ax2", raw_ax2), ("calibrated_az", cal_az)]);
        let f = health_check(&d, &nc, None);
        let ev = f.iter().find(|x| x.kind == "regime_change_event").expect("an event");
        assert!(ev.culprit.as_ref().unwrap().iter().any(|c| c == "raw_ax2"));
    }
}
