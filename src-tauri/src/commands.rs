use crate::{library, repository, services::LibraryService, AppState};
use std::collections::BTreeMap;
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
        Some(path) => LibraryService::new(path.clone())
            .repository_status()
            .map(Some),
        None => Ok(None),
    }
}

#[tauri::command]
pub fn open_repository(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<repository::RepositoryStatus, String> {
    let (_, status) = LibraryService::open(&PathBuf::from(path))?;
    repository::save_selected_repository(&app, PathBuf::from(&status.path).as_path())?;
    *state.repository_path.lock().map_err(|_| {
        "リポジトリ状態を更新できませんでした。アプリを再起動してください。".to_string()
    })? = Some(PathBuf::from(&status.path));
    Ok(status)
}

#[tauri::command]
pub async fn import_archive(
    path: String,
    thumbnail_path: Option<String>,
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
    let thumbnail_path = thumbnail_path.map(PathBuf::from);
    LibraryService::new(repository_path)
        .import_archive(&PathBuf::from(path), thumbnail_path.as_deref())
        .await
}

fn current_service(state: &State<'_, AppState>) -> Result<LibraryService, String> {
    let root = state
        .repository_path
        .lock()
        .map_err(|_| {
            "リポジトリ状態を読み取れませんでした。アプリを再起動してください。".to_string()
        })?
        .clone()
        .ok_or_else(|| "先にアプリ管理リポジトリを選択してください。".to_string())?;
    Ok(LibraryService::new(root))
}

#[tauri::command(async)]
pub fn get_video_duration(archive_id: String, state: State<'_, AppState>) -> Result<u64, String> {
    current_service(&state)?.get_video_duration(&archive_id)
}

#[tauri::command(async)]
pub fn list_archives(
    query: library::ArchiveQuery,
    state: State<'_, AppState>,
) -> Result<Vec<library::ArchiveSummary>, String> {
    current_service(&state)?.list_archives(&query)
}

#[tauri::command(async)]
pub fn list_tags(state: State<'_, AppState>) -> Result<Vec<library::TagWithCount>, String> {
    current_service(&state)?.list_tags()
}

#[tauri::command(async)]
pub fn list_tag_groups(state: State<'_, AppState>) -> Result<Vec<library::TagGroup>, String> {
    current_service(&state)?.list_tag_groups()
}

#[tauri::command(async)]
pub fn create_tag_group(
    category: String,
    name: String,
    parent_id: Option<i64>,
    state: State<'_, AppState>,
) -> Result<library::TagGroup, String> {
    current_service(&state)?.create_tag_group(&category, &name, parent_id)
}

#[tauri::command(async)]
pub fn rename_tag_group(
    group_id: i64,
    name: String,
    expected_revision: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    current_service(&state)?.rename_tag_group(group_id, &name, expected_revision)
}

#[tauri::command(async)]
pub fn delete_tag_group(
    group_id: i64,
    expected_revision: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    current_service(&state)?.delete_tag_group(group_id, expected_revision)
}

#[tauri::command(async)]
pub fn move_tag(
    tag_id: i64,
    parent_id: Option<i64>,
    expected_revision: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    current_service(&state)?.move_tag(tag_id, parent_id, expected_revision)
}

#[tauri::command(async)]
pub fn move_tag_group(
    group_id: i64,
    parent_id: Option<i64>,
    expected_revision: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    current_service(&state)?.move_tag_group(group_id, parent_id, expected_revision)
}

#[tauri::command(async)]
pub fn set_favorite(
    archive_id: String,
    is_favorite: bool,
    expected_revision: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    current_service(&state)?.set_favorite(&archive_id, is_favorite, expected_revision)
}

#[tauri::command(async)]
pub fn add_tag(
    archive_id: String,
    category: String,
    name: String,
    expected_revision: i64,
    state: State<'_, AppState>,
) -> Result<library::TagSummary, String> {
    current_service(&state)?.add_tag(&archive_id, &category, &name, expected_revision)
}

#[tauri::command(async)]
pub fn add_tag_to_archives(
    archive_ids: Vec<String>,
    category: String,
    name: String,
    expected_revisions: BTreeMap<String, i64>,
    state: State<'_, AppState>,
) -> Result<library::TagSummary, String> {
    current_service(&state)?.add_tag_to_archives(
        &archive_ids,
        &category,
        &name,
        &expected_revisions,
    )
}

#[tauri::command(async)]
pub fn remove_tag(
    archive_id: String,
    tag_id: i64,
    expected_revision: i64,
    expected_tag_revision: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    current_service(&state)?.remove_tag(
        &archive_id,
        tag_id,
        expected_revision,
        expected_tag_revision,
    )
}

#[tauri::command(async)]
pub fn remove_tag_from_archives(
    archive_ids: Vec<String>,
    tag_id: i64,
    expected_revisions: BTreeMap<String, i64>,
    expected_tag_revision: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    current_service(&state)?.remove_tag_from_archives(
        &archive_ids,
        tag_id,
        &expected_revisions,
        expected_tag_revision,
    )
}

#[tauri::command]
pub async fn backup_repository(
    destination: String,
    state: State<'_, AppState>,
) -> Result<crate::maintenance::BackupSummary, String> {
    let service = current_service(&state)?;
    // インポートと同時に走らせると、ファイルとDBの対応が崩れた状態でコピーしかねない。
    let _import_guard = state.import_lock.lock().await;
    tokio::task::spawn_blocking(move || service.backup_repository(&PathBuf::from(destination)))
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
    with_exclusive_repository(&state, move |service| {
        service.unmanage_archives(&archive_ids, &download_directory)
    })
    .await
}

async fn with_exclusive_repository<T: Send + 'static>(
    state: &State<'_, AppState>,
    work: impl FnOnce(LibraryService) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let service = current_service(state)?;
    // インポート・バックアップと同時にサムネイルを差し替えないよう直列化する。
    let _guard = state.import_lock.lock().await;
    tokio::task::spawn_blocking(move || work(service))
        .await
        .map_err(|error| format!("処理に失敗しました: {error}"))?
}

#[tauri::command]
pub async fn regenerate_thumbnail(
    archive_id: String,
    entry_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    with_exclusive_repository(&state, move |service| {
        service.regenerate_thumbnail(&archive_id, entry_name.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn set_package_image(
    archive_id: String,
    image_path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let image_path = PathBuf::from(image_path);
    with_exclusive_repository(&state, move |service| {
        service.set_package_image(&archive_id, &image_path)
    })
    .await
}

#[tauri::command]
pub async fn regenerate_missing_thumbnails(
    state: State<'_, AppState>,
) -> Result<crate::maintenance::RegenerateSummary, String> {
    with_exclusive_repository(&state, |service| service.regenerate_missing_thumbnails()).await
}

// ビューアーは同梱バイナリ（externalBin）として本体の実行ファイルと同じ場所に置かれる。
#[tauri::command]
pub fn open_in_viewer(archive_id: String, state: State<'_, AppState>) -> Result<(), String> {
    current_service(&state)?.open_in_viewer(&archive_id)
}

#[tauri::command]
pub async fn list_cover_candidates(
    archive_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    with_exclusive_repository(&state, move |service| {
        service.list_cover_candidates(&archive_id)
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
    with_exclusive_repository(&state, move |service| {
        let mut encode =
            |input: &std::path::Path, output: &std::path::Path, speed: u8| -> Result<(), String> {
                crate::conversion::run_avifenc(&app, input, output, speed)
            };
        service.convert_archive(
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
