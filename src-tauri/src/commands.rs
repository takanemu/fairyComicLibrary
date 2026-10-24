use crate::{library, repository, AppState};
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

fn current_repository(state: &State<'_, AppState>) -> Result<PathBuf, String> {
    state
        .repository_path
        .lock()
        .map_err(|_| {
            "リポジトリ状態を読み取れませんでした。アプリを再起動してください。".to_string()
        })?
        .clone()
        .ok_or_else(|| "先にアプリ管理リポジトリを選択してください。".to_string())
}

#[tauri::command(async)]
pub fn list_archives(
    query: library::ArchiveQuery,
    state: State<'_, AppState>,
) -> Result<Vec<library::ArchiveSummary>, String> {
    library::list_archives(&current_repository(&state)?, &query)
}

#[tauri::command(async)]
pub fn list_tags(state: State<'_, AppState>) -> Result<Vec<library::TagWithCount>, String> {
    library::list_tags(&current_repository(&state)?)
}

#[tauri::command(async)]
pub fn set_favorite(
    archive_id: String,
    is_favorite: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    library::set_favorite(&current_repository(&state)?, &archive_id, is_favorite)
}

#[tauri::command(async)]
pub fn add_tag(
    archive_id: String,
    category: String,
    name: String,
    state: State<'_, AppState>,
) -> Result<library::TagSummary, String> {
    library::add_tag(&current_repository(&state)?, &archive_id, &category, &name)
}

#[tauri::command(async)]
pub fn remove_tag(
    archive_id: String,
    tag_id: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    library::remove_tag(&current_repository(&state)?, &archive_id, tag_id)
}
