use std::path::Path;
use std::collections::HashMap;

/// Detect header row index in a CSV file.
/// Returns the 0-based row index of the header row.
pub fn detect_csv_header(filepath: &Path, delimiter: u8, max_lines: usize) -> Result<usize, String> {
    // Try UTF-8 first, then latin1 (read as bytes and convert)
    let content = std::fs::read(filepath).map_err(|e| format!("Cannot read file: {e}"))?;

    let text = String::from_utf8(content.clone())
        .unwrap_or_else(|_| {
            // Fallback: treat as latin1 (each byte maps to same Unicode code point)
            content.iter().map(|&b| b as char).collect()
        });

    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());

    let mut rows: Vec<Vec<String>> = Vec::new();
    for (i, result) in reader.records().enumerate() {
        if i >= max_lines { break; }
        match result {
            Ok(record) => {
                let row: Vec<String> = record.iter().map(|s| s.to_string()).collect();
                if !row.is_empty() {
                    rows.push(row);
                }
            }
            Err(_) => continue,
        }
    }

    if rows.is_empty() {
        return Err("No data found in file".to_string());
    }

    // Find most common column count
    let mut counts: HashMap<usize, usize> = HashMap::new();
    for row in &rows {
        *counts.entry(row.len()).or_insert(0) += 1;
    }
    let most_common = counts.into_iter().max_by_key(|&(_, c)| c).map(|(len, _)| len).unwrap_or(0);

    // Scan from bottom up for all-string row
    for i in (0..rows.len()).rev() {
        let row = &rows[i];
        if row.len() != most_common { continue; }

        let mut all_strings = true;
        let mut has_content = true;

        for cell in row {
            let trimmed = cell.trim();
            if trimmed.is_empty() {
                has_content = false;
                break;
            }
            // Try to parse as float
            if trimmed.parse::<f64>().is_ok() {
                all_strings = false;
                break;
            }
            // Check if it looks like a date
            if is_date_like(trimmed) {
                all_strings = false;
                break;
            }
        }

        if all_strings && has_content {
            return Ok(i);
        }
    }

    Ok(0)
}

/// Detect header row index in an Excel file.
pub fn detect_excel_header(filepath: &Path, max_rows: usize) -> Result<usize, String> {
    use calamine::{open_workbook_auto, Reader, Data};

    let mut workbook = open_workbook_auto(filepath)
        .map_err(|e| format!("Cannot open Excel file: {e}"))?;

    let sheet_name = workbook.sheet_names().first()
        .ok_or("No sheets found")?
        .clone();

    let range = workbook.worksheet_range(&sheet_name)
        .map_err(|e| format!("Cannot read sheet: {e}"))?;

    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    for (i, row) in range.rows().enumerate() {
        if i >= max_rows { break; }
        let cells: Vec<Option<String>> = row.iter().map(|cell| {
            match cell {
                Data::Empty => None,
                Data::String(s) => Some(s.clone()),
                Data::Float(f) => Some(f.to_string()),
                Data::Int(i) => Some(i.to_string()),
                Data::Bool(b) => Some(b.to_string()),
                Data::DateTime(dt) => Some(dt.to_string()),
                Data::DateTimeIso(s) => Some(s.clone()),
                Data::DurationIso(s) => Some(s.clone()),
                Data::Error(e) => Some(format!("{e:?}")),
            }
        }).collect();
        rows.push(cells);
    }

    if rows.is_empty() {
        return Err("No data in sheet".to_string());
    }

    // Count non-empty columns
    let used_cols = rows.iter()
        .flat_map(|row| row.iter().enumerate().filter(|(_, c)| c.is_some()).map(|(i, _)| i))
        .collect::<std::collections::HashSet<_>>()
        .len();

    // Scan from bottom up
    for i in (0..rows.len()).rev() {
        let row = &rows[i];
        let non_empty_count = row.iter().filter(|c| c.is_some()).count();
        if non_empty_count < used_cols { continue; }

        let mut all_strings = true;
        for cell in row {
            if let Some(val) = cell {
                // If it parses as a number, it's not a header
                if val.parse::<f64>().is_ok() {
                    all_strings = false;
                    break;
                }
                if is_date_like(val) {
                    all_strings = false;
                    break;
                }
            }
        }

        if all_strings && non_empty_count >= used_cols {
            return Ok(i);
        }
    }

    Ok(0)
}

fn is_date_like(s: &str) -> bool {
    // Check for date-like patterns
    let has_separators = s.contains('/') || s.contains(':');
    let has_date_words = s.to_lowercase().contains("am") || s.to_lowercase().contains("pm");

    if !has_separators && !has_date_words {
        return false;
    }

    // Try to parse with chrono
    use chrono::NaiveDateTime;
    let formats = [
        "%Y-%m-%d %H:%M:%S",
        "%m/%d/%Y %H:%M:%S",
        "%d/%m/%Y %H:%M:%S",
        "%Y-%m-%d",
        "%m/%d/%Y",
    ];
    for fmt in &formats {
        if NaiveDateTime::parse_from_str(s, fmt).is_ok() {
            return true;
        }
        if chrono::NaiveDate::parse_from_str(s, fmt).is_ok() {
            return true;
        }
    }
    false
}
