use crate::{repository, AppState};
use std::path::PathBuf;
use tauri::{AppHandle, State};

#[tauri::command]
pub fn get_repository_status(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<repository::RepositoryStatus>, String> {
    let saved_status = repository::load_saved_repository(&app)?;
    if let Some(status) = saved_status {
        *state.repository_path.lock().map_err(|_| {
            "リポジトリ状態を更新できませんでした。アプリを再起動してください。".to_string()
        })? = Some(PathBuf::from(&status.path));
        return Ok(Some(status));
    }

    let path = state.repository_path.lock().map_err(|_| {
        "リポジトリ状態を読み取れませんでした。アプリを再起動してください。".to_string()
    })?;
    match path.as_ref() {
        Some(path) => repository::open(path).map(Some),
        None => Ok(None),
    }
}

#[tauri::command]
pub fn open_repository(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<repository::RepositoryStatus, String> {
    let status = repository::open(&PathBuf::from(path))?;
    repository::save_selected_repository(&app, PathBuf::from(&status.path).as_path())?;
    *state.repository_path.lock().map_err(|_| {
        "リポジトリ状態を更新できませんでした。アプリを再起動してください。".to_string()
    })? = Some(PathBuf::from(&status.path));
    Ok(status)
}

#[tauri::command]
pub async fn import_archive(
    path: String,
    state: State<'_, AppState>,
) -> Result<crate::importing::ImportOutcome, String> {
    let repository_path = state
        .repository_path
        .lock()
        .map_err(|_| {
            "リポジトリ状態を読み取れませんでした。アプリを再起動してください。".to_string()
        })?
        .clone()
        .ok_or_else(|| "先にアプリ管理リポジトリを選択してください。".to_string())?;

    let _import_guard = state.import_lock.lock().await;
    crate::importing::import_archive(&repository_path, &PathBuf::from(path)).await
}
