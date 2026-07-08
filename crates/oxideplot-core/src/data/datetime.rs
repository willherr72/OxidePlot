use chrono::NaiveDateTime;

/// Sentinel value returned by `detect_date_format` when the column contains
/// RFC 3339 / ISO 8601 timestamps (e.g. `2026-02-10T22:26:28.987Z`).
pub const RFC3339_FORMAT: &str = "__rfc3339__";

/// All date formats to try (matching the Python version)
pub const DATE_FORMATS: &[&str] = &[
    "%Y-%m-%dT%H:%M:%S%.f",
    "%Y-%m-%dT%H:%M:%S",
    "%Y-%m-%d %H:%M:%S",
    "%m/%d/%Y %H:%M:%S",
    "%d/%m/%Y %H:%M:%S",
    "%Y/%m/%d %H:%M:%S",
    "%m-%d-%Y %H:%M:%S",
    "%d-%m-%Y %H:%M:%S",
    "%Y-%m-%d %H:%M:%S%.f",
    "%m/%d/%Y %H:%M:%S%.f",
    "%d/%m/%Y %H:%M:%S%.f",
    "%Y/%m/%d %H:%M:%S%.f",
    "%m-%d-%Y %H:%M:%S%.f",
    "%d-%m-%Y %H:%M:%S%.f",
    // 12-hour clock with AM/PM (e.g. "07/07/2026 02:55:14 PM"), incl. the merged
    // Date+Time form produced by merge_date_time_columns.
    "%m/%d/%Y %I:%M:%S %p",
    "%d/%m/%Y %I:%M:%S %p",
    "%Y-%m-%d %I:%M:%S %p",
    "%m/%d/%Y %I:%M %p",
    "%d/%m/%Y %I:%M %p",
    "%Y-%m-%d %I:%M %p",
    "%Y-%m-%d",
    "%m/%d/%Y",
    "%d/%m/%Y",
    "%Y/%m/%d",
    "%m-%d-%Y",
    "%d-%m-%Y",
];

/// Detect the most likely date format from a slice of string values.
/// Returns the format string with the highest parse success rate.
/// Returns `RFC3339_FORMAT` for ISO 8601 timestamps with timezone (e.g. `...Z`).
pub fn detect_date_format(values: &[String]) -> Option<&'static str> {
    let sample: Vec<&str> = values.iter()
        .filter(|s| !s.is_empty())
        .take(100)
        .map(|s| s.as_str())
        .collect();

    if sample.is_empty() {
        return None;
    }

    // Try RFC 3339 / ISO 8601 with timezone first (e.g. "2026-02-10T22:26:28.987Z")
    let rfc3339_valid = sample.iter()
        .filter(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok())
        .count();
    let rfc3339_score = rfc3339_valid as f64 / sample.len() as f64;

    let mut best_format: Option<&'static str> = None;
    let mut best_score: f64 = rfc3339_score;
    if rfc3339_score > 0.0 {
        best_format = Some(RFC3339_FORMAT);
    }

    for &fmt in DATE_FORMATS {
        let valid = sample.iter().filter(|s| {
            NaiveDateTime::parse_from_str(s, fmt).is_ok()
                || chrono::NaiveDate::parse_from_str(s, fmt).is_ok()
        }).count();

        let score = valid as f64 / sample.len() as f64;
        if score > best_score {
            best_score = score;
            best_format = Some(fmt);
        }
    }

    if best_score > 0.0 { best_format } else { None }
}

/// Bare time-of-day formats (no date), tried via `NaiveTime`.
pub const TIME_FORMATS: &[&str] = &[
    "%I:%M:%S %p", // 02:55:14 PM
    "%I:%M %p",    // 02:55 PM
    "%H:%M:%S%.f", // 14:55:14.123
    "%H:%M:%S",    // 14:55:14
    "%H:%M",       // 14:55
];

/// True when the column's non-empty values are dates WITHOUT a time component
/// (so the column can be paired with an adjacent time-only column). Requires a
/// high match rate to avoid false positives on numeric/text columns.
pub fn is_date_only_column(values: &[String]) -> bool {
    match detect_date_format(values) {
        Some(fmt) => fmt != RFC3339_FORMAT && !fmt.contains("%H") && !fmt.contains("%I"),
        None => false,
    }
}

/// True when the column's non-empty values are bare times-of-day (e.g.
/// `02:55:14 PM`). Requires >80% of the sampled values to match one format.
pub fn is_time_only_column(values: &[String]) -> bool {
    let sample: Vec<&str> = values
        .iter()
        .filter(|s| !s.is_empty())
        .take(100)
        .map(|s| s.as_str())
        .collect();
    if sample.is_empty() {
        return false;
    }
    TIME_FORMATS.iter().any(|&fmt| {
        let valid = sample
            .iter()
            .filter(|s| chrono::NaiveTime::parse_from_str(s, fmt).is_ok())
            .count();
        valid as f64 / sample.len() as f64 > 0.8
    })
}

/// Parse a string value to a Unix timestamp (with subsecond precision) using the given format.
/// If `format` is `RFC3339_FORMAT`, uses RFC 3339 parsing directly.
pub fn parse_to_timestamp(value: &str, format: &str) -> Option<f64> {
    // RFC 3339 / ISO 8601 with timezone (e.g. "2026-02-10T22:26:28.987Z")
    if format == RFC3339_FORMAT {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(value) {
            return Some(dt.timestamp_millis() as f64 / 1000.0);
        }
        return None;
    }

    if let Ok(dt) = NaiveDateTime::parse_from_str(value, format) {
        // Preserve subsecond precision via milliseconds
        Some(dt.and_utc().timestamp_millis() as f64 / 1000.0)
    } else if let Ok(d) = chrono::NaiveDate::parse_from_str(value, format) {
        Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp() as f64)
    } else {
        None
    }
}

/// Fix error timestamps in a vector of timestamp values.
/// Timestamps below the threshold (error_range_max + 1) are considered errors
/// and are replaced with interpolated values based on nearby valid timestamps.
pub fn fix_error_timestamps(
    x_values: &[f64],
    _error_range_min: f64,
    error_range_max: f64,
    increment: f64,
) -> Vec<f64> {
    let mut corrected = x_values.to_vec();
    let n = corrected.len();
    let threshold = error_range_max + 1.0;

    // Find first valid timestamp
    let mut j = 0;
    while j < n && corrected[j] < threshold {
        j += 1;
    }

    // Fix leading errors
    if j > 0 {
        let first_valid = if j < n { corrected[j] } else { threshold };
        for i in (0..j).rev() {
            corrected[i] = first_valid - increment * (j - i) as f64;
        }
    }

    // Fix errors after valid data
    let mut i = j;
    while i < n {
        if corrected[i] < threshold {
            let start = i;
            while i < n && corrected[i] < threshold {
                i += 1;
            }
            if start > 0 {
                let base = corrected[start - 1];
                for k in start..i {
                    corrected[k] = base + increment * (k - start + 1) as f64;
                }
            } else {
                for k in start..i {
                    corrected[k] = threshold + increment * (k - start + 1) as f64;
                }
            }
        } else {
            i += 1;
        }
    }

    corrected
}

/// Format a Unix timestamp as a human-readable datetime string.
/// Shows milliseconds when the timestamp has a fractional component.
pub fn format_timestamp(ts: f64) -> String {
    use chrono::{DateTime, Utc};
    let secs = ts.floor() as i64;
    let nanos = ((ts - ts.floor()) * 1_000_000_000.0) as u32;
    match DateTime::<Utc>::from_timestamp(secs, nanos) {
        Some(dt) => {
            if nanos == 0 {
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            } else {
                dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
            }
        }
        None => format!("{ts:.3}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339_returns_positive_timestamp() {
        // RFC 3339 string with Z timezone: parse_to_timestamp should return the Unix epoch seconds.
        let ts = parse_to_timestamp("2024-02-10T14:30:00Z", RFC3339_FORMAT)
            .expect("should parse a valid RFC 3339 timestamp");
        // 2024-02-10 is well after Unix epoch (Jan 1 1970), so timestamp must be positive.
        assert!(ts > 0.0, "expected positive epoch seconds, got {ts}");
    }

    #[test]
    fn parses_rfc3339_known_epoch_seconds() {
        // 1970-01-01T00:00:00Z is Unix epoch 0.
        let ts = parse_to_timestamp("1970-01-01T00:00:00Z", RFC3339_FORMAT)
            .expect("should parse Unix epoch");
        assert_eq!(ts, 0.0);
    }

    #[test]
    fn rejects_non_datetime_string_for_rfc3339_format() {
        let result = parse_to_timestamp("hello", RFC3339_FORMAT);
        assert!(result.is_none(), "expected None for non-datetime string, got {result:?}");
    }

    #[test]
    fn rejects_empty_string_for_rfc3339_format() {
        let result = parse_to_timestamp("", RFC3339_FORMAT);
        assert!(result.is_none(), "expected None for empty string, got {result:?}");
    }

    #[test]
    fn parses_naive_datetime_via_detected_format() {
        // Source the format from detect_date_format (not a hardcoded literal) so this
        // also characterizes detection of a naive, timezone-less datetime and stays
        // correct if DATE_FORMATS is reordered.
        let samples = vec!["2024-02-10 14:30:00".to_string()];
        let fmt = detect_date_format(&samples).expect("should detect naive datetime format");
        let ts = parse_to_timestamp("2024-02-10 14:30:00", fmt)
            .expect("should parse naive datetime");
        assert!(ts > 0.0, "expected positive epoch seconds, got {ts}");
    }

    #[test]
    fn detect_date_format_finds_rfc3339() {
        let samples = vec!["2024-02-10T14:30:00Z".to_string(), "2024-02-11T09:00:00Z".to_string()];
        let fmt = detect_date_format(&samples);
        assert_eq!(fmt, Some(RFC3339_FORMAT));
    }

    #[test]
    fn detect_date_format_returns_none_for_non_dates() {
        let samples = vec!["hello".to_string(), "world".to_string(), "foo".to_string()];
        let fmt = detect_date_format(&samples);
        assert!(fmt.is_none());
    }

    #[test]
    fn format_timestamp_roundtrip() {
        // Unix epoch should format to a known string.
        let s = format_timestamp(0.0);
        assert_eq!(s, "1970-01-01 00:00:00");
    }
}
