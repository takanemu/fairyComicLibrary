use crate::database::{self, CURRENT_SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const REPOSITORY_MANIFEST: &str = "repository.json";
const REPOSITORY_FORMAT_VERSION: u32 = 1;
const SAVED_PATH_FILE: &str = "repository-location";

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    format_version: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryStatus {
    pub path: String,
    pub schema_version: u32,
    pub archive_count: u64,
    pub warnings: Vec<String>,
}

pub fn open(path: &Path) -> Result<RepositoryStatus, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("リポジトリのディレクトリを確認できませんでした: {error}"))?;
    if !metadata.is_dir() {
        return Err("リポジトリにはディレクトリを選択してください。".to_string());
    }
    let root = path
        .canonicalize()
        .map_err(|error| format!("リポジトリの場所を確定できませんでした: {error}"))?;
    let manifest_path = root.join(REPOSITORY_MANIFEST);

    if manifest_path.exists() {
        let manifest: Manifest = serde_json::from_slice(
            &std::fs::read(&manifest_path)
                .map_err(|error| format!("リポジトリ情報を読み取れませんでした: {error}"))?,
        )
        .map_err(|error| format!("リポジトリ情報の形式が不正です: {error}"))?;
        if manifest.format_version != REPOSITORY_FORMAT_VERSION {
            return Err(format!(
                "リポジトリ形式のバージョン {} には対応していません（対応バージョン: {}）。",
                manifest.format_version, REPOSITORY_FORMAT_VERSION
            ));
        }
    } else {
        let mut entries = std::fs::read_dir(&root)
            .map_err(|error| format!("リポジトリの内容を確認できませんでした: {error}"))?;
        if entries
            .next()
            .transpose()
            .map_err(|error| format!("リポジトリの内容を確認できませんでした: {error}"))?
            .is_some()
        {
            return Err(
                "選択したディレクトリは空ではなく、Fairy Comic Libraryのリポジトリ情報もありません。別のディレクトリを選択してください。"
                    .to_string(),
            );
        }

        write_manifest(&manifest_path)?;
    }

    for directory in ["archives", "thumbnails", "database"] {
        std::fs::create_dir_all(root.join(directory)).map_err(|error| {
            format!("リポジトリ内に {directory}/ を作成できませんでした: {error}")
        })?;
    }

    let database_path = root.join("database/library.sqlite3");
    let archive_count = database::initialize(&database_path)?;
    remove_stale_conversions(&root);
    if let Err(error) = crate::thumbnails::migrate_avif_thumbnails(&root) {
        eprintln!("旧サムネイルの移行を完了できませんでした: {error}");
    }

    let warnings = crate::maintenance::verify(&root)?.warnings;

    Ok(RepositoryStatus {
        path: root.to_string_lossy().into_owned(),
        schema_version: CURRENT_SCHEMA_VERSION,
        archive_count,
        warnings,
    })
}

// 変換中の異常終了で残った一時ファイルを片付ける。
fn remove_stale_conversions(root: &Path) {
    for directory in ["archives/.staging", "thumbnails/.staging"] {
        let Ok(entries) = std::fs::read_dir(root.join(directory)) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .ends_with(".convert.part")
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

pub fn load_saved_repository(app: &AppHandle) -> Result<Option<RepositoryStatus>, String> {
    let Some(path) = read_saved_path(app)? else {
        return Ok(None);
    };
    open(&path).map(Some)
}

pub fn save_selected_repository(app: &AppHandle, path: &Path) -> Result<(), String> {
    let config_directory = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("アプリ設定ディレクトリを取得できませんでした: {error}"))?;
    std::fs::create_dir_all(&config_directory)
        .map_err(|error| format!("アプリ設定ディレクトリを作成できませんでした: {error}"))?;

    let destination = config_directory.join(SAVED_PATH_FILE);
    let temporary = config_directory.join(format!("{SAVED_PATH_FILE}.tmp"));
    std::fs::write(&temporary, path.as_os_str().as_encoded_bytes())
        .map_err(|error| format!("リポジトリの設定を保存できませんでした: {error}"))?;
    std::fs::rename(&temporary, &destination)
        .map_err(|error| format!("リポジトリの設定を確定できませんでした: {error}"))
}

fn read_saved_path(app: &AppHandle) -> Result<Option<PathBuf>, String> {
    let config_directory = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("アプリ設定ディレクトリを取得できませんでした: {error}"))?;
    let saved_path = config_directory.join(SAVED_PATH_FILE);
    match std::fs::read(&saved_path) {
        Ok(bytes) => {
            let path = String::from_utf8(bytes)
                .map_err(|error| format!("保存済みリポジトリのパスが不正です: {error}"))?;
            Ok(Some(PathBuf::from(path)))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "保存済みリポジトリの設定を読み取れませんでした: {error}"
        )),
    }
}

fn write_manifest(path: &Path) -> Result<(), String> {
    let manifest = Manifest {
        format_version: REPOSITORY_FORMAT_VERSION,
    };
    let serialized = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("リポジトリ情報を作成できませんでした: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serialized)
        .map_err(|error| format!("リポジトリ情報を保存できませんでした: {error}"))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("リポジトリ情報を確定できませんでした: {error}"))
}

#[cfg(test)]
mod tests {
    use super::open;

    #[test]
    fn initializes_and_reopens_repository() {
        let directory = tempfile::tempdir().expect("temporary directory");

        let first = open(directory.path()).expect("initialize repository");
        let second = open(directory.path()).expect("reopen repository");

        assert_eq!(first.path, second.path);
        assert_eq!(first.schema_version, 2);
        assert_eq!(first.archive_count, 0);
        for relative_path in [
            "repository.json",
            "archives",
            "thumbnails",
            "database/library.sqlite3",
        ] {
            assert!(directory.path().join(relative_path).exists());
        }
    }

    #[test]
    fn refuses_to_initialize_non_empty_unmanaged_directory() {
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::write(directory.path().join("keep.txt"), "existing data")
            .expect("create existing file");

        let error = open(directory.path()).expect_err("refuse unmanaged directory");

        assert!(error.contains("空ではなく"));
        assert!(directory.path().join("keep.txt").exists());
        assert!(!directory.path().join("repository.json").exists());
    }

    #[test]
    fn does_not_recreate_a_missing_saved_repository() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let missing_path = directory.path().join("removed-repository");

        assert!(open(&missing_path).is_err());
        assert!(!missing_path.exists());
    }
}
