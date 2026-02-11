/// Infer the measurement unit from a column name.
/// Matches the Python version's inferUnit function.
pub fn infer_unit(column_name: &str) -> String {
    let lower = column_name.to_lowercase();

    if lower.contains("inc") || lower.contains("azi") || lower.contains("tool-face") || lower.contains("angle") {
        "Degrees".to_string()
    } else if lower.contains("vibe") || lower.contains("accel") || lower.contains("shock")
        || lower.contains("gx") || lower.contains("gy") || lower.contains("gz") || lower.contains("grav") {
        "G".to_string()
    } else if lower.contains("temp") {
        "\u{00B0}C".to_string()
    } else if lower.contains("gamma") {
        "CPS".to_string()
    } else if lower.contains("pulse") || lower.contains("flow") {
        "Counts".to_string()
    } else if lower.contains("1v8") || lower.contains("5v") || lower.contains("3v3")
        || lower.contains("bat") || lower.contains("bus") {
        "V".to_string()
    } else if lower.contains("current") {
        "mA".to_string()
    } else if lower.contains("mag") || lower.contains("mx") || lower.contains("my") || lower.contains("mz") {
        "Gauss".to_string()
    } else if lower.contains("rpm") {
        "RPM".to_string()
    } else {
        "units".to_string()
    }
}
