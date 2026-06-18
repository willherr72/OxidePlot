//! Read-only table view logic over `LoadedData`: filtering (global search +
//! per-column) and sorting (numeric-aware) into a view-order row index, plus
//! windowed row extraction. Pure + native-testable; the wasm layer wraps this.

use std::collections::HashMap;
use crate::data::loader::LoadedData;

#[derive(Clone, Debug)]
pub enum ColFilter {
    /// Case-insensitive substring match.
    Text(String),
    /// Numeric range; `None` bound = unbounded. Unparseable cells fail the filter.
    Range { min: Option<f64>, max: Option<f64> },
}

#[derive(Clone, Default)]
pub struct TableQuery {
    /// (column index, ascending). None = original row order.
    pub sort: Option<(usize, bool)>,
    /// Global case-insensitive substring filter across all columns. Empty = off.
    pub search: String,
    /// Per-column filters (column index -> filter).
    pub col_filters: HashMap<usize, ColFilter>,
    /// Per-column: true if the column should sort/filter numerically.
    pub numeric_cols: Vec<bool>,
}

#[inline]
fn cell<'a>(data: &'a LoadedData, col: usize, row: usize) -> &'a str {
    data.column_data
        .get(col)
        .and_then(|c| c.get(row))
        .map(|s| s.as_str())
        .unwrap_or("")
}

fn is_numeric(q: &TableQuery, col: usize) -> bool {
    q.numeric_cols.get(col).copied().unwrap_or(false)
}

fn row_passes(data: &LoadedData, q: &TableQuery, row: usize) -> bool {
    // Global search: any column contains the term (case-insensitive).
    if !q.search.is_empty() {
        let needle = q.search.to_lowercase();
        let hit = (0..data.column_data.len())
            .any(|c| cell(data, c, row).to_lowercase().contains(&needle));
        if !hit {
            return false;
        }
    }
    // Per-column filters (all must pass).
    for (&col, filt) in &q.col_filters {
        let val = cell(data, col, row);
        let ok = match filt {
            ColFilter::Text(s) => {
                s.is_empty() || val.to_lowercase().contains(&s.to_lowercase())
            }
            ColFilter::Range { min, max } => match val.trim().parse::<f64>() {
                Ok(v) => min.map_or(true, |lo| v >= lo) && max.map_or(true, |hi| v <= hi),
                Err(_) => false,
            },
        };
        if !ok {
            return false;
        }
    }
    true
}

/// Filtered (search + per-column) then sorted row indices, in display order.
pub fn compute_view_index(data: &LoadedData, q: &TableQuery) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..data.row_count).filter(|&r| row_passes(data, q, r)).collect();

    if let Some((col, ascending)) = q.sort {
        if is_numeric(q, col) {
            // Numeric: parse to f64; unparseable/non-finite sort to the end.
            let key = |r: &usize| cell(data, col, *r).trim().parse::<f64>().ok()
                .filter(|v| v.is_finite());
            idx.sort_by(|a, b| {
                let (ka, kb) = (key(a), key(b));
                let ord = match (ka, kb) {
                    (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                    (Some(_), None) => std::cmp::Ordering::Less,   // values before blanks
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                };
                if ascending { ord } else { ord.reverse() }
            });
        } else {
            idx.sort_by(|a, b| {
                let ord = cell(data, col, *a).cmp(cell(data, col, *b));
                if ascending { ord } else { ord.reverse() }
            });
        }
    }
    idx
}

/// Extract `count` rows starting at `start` (clamped) from `view_index`, each row
/// = all columns' cells as owned strings.
pub fn window_rows(data: &LoadedData, view_index: &[usize], start: usize, count: usize) -> Vec<Vec<String>> {
    let end = start.saturating_add(count).min(view_index.len());
    let cols = data.column_data.len();
    view_index[start.min(view_index.len())..end]
        .iter()
        .map(|&r| (0..cols).map(|c| cell(data, c, r).to_string()).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::loader::LoadedData;
    use std::collections::HashMap;

    fn fixture() -> LoadedData {
        LoadedData {
            columns: vec!["n".into(), "name".into()],
            column_data: vec![
                vec!["2".into(), "10".into(), "1".into()],          // col 0: numeric
                vec!["bob".into(), "alice".into(), "carol".into()], // col 1: text
            ],
            row_count: 3,
        }
    }
    fn q() -> TableQuery { TableQuery { numeric_cols: vec![true, false], ..Default::default() } }

    #[test]
    fn numeric_sort_orders_by_value_not_lexically() {
        let d = fixture();
        let query = TableQuery { sort: Some((0, true)), ..q() };
        assert_eq!(compute_view_index(&d, &query), vec![2, 0, 1]); // 1,2,10
    }
    #[test]
    fn numeric_sort_descending_reverses() {
        let d = fixture();
        let query = TableQuery { sort: Some((0, false)), ..q() };
        assert_eq!(compute_view_index(&d, &query), vec![1, 0, 2]); // 10,2,1
    }
    #[test]
    fn text_sort_is_lexical() {
        let d = fixture();
        let query = TableQuery { sort: Some((1, true)), ..q() };
        assert_eq!(compute_view_index(&d, &query), vec![1, 0, 2]); // alice,bob,carol
    }
    #[test]
    fn search_filters_case_insensitive_across_columns() {
        let d = fixture();
        let query = TableQuery { search: "AL".into(), ..q() };
        assert_eq!(compute_view_index(&d, &query), vec![1]); // "alice"
    }
    #[test]
    fn numeric_range_filter_retains_in_range() {
        let d = fixture();
        let mut f = HashMap::new();
        f.insert(0, ColFilter::Range { min: Some(2.0), max: None });
        let query = TableQuery { col_filters: f, ..q() };
        assert_eq!(compute_view_index(&d, &query), vec![0, 1]); // 2,10
    }
    #[test]
    fn text_column_filter_substring() {
        let d = fixture();
        let mut f = HashMap::new();
        f.insert(1, ColFilter::Text("o".into()));
        let query = TableQuery { col_filters: f, ..q() };
        assert_eq!(compute_view_index(&d, &query), vec![0, 2]); // bob, carol
    }
    #[test]
    fn window_rows_slices_in_view_order() {
        let d = fixture();
        let rows = window_rows(&d, &[2, 0, 1], 0, 2);
        assert_eq!(rows, vec![
            vec!["1".to_string(), "carol".to_string()],
            vec!["2".to_string(), "bob".to_string()],
        ]);
    }
    #[test]
    fn window_rows_clamps_out_of_bounds() {
        let d = fixture();
        let rows = window_rows(&d, &[0, 1, 2], 2, 10);
        assert_eq!(rows.len(), 1); // only row at index 2 remains
    }
}
