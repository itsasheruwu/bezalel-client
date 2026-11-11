use tauri::Manager;

pub fn apply(app: &tauri::AppHandle, preference: &str) -> Result<(), String> {
    let selected = match preference {
        "light" => Some(tauri::Theme::Light),
        "dark" => Some(tauri::Theme::Dark),
        "system" => None,
        _ => return Err("Choose Light, Dark, or Sync with system.".into()),
    };
    let window = app
        .get_webview_window("main")
        .ok_or("App window is unavailable.")?;
    window.set_theme(selected).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        #[link(name = "dwmapi")]
        unsafe extern "system" {
            fn DwmSetWindowAttribute(
                window: *mut c_void,
                attribute: u32,
                value: *const c_void,
                size: u32,
            ) -> i32;
        }
        let dark =
            selected.unwrap_or(window.theme().map_err(|e| e.to_string())?) == tauri::Theme::Dark;
        let caption: u32 = if dark { 0x00141414 } else { 0x00f8f8f8 };
        let text: u32 = if dark { 0x00e4e4e4 } else { 0x00141414 };
        let hwnd = window.hwnd().map_err(|e| e.to_string())?;
        unsafe {
            DwmSetWindowAttribute(hwnd.0 as _, 35, (&caption as *const u32).cast(), 4);
            DwmSetWindowAttribute(hwnd.0 as _, 36, (&text as *const u32).cast(), 4);
        }
    }
    Ok(())
}
