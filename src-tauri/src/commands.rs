use tauri::Manager;

#[tauri::command]
pub fn load_prefs(app: tauri::AppHandle) -> Result<String, String> {
    let config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let prefs_path = config_dir.join("prefs.json");
    if prefs_path.exists() {
        std::fs::read_to_string(&prefs_path).map_err(|e| e.to_string())
    } else {
        Ok("{}".into())
    }
}

#[tauri::command]
pub fn save_prefs(app: tauri::AppHandle, contents: String) -> Result<(), String> {
    let config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    let prefs_path = config_dir.join("prefs.json");
    std::fs::write(&prefs_path, contents).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pick_file() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("data", &["csv", "dat", "txt", "tsv", "xlsx", "xls"])
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
