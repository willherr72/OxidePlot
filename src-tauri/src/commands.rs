#[tauri::command]
pub fn pick_file() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("data", &["csv", "xlsx", "xls"])
        .pick_file()
        .map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn read_file(path: String) -> Result<Vec<u8>, String> {
    std::fs::read(&path).map_err(|e| e.to_string())
}

/// Open a native save-file dialog and write `contents` to the chosen path.
///
/// Returns the chosen path as a string, or `Ok(None)` if the user cancelled.
/// Returns `Err(String)` if the write fails.
#[tauri::command]
pub fn save_file(default_name: String, contents: Vec<u8>) -> Result<Option<String>, String> {
    let path = rfd::FileDialog::new()
        .set_file_name(&default_name)
        .save_file();

    match path {
        None => Ok(None),
        Some(p) => {
            std::fs::write(&p, &contents).map_err(|e| e.to_string())?;
            Ok(Some(p.to_string_lossy().into_owned()))
        }
    }
}
