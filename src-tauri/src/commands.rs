use crate::{library, repository, AppState};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};

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
pub fn add_tag_to_archives(
    archive_ids: Vec<String>,
    category: String,
    name: String,
    state: State<'_, AppState>,
) -> Result<library::TagSummary, String> {
    library::add_tag_to_archives(&current_repository(&state)?, &archive_ids, &category, &name)
}

#[tauri::command(async)]
pub fn remove_tag(
    archive_id: String,
    tag_id: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    library::remove_tag(&current_repository(&state)?, &archive_id, tag_id)
}

#[tauri::command(async)]
pub fn remove_tag_from_archives(
    archive_ids: Vec<String>,
    tag_id: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    library::remove_tag_from_archives(&current_repository(&state)?, &archive_ids, tag_id)
}

#[tauri::command]
pub async fn backup_repository(
    destination: String,
    state: State<'_, AppState>,
) -> Result<crate::maintenance::BackupSummary, String> {
    let repository_path = current_repository(&state)?;
    // インポートと同時に走らせると、ファイルとDBの対応が崩れた状態でコピーしかねない。
    let _import_guard = state.import_lock.lock().await;
    tokio::task::spawn_blocking(move || {
        crate::maintenance::backup(&repository_path, &PathBuf::from(destination))
    })
    .await
    .map_err(|error| format!("バックアップ処理に失敗しました: {error}"))?
}

#[tauri::command]
pub async fn unmanage_archives(
    archive_ids: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<crate::maintenance::UnmanageSummary, String> {
    let download_directory = app
        .path()
        .download_dir()
        .map_err(|error| format!("ダウンロードフォルダを取得できませんでした: {error}"))?;
    with_exclusive_repository(&state, move |root| {
        crate::maintenance::unmanage_archives(&root, &download_directory, &archive_ids)
    })
    .await
}

async fn with_exclusive_repository<T: Send + 'static>(
    state: &State<'_, AppState>,
    work: impl FnOnce(PathBuf) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let repository_path = current_repository(state)?;
    // インポート・バックアップと同時にサムネイルを差し替えないよう直列化する。
    let _guard = state.import_lock.lock().await;
    tokio::task::spawn_blocking(move || work(repository_path))
        .await
        .map_err(|error| format!("処理に失敗しました: {error}"))?
}

#[tauri::command]
pub async fn regenerate_thumbnail(
    archive_id: String,
    entry_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    with_exclusive_repository(&state, move |root| {
        crate::maintenance::regenerate_thumbnail(&root, &archive_id, entry_name.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn regenerate_missing_thumbnails(
    state: State<'_, AppState>,
) -> Result<crate::maintenance::RegenerateSummary, String> {
    with_exclusive_repository(&state, |root| {
        crate::maintenance::regenerate_missing_thumbnails(&root)
    })
    .await
}

// ビューアーは同梱バイナリ（externalBin）として本体の実行ファイルと同じ場所に置かれる。
#[tauri::command]
pub fn open_in_viewer(archive_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let root = current_repository(&state)?;
    let archive = crate::maintenance::archive_path(&root, &archive_id)?;
    let viewer = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("brave-comic-viewer")))
        .filter(|path| path.is_file())
        .ok_or_else(|| "ビューアー（brave-comic-viewer）が見つかりません。".to_string())?;
    let mut child = std::process::Command::new(&viewer)
        .arg(&archive)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("ビューアーを起動できませんでした: {error}"))?;
    // 終了を待つだけのスレッドでゾンビプロセスを残さない。
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    crate::library::record_read(&root, &archive_id).map_err(|error| {
        format!("ビューアーは起動しましたが読書履歴を更新できませんでした: {error}")
    })
}

#[tauri::command]
pub async fn list_cover_candidates(
    archive_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    with_exclusive_repository(&state, move |root| {
        crate::maintenance::list_cover_candidates(&root, &archive_id)
    })
    .await
}

#[tauri::command]
pub async fn convert_archive(
    archive_id: String,
    avif_speed: u8,
    on_progress: tauri::ipc::Channel<crate::conversion::ConvertProgress>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<crate::conversion::ConvertOutcome, String> {
    let cancel = state.convert_cancel.clone();
    cancel.store(false, std::sync::atomic::Ordering::SeqCst);
    with_exclusive_repository(&state, move |root| {
        let mut encode =
            |input: &std::path::Path, output: &std::path::Path, speed: u8| -> Result<(), String> {
                crate::conversion::run_avifenc(&app, input, output, speed)
            };
        crate::conversion::convert_archive(
            &root,
            &archive_id,
            avif_speed,
            &cancel,
            &mut encode,
            &mut |progress| {
                let _ = on_progress.send(progress);
            },
        )
    })
    .await
}

#[tauri::command]
pub fn cancel_conversion(state: State<'_, AppState>) {
    state
        .convert_cancel
        .store(true, std::sync::atomic::Ordering::SeqCst);
}
