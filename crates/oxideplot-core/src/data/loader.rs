use std::path::Path;
use crate::data::parser;

/// Result of loading a data file: column names and column data as strings
pub struct LoadedData {
    pub columns: Vec<String>,
    pub column_data: Vec<Vec<String>>,  // column-major: column_data[col_idx][row_idx]
    pub row_count: usize,
}

/// Load a CSV or Excel file and return the column names and raw string data.
pub fn load_file(path: &Path) -> Result<LoadedData, String> {
    let ext = path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "csv" => load_csv(path),
        "xls" | "xlsx" => load_excel(path),
        _ => Err(format!("Unsupported file format: .{ext}")),
    }
}

fn load_csv(path: &Path) -> Result<LoadedData, String> {
    let header_row = parser::detect_csv_header(path, b',', 50)?;

    // Read the file bytes
    let content = std::fs::read(path).map_err(|e| format!("Cannot read file: {e}"))?;
    let text = String::from_utf8(content.clone())
        .unwrap_or_else(|_| content.iter().map(|&b| b as char).collect());

    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b',')
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

    // Extract column names from header row
    let columns: Vec<String> = all_rows[header_row].iter()
        .map(|s| s.trim().to_string())
        .collect();

    // Data starts after header row
    let data_rows = &all_rows[header_row + 1..];
    let num_cols = columns.len();

    // Convert to column-major format
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

    Ok(LoadedData { columns, column_data, row_count })
}

fn load_excel(path: &Path) -> Result<LoadedData, String> {
    use calamine::{open_workbook_auto, Reader, Data};

    let header_row = parser::detect_excel_header(path, 50)?;

    let mut workbook = open_workbook_auto(path)
        .map_err(|e| format!("Cannot open Excel file: {e}"))?;

    let sheet_name = workbook.sheet_names().first()
        .ok_or("No sheets found")?
        .clone();

    let range = workbook.worksheet_range(&sheet_name)
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

    Ok(LoadedData { columns, column_data, row_count })
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
