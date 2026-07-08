use std::path::Path;
use crate::data::parser;

/// Result of loading a data file: column names and column data as strings
pub struct LoadedData {
    pub columns: Vec<String>,
    pub column_data: Vec<Vec<String>>,  // column-major: column_data[col_idx][row_idx]
    pub row_count: usize,
}

/// Metadata struct for the WASM boundary — serializable column+row summary.
#[derive(serde::Serialize)]
pub struct ColumnMeta {
    pub name: String,
    pub kind: String,
}

#[derive(serde::Serialize)]
pub struct FileMeta {
    pub columns: Vec<ColumnMeta>,
    pub rows: usize,
}

impl FileMeta {
    pub fn from_loaded(data: &LoadedData) -> Self {
        let columns = data.columns.iter().zip(data.column_data.iter()).map(|(name, col)| {
            let kind = if column_to_timestamps(col).is_some() {
                "datetime".to_string()
            } else {
                let (_, frac) = column_to_f64(col);
                if frac >= 0.5 {
                    "numeric".to_string()
                } else {
                    "text".to_string()
                }
            };
            ColumnMeta { name: name.clone(), kind }
        }).collect();

        FileMeta {
            columns,
            rows: data.row_count,
        }
    }
}

/// Load from raw bytes, dispatching by the extension of `filename`.
/// This is the primary entry point for the WASM path (bytes already read by Tauri/JS).
pub fn load_from_bytes(bytes: &[u8], filename: &str) -> Result<LoadedData, String> {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "csv" | "dat" | "txt" | "tsv" => load_csv_from_bytes(bytes),
        "xls" | "xlsx" => load_excel_from_bytes(bytes),
        _ => Err(format!("Unsupported file format: .{ext}")),
    }
}

/// Load a CSV or Excel file and return the column names and raw string data.
pub fn load_file(path: &Path) -> Result<LoadedData, String> {
    let ext = path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "csv" | "dat" | "txt" | "tsv" => load_csv(path),
        "xls" | "xlsx" => load_excel(path),
        _ => Err(format!("Unsupported file format: .{ext}")),
    }
}

/// Finalize a freshly-parsed table: merge adjacent Date+Time column pairs into
/// single datetime columns, then build the `LoadedData`.
fn finalize_loaded(
    mut columns: Vec<String>,
    mut column_data: Vec<Vec<String>>,
    row_count: usize,
) -> LoadedData {
    // Give blank header cells a generic label (common in instrument exports where
    // e.g. the frequency/index column is left unnamed) so they read clearly and
    // are selectable in the column picker rather than showing as empty.
    for (i, name) in columns.iter_mut().enumerate() {
        if name.trim().is_empty() {
            *name = format!("Column {}", i + 1);
        }
    }
    merge_date_time_columns(&mut columns, &mut column_data);
    LoadedData { columns, column_data, row_count }
}

/// Merge an adjacent date-only + time-only column pair (a common instrument-log
/// export shape, e.g. `Date` = "07/07/2026" and `Time` = "02:55:14 PM") into a
/// single datetime column whose cells are "date time", so it's detected and
/// plotted as a proper timestamp axis instead of a constant date + a text time.
fn merge_date_time_columns(columns: &mut Vec<String>, column_data: &mut Vec<Vec<String>>) {
    use crate::data::datetime::{is_date_only_column, is_time_only_column};
    let mut i = 0;
    while i + 1 < columns.len() {
        if is_date_only_column(&column_data[i]) && is_time_only_column(&column_data[i + 1]) {
            let merged_name = format!("{} {}", columns[i].trim(), columns[i + 1].trim());
            let merged: Vec<String> = column_data[i]
                .iter()
                .zip(column_data[i + 1].iter())
                .map(|(d, t)| {
                    let (d, t) = (d.trim(), t.trim());
                    if d.is_empty() || t.is_empty() {
                        String::new()
                    } else {
                        format!("{d} {t}")
                    }
                })
                .collect();
            columns[i] = merged_name;
            column_data[i] = merged;
            columns.remove(i + 1);
            column_data.remove(i + 1);
        }
        i += 1;
    }
}

/// Auto-detect the field delimiter (tab, comma, semicolon, or pipe) of a
/// delimited-text file — so generic `.dat`/`.txt`/`.tsv` exports load without
/// specialization, and non-comma CSVs work too. Scores each candidate by how
/// consistently it splits rows into the same number of (>1) columns, sampling
/// from the END of the file so an instrument metadata preamble doesn't skew it.
/// Falls back to comma.
fn detect_delimiter(bytes: &[u8]) -> u8 {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return b',';
    }
    // Data rows live at the end; sample the last 40 non-empty lines.
    let sample: Vec<&str> = lines.iter().rev().take(40).copied().collect();
    let mut best = b',';
    let mut best_score = 0.0_f64;
    for &d in &[b'\t', b',', b';', b'|'] {
        let dc = d as char;
        let counts: Vec<usize> = sample.iter().map(|l| l.matches(dc).count() + 1).collect();
        let max_cols = *counts.iter().max().unwrap_or(&1);
        if max_cols <= 1 {
            continue; // this delimiter doesn't split these rows
        }
        // Most common column count (>1) and how consistently it appears.
        let mut modal = 1usize;
        let mut modal_freq = 0usize;
        for c in 2..=max_cols {
            let f = counts.iter().filter(|&&x| x == c).count();
            if f > modal_freq {
                modal_freq = f;
                modal = c;
            }
        }
        let score = (modal_freq as f64 / counts.len() as f64) * modal as f64;
        if score > best_score {
            best_score = score;
            best = d;
        }
    }
    best
}

/// Parse a delimited-text file (CSV/DAT/TXT/TSV) from raw bytes, auto-detecting
/// the delimiter (core implementation).
pub fn load_csv_from_bytes(bytes: &[u8]) -> Result<LoadedData, String> {
    let delimiter = detect_delimiter(bytes);
    let header_row = parser::detect_csv_header_from_bytes(bytes, delimiter, 50)?;

    let text = String::from_utf8(bytes.to_vec())
        .unwrap_or_else(|_| bytes.iter().map(|&b| b as char).collect());

    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());

    let mut all_rows: Vec<Vec<String>> = Vec::new();
    for result in reader.records() {
        match result {
            Ok(record) => {
                all_rows.push(record.iter().map(|s| s.to_string()).collect());
            }
            Err(_) => continue,
        }
    }

    if all_rows.is_empty() || header_row >= all_rows.len() {
        return Err("No data found after header detection".to_string());
    }

    let columns: Vec<String> = all_rows[header_row].iter()
        .map(|s| s.trim().to_string())
        .collect();

    let data_rows = &all_rows[header_row + 1..];
    let num_cols = columns.len();

    let mut column_data: Vec<Vec<String>> = vec![Vec::new(); num_cols];
    let row_count = data_rows.len();

    for row in data_rows {
        for (col_idx, col_data) in column_data.iter_mut().enumerate() {
            if col_idx < row.len() {
                col_data.push(row[col_idx].clone());
            } else {
                col_data.push(String::new());
            }
        }
    }

    Ok(finalize_loaded(columns, column_data, row_count))
}

/// Path-based shim: read file then delegate to load_csv_from_bytes (legacy support).
fn load_csv(path: &Path) -> Result<LoadedData, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read file: {e}"))?;
    load_csv_from_bytes(&bytes)
}

/// Parse Excel from raw bytes using calamine's reader-based API.
pub fn load_excel_from_bytes(bytes: &[u8]) -> Result<LoadedData, String> {
    use calamine::{Reader, Data};
    use std::io::Cursor;

    let cursor = Cursor::new(bytes.to_vec());
    let mut workbook = calamine::open_workbook_auto_from_rs(cursor)
        .map_err(|e| format!("Cannot open Excel file from bytes: {e}"))?;

    let header_row = parser::detect_excel_header_from_workbook(&mut workbook, 50)?;

    // Re-open since we consumed the workbook for header detection — reset by re-creating
    // Actually we can't seek back; instead detect header first then re-open.
    // Workaround: re-create from the same bytes.
    let cursor2 = Cursor::new(bytes.to_vec());
    let mut workbook2 = calamine::open_workbook_auto_from_rs(cursor2)
        .map_err(|e| format!("Cannot re-open Excel file from bytes: {e}"))?;

    let sheet_name = workbook2.sheet_names().first()
        .ok_or("No sheets found")?
        .clone();

    let range = workbook2.worksheet_range(&sheet_name)
        .map_err(|e| format!("Cannot read sheet: {e}"))?;

    let all_rows: Vec<Vec<String>> = range.rows().map(|row| {
        row.iter().map(|cell| {
            match cell {
                Data::Empty => String::new(),
                Data::String(s) => s.clone(),
                Data::Float(f) => f.to_string(),
                Data::Int(i) => i.to_string(),
                Data::Bool(b) => b.to_string(),
                Data::DateTime(dt) => dt.to_string(),
                Data::DateTimeIso(s) => s.clone(),
                Data::DurationIso(s) => s.clone(),
                Data::Error(e) => format!("{e:?}"),
            }
        }).collect()
    }).collect();

    if all_rows.is_empty() || header_row >= all_rows.len() {
        return Err("No data found after header detection".to_string());
    }

    let columns: Vec<String> = all_rows[header_row].iter()
        .map(|s| s.trim().to_string())
        .collect();

    let data_rows = &all_rows[header_row + 1..];
    let num_cols = columns.len();
    let mut column_data: Vec<Vec<String>> = vec![Vec::new(); num_cols];
    let row_count = data_rows.len();

    for row in data_rows {
        for (col_idx, col_data) in column_data.iter_mut().enumerate() {
            if col_idx < row.len() {
                col_data.push(row[col_idx].clone());
            } else {
                col_data.push(String::new());
            }
        }
    }

    Ok(finalize_loaded(columns, column_data, row_count))
}

/// Path-based shim: read file then delegate to load_excel_from_bytes (legacy support).
fn load_excel(path: &Path) -> Result<LoadedData, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read file: {e}"))?;
    load_excel_from_bytes(&bytes)
}

/// Extract numeric f64 values from a string column.
/// Returns (values, fraction_valid) where invalid entries become NaN.
pub fn column_to_f64(data: &[String]) -> (Vec<f64>, f64) {
    let mut values = Vec::with_capacity(data.len());
    let mut valid = 0usize;
    for s in data {
        match s.trim().parse::<f64>() {
            Ok(v) => {
                values.push(v);
                if v.is_finite() { valid += 1; }
            }
            Err(_) => values.push(f64::NAN),
        }
    }
    let frac = if data.is_empty() { 0.0 } else { valid as f64 / data.len() as f64 };
    (values, frac)
}

/// Try to parse a string column as datetime timestamps.
/// Returns Some((timestamps, fraction_valid)) if the column looks like dates.
pub fn column_to_timestamps(data: &[String]) -> Option<(Vec<f64>, f64)> {
    use crate::data::datetime::{detect_date_format, parse_to_timestamp};

    let format = detect_date_format(data)?;

    let mut timestamps = Vec::with_capacity(data.len());
    let mut valid = 0usize;
    for s in data {
        match parse_to_timestamp(s.trim(), format) {
            Some(ts) => {
                timestamps.push(ts);
                valid += 1;
            }
            None => timestamps.push(f64::NAN),
        }
    }

    let frac = if data.is_empty() { 0.0 } else { valid as f64 / data.len() as f64 };
    if frac > 0.7 {
        Some((timestamps, frac))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_csv_from_bytes_detects_columns() {
        let csv = b"time,temp\n0,20.0\n1,21.5\n";
        let d = load_from_bytes(csv, "x.csv").unwrap();
        assert_eq!(d.columns, vec!["time".to_string(), "temp".to_string()]);
        assert_eq!(d.row_count, 2);
    }

    #[test]
    fn loads_tab_delimited_dat_with_preamble() {
        // QDaq-style .dat: a metadata preamble, then a tab-delimited data table.
        // The generic loader must auto-detect the tab delimiter and skip the
        // preamble to the 4-column (Frequency + 3 channel) data.
        let dat = "QDaq Spectrum Analyzer\n07/07/2026  03:33:57 PM\n\n\
Number of Channels\t3\t\t\tg RMS Calculation\n\
Frequency, Hz\tg(rms)/Hz\n\
\tT3 X\tT3 Y\tT3 Z\n\
6.100000\t7.131403E-3\t6.590730E-3\t9.327743E-3\n\
12.200351\t3.081071E-4\t5.749488E-4\t1.917445E-4\n\
18.300702\t2.344623E-3\t1.551256E-3\t1.012578E-3\n\
24.401052\t1.360337E-3\t5.362999E-4\t5.258780E-4\n";
        let d = load_from_bytes(dat.as_bytes(), "scan.dat").unwrap();
        assert_eq!(d.columns.len(), 4, "tab data table should yield 4 columns");
        assert_eq!(d.row_count, 4, "4 numeric data rows past the preamble");
        // Frequency + channel columns are numeric (scientific notation parses).
        let meta = FileMeta::from_loaded(&d);
        assert_eq!(meta.columns[0].kind, "numeric", "frequency column is numeric");
        assert_eq!(meta.columns[1].kind, "numeric", "channel column is numeric");
    }

    #[test]
    fn merges_adjacent_date_and_12h_time_columns() {
        // Separate Date (constant day) + 12-hour Time columns (instrument-log
        // export shape) must merge into one non-degenerate datetime axis.
        let csv = b"Date,Time,Temp\n07/07/2026,02:55:14 PM,21.7\n07/07/2026,02:55:15 PM,21.6\n";
        let d = load_from_bytes(csv, "chamber.csv").unwrap();
        assert_eq!(d.columns.len(), 2, "Date+Time should collapse to one column + Temp");
        assert_eq!(d.columns[0], "Date Time");
        let meta = FileMeta::from_loaded(&d);
        assert_eq!(meta.columns[0].kind, "datetime", "merged column must be datetime");
        let (ts, _) = column_to_timestamps(&d.column_data[0]).expect("merged col parses to timestamps");
        assert!(ts[1] > ts[0], "merged timestamps must advance (real time axis)");
    }

    #[test]
    fn file_meta_from_loaded() {
        let csv = b"x,y,z\n1,2,3\n4,5,6\n";
        let d = load_from_bytes(csv, "data.csv").unwrap();
        let meta = FileMeta::from_loaded(&d);
        assert_eq!(meta.columns.len(), 3);
        assert_eq!(meta.rows, 2);
        assert_eq!(meta.columns[0].name, "x");
    }

    #[test]
    fn column_meta_kind_numeric() {
        // time and temp are both numeric (0/1 and 20.0/21.5 parse as f64)
        let csv = b"time,temp\n0,20.0\n1,21.5\n";
        let d = load_from_bytes(csv, "x.csv").unwrap();
        let meta = FileMeta::from_loaded(&d);
        assert_eq!(meta.columns[0].name, "time");
        assert_eq!(meta.columns[0].kind, "numeric");
        assert_eq!(meta.columns[1].name, "temp");
        assert_eq!(meta.columns[1].kind, "numeric");
    }

    #[test]
    fn column_meta_kind_text() {
        // label column has string values that cannot be parsed as numbers or dates
        let csv = b"label,value\nalpha,1\nbeta,2\ngamma,3\n";
        let d = load_from_bytes(csv, "x.csv").unwrap();
        let meta = FileMeta::from_loaded(&d);
        assert_eq!(meta.columns[0].name, "label");
        assert_eq!(meta.columns[0].kind, "text");
        assert_eq!(meta.columns[1].name, "value");
        assert_eq!(meta.columns[1].kind, "numeric");
    }

    #[test]
    fn column_meta_kind_datetime() {
        // timestamp column should be detected as datetime
        let csv = b"ts,val\n2024-01-01 00:00:00,10\n2024-01-02 00:00:00,20\n2024-01-03 00:00:00,30\n";
        let d = load_from_bytes(csv, "x.csv").unwrap();
        let meta = FileMeta::from_loaded(&d);
        assert_eq!(meta.columns[0].name, "ts");
        assert_eq!(meta.columns[0].kind, "datetime");
        assert_eq!(meta.columns[1].name, "val");
        assert_eq!(meta.columns[1].kind, "numeric");
    }

    #[test]
    fn unsupported_extension_errors() {
        let result = load_from_bytes(b"data", "file.json");
        assert!(result.is_err());
    }
}

/// Resolve a column reference (name or numeric index string) to a column index.
pub fn resolve_col(data: &LoadedData, spec: &str) -> Option<usize> {
    if let Ok(i) = spec.parse::<usize>() {
        if i < data.columns.len() {
            return Some(i);
        }
    }
    data.columns.iter().position(|c| c == spec)
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    fn ld() -> LoadedData {
        LoadedData {
            columns: vec!["time".into(), "temp".into(), "pressure".into()],
            column_data: vec![vec![], vec![], vec![]],
            row_count: 0,
        }
    }

    #[test]
    fn resolve_by_name_and_index() {
        let d = ld();
        assert_eq!(resolve_col(&d, "temp"), Some(1));
        assert_eq!(resolve_col(&d, "2"), Some(2));
        assert_eq!(resolve_col(&d, "nope"), None);
        assert_eq!(resolve_col(&d, "9"), None); // out of range
    }
}
