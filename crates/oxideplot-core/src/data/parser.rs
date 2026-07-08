use std::path::Path;
use std::collections::HashMap;

/// Detect header row index from raw CSV bytes.
/// Returns the 0-based row index of the header row.
pub fn detect_csv_header_from_bytes(bytes: &[u8], delimiter: u8, max_lines: usize) -> Result<usize, String> {
    let text = String::from_utf8(bytes.to_vec())
        .unwrap_or_else(|_| {
            bytes.iter().map(|&b| b as char).collect()
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
            if trimmed.parse::<f64>().is_ok() {
                all_strings = false;
                break;
            }
            if is_date_like(trimmed) {
                all_strings = false;
                break;
            }
        }

        if all_strings && has_content {
            return Ok(i);
        }
    }

    // No clean all-string header found. Fall back to the FIRST row that matches
    // the most-common column count — this skips a title/metadata preamble (whose
    // rows have odd column counts) to the start of the real data block, even when
    // that block's header has blank cells (e.g. an unnamed index/frequency
    // column in an instrument .dat export). Better than blindly using row 0.
    for (i, row) in rows.iter().enumerate() {
        if row.len() == most_common {
            return Ok(i);
        }
    }

    Ok(0)
}

/// Detect header row index in a CSV file (path-based shim — reads file then delegates).
pub fn detect_csv_header(filepath: &Path, delimiter: u8, max_lines: usize) -> Result<usize, String> {
    let content = std::fs::read(filepath).map_err(|e| format!("Cannot read file: {e}"))?;
    detect_csv_header_from_bytes(&content, delimiter, max_lines)
}

/// Detect header row from an already-open calamine workbook (used by bytes path).
pub fn detect_excel_header_from_workbook<RS>(
    workbook: &mut calamine::Sheets<RS>,
    max_rows: usize,
) -> Result<usize, String>
where
    RS: std::io::Read + std::io::Seek,
{
    use calamine::{Reader, Data};

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

    detect_header_from_rows(&rows)
}

/// Detect header row index in an Excel file (path-based shim — delegates to workbook variant).
pub fn detect_excel_header(filepath: &Path, max_rows: usize) -> Result<usize, String> {
    use calamine::open_workbook_auto;

    let mut workbook = open_workbook_auto(filepath)
        .map_err(|e| format!("Cannot open Excel file: {e}"))?;

    detect_excel_header_from_workbook(&mut workbook, max_rows)
}

/// Shared logic: given rows of optional-string cells, find the header row index.
fn detect_header_from_rows(rows: &[Vec<Option<String>>]) -> Result<usize, String> {
    if rows.is_empty() {
        return Err("No data in sheet".to_string());
    }

    let used_cols = rows.iter()
        .flat_map(|row| row.iter().enumerate().filter(|(_, c)| c.is_some()).map(|(i, _)| i))
        .collect::<std::collections::HashSet<_>>()
        .len();

    for i in (0..rows.len()).rev() {
        let row = &rows[i];
        let non_empty_count = row.iter().filter(|c| c.is_some()).count();
        if non_empty_count < used_cols { continue; }

        let mut all_strings = true;
        for cell in row {
            if let Some(val) = cell {
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
    let has_separators = s.contains('/') || s.contains(':');
    let has_date_words = s.to_lowercase().contains("am") || s.to_lowercase().contains("pm");

    if !has_separators && !has_date_words {
        return false;
    }

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
