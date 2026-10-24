mod commands;
mod database;
mod importing;
mod repository;
mod thumbnails;

use std::sync::Mutex;
use tokio::sync::Mutex as AsyncMutex;

pub struct AppState {
    repository_path: Mutex<Option<std::path::PathBuf>>,
    import_lock: AsyncMutex<()>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            repository_path: Mutex::new(None),
            import_lock: AsyncMutex::new(()),
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_repository_status,
            commands::open_repository,
            commands::import_archive
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Fairy Comic Library");
}
