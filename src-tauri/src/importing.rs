use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::{
    fs::File as StdFile,
    io::{Read, Seek, SeekFrom},
    os::fd::AsRawFd,
    os::unix::fs::MetadataExt,
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};
use tokio::fs::{self, File, OpenOptions};

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ImportOutcome {
    Imported {
        archive_id: String,
        relative_path: String,
        file_size: u64,
        source_removed: bool,
        warning: Option<String>,
    },
    Duplicate {
        archive_id: String,
        relative_path: String,
    },
}

pub async fn import_archive(
    repository_root: &Path,
    source_path: &Path,
) -> Result<ImportOutcome, String> {
    let path_metadata = fs::symlink_metadata(source_path)
        .await
        .map_err(|error| format!("選択したファイルを確認できませんでした: {error}"))?;
    if path_metadata.file_type().is_symlink() {
        return Err("シンボリックリンクではなく、アーカイブ本体を選択してください。".to_string());
    }
    if !path_metadata.is_file() {
        return Err("インポート対象には通常ファイルを選択してください。".to_string());
    }

    let source_path = source_path
        .canonicalize()
        .map_err(|error| format!("選択したファイルの場所を確定できませんでした: {error}"))?;
    let filename = source_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            "ファイル名をUTF-8として扱えません。ファイル名を変更してから再試行してください。"
                .to_string()
        })?
        .to_string();
    let extension = source_path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "対応する拡張子のアーカイブを選択してください。".to_string())?;
    if !["7z", "zip", "rar", "cbz", "cbr", "pdf"].contains(&extension.as_str()) {
        return Err(format!(
            "対応していないファイル形式です: .{extension}（7z、zip、rar、cbz、cbr、pdfに対応）"
        ));
    }

    let lock_path = source_path.clone();
    let source_lock = tokio::task::spawn_blocking(move || open_shared_locked_file(&lock_path))
        .await
        .map_err(|error| format!("元ファイルのロック処理に失敗しました: {error}"))??;
    let source_metadata = source_lock
        .metadata()
        .map_err(|error| format!("元ファイルの情報を取得できませんでした: {error}"))?;
    if !source_metadata.is_file() {
        return Err("インポート対象には通常ファイルを選択してください。".to_string());
    }

    let mut header_file = source_lock
        .try_clone()
        .map_err(|error| format!("アーカイブのヘッダーを読み込めませんでした: {error}"))?;
    let mut header = [0_u8; 8];
    header_file
        .read_exact(&mut header)
        .map_err(|error| format!("アーカイブのヘッダーを読み込めませんでした: {error}"))?;
    validate_signature(&extension, &header)?;

    let hash_file = source_lock
        .try_clone()
        .map_err(|error| format!("アーカイブのハッシュ計算を開始できませんでした: {error}"))?;
    let archive_id = tokio::task::spawn_blocking(move || sha256sum_file(hash_file))
        .await
        .map_err(|error| format!("SHA-256計算処理に失敗しました: {error}"))??;
    let file_size = source_metadata.len();

    let database_path = repository_root.join("database/library.sqlite3");
    let existing_archive = find_archive(&database_path, &archive_id)?;
    if let Some(relative_path) = existing_archive {
        return Ok(ImportOutcome::Duplicate {
            archive_id,
            relative_path,
        });
    }

    let count_path = source_path.clone();
    let count_format = extension.clone();
    let page_count = match tokio::task::spawn_blocking(move || {
        crate::thumbnails::count_pages(&count_path, &count_format)
    })
    .await
    {
        Ok(Ok(count)) => count,
        Ok(Err(error)) => {
            return Err(format!(
                "ページ数を取得できなかったため、インポートを中止しました: {error}"
            ));
        }
        Err(error) => {
            return Err(format!(
                "ページ数の取得処理に失敗したため、インポートを中止しました: {error}"
            ));
        }
    };

    let thumbnail_relative_path = format!("thumbnails/{}/{}.webp", &archive_id[..2], archive_id);
    let thumbnail_destination = repository_root.join(&thumbnail_relative_path);
    let thumbnail_staging_directory = repository_root.join("thumbnails/.staging");
    if let Err(error) = fs::create_dir_all(&thumbnail_staging_directory).await {
        return Err(format!(
            "サムネイル用一時ディレクトリを作成できませんでした: {error}"
        ));
    }
    let thumbnail_staging_path =
        thumbnail_staging_directory.join(format!("{}.webp.part", next_staging_name()));
    let thumbnail_archive_path = source_path.clone();
    let thumbnail_output_path = thumbnail_staging_path.clone();
    let thumbnail_format = extension.clone();
    let generated_thumbnail = match tokio::task::spawn_blocking(move || {
        crate::thumbnails::create_thumbnail(
            &thumbnail_archive_path,
            &thumbnail_format,
            &thumbnail_output_path,
        )
    })
    .await
    {
        Ok(result) => result,
        Err(error) => {
            remove_if_exists(&thumbnail_staging_path).await;
            return Err(format!("サムネイル生成処理に失敗しました: {error}"));
        }
    };
    if let Err(error) = generated_thumbnail {
        remove_if_exists(&thumbnail_staging_path).await;
        return Err(format!(
            "表紙サムネイルを作成できなかったため、インポートを中止しました: {error}"
        ));
    }

    if !metadata_matches(
        &source_metadata,
        &fs::symlink_metadata(&source_path).await.ok(),
    ) {
        remove_if_exists(&thumbnail_staging_path).await;
        return Err(
            "処理中に元ファイルの場所または内容が変わったため、インポートを中止しました。"
                .to_string(),
        );
    }

    let relative_path = format!("archives/{}/{}", &archive_id[..2], filename);
    let destination = repository_root.join(&relative_path);
    let destination_directory = destination
        .parent()
        .ok_or_else(|| "アーカイブの保存先を作成できませんでした。".to_string())?;
    if let Err(error) = fs::create_dir_all(destination_directory).await {
        remove_if_exists(&thumbnail_staging_path).await;
        return Err(format!("アーカイブの保存先を作成できませんでした: {error}"));
    }
    let archive_staging_path = repository_root
        .join("archives/.staging")
        .join(next_staging_name());
    let destination_created = match install_archive(
        &source_path,
        &destination,
        &archive_staging_path,
        &archive_id,
    )
    .await
    {
        Ok(created) => created,
        Err(error) => {
            remove_if_exists(&thumbnail_staging_path).await;
            remove_empty_directory(destination_directory).await;
            return Err(error);
        }
    };
    let source_is_destination = source_path == destination
        || fs::canonicalize(&destination)
            .await
            .is_ok_and(|path| path == source_path);

    let thumbnail_directory = thumbnail_destination
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repository_root.join("thumbnails"));
    if let Err(error) = fs::create_dir_all(&thumbnail_directory).await {
        remove_if_exists(&thumbnail_staging_path).await;
        let rollback = rollback_archive(&destination, destination_created).await;
        return Err(format!(
            "サムネイルの保存先を作成できませんでした: {error}{}",
            rollback
                .map(|message| format!(" {message}"))
                .unwrap_or_default()
        ));
    }
    let thumbnail_created =
        match place_thumbnail(&thumbnail_staging_path, &thumbnail_destination).await {
            Ok(created) => created,
            Err(error) => {
                remove_if_exists(&thumbnail_staging_path).await;
                let rollback = rollback_archive(&destination, destination_created).await;
                remove_empty_directory(&thumbnail_directory).await;
                return Err(format!(
                    "サムネイルを保存できませんでした: {error}{}",
                    rollback
                        .map(|message| format!(" {message}"))
                        .unwrap_or_default()
                ));
            }
        };

    if let Err(error) = register_archive(
        &database_path,
        &archive_id,
        &filename,
        &relative_path,
        &thumbnail_relative_path,
        &extension,
        file_size,
        page_count,
    ) {
        let archive_rollback_error = rollback_archive(&destination, destination_created).await;
        let thumbnail_rollback_error =
            rollback_archive(&thumbnail_destination, thumbnail_created).await;
        let rollback_message = [archive_rollback_error, thumbnail_rollback_error]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        if !rollback_message.is_empty() {
            return Err(format!("{error} {rollback_message}"));
        }
        return Err(error);
    }

    if source_is_destination {
        return Ok(ImportOutcome::Imported {
            archive_id,
            relative_path,
            file_size,
            source_removed: false,
            warning: Some("元ファイルは既にリポジトリの保存先にあります。".to_string()),
        });
    }

    let current_metadata = fs::symlink_metadata(&source_path).await.ok();
    if !metadata_matches(&source_metadata, &current_metadata) {
        return Ok(ImportOutcome::Imported {
            archive_id,
            relative_path,
            file_size,
            source_removed: false,
            warning: Some(
                "アーカイブは登録されましたが、元ファイルの場所または内容が変わったため、元ファイルは削除していません。"
                    .to_string(),
            ),
        });
    }

    match fs::remove_file(&source_path).await {
        Ok(()) => Ok(ImportOutcome::Imported {
            archive_id,
            relative_path,
            file_size,
            source_removed: true,
            warning: None,
        }),
        Err(error) => Ok(ImportOutcome::Imported {
            archive_id,
            relative_path,
            file_size,
            source_removed: false,
            warning: Some(format!(
                "アーカイブは登録されましたが、元ファイルを削除できませんでした。元のファイルが残っています: {error}"
            )),
        }),
    }
}

fn metadata_matches(expected: &std::fs::Metadata, actual: &Option<std::fs::Metadata>) -> bool {
    actual.as_ref().is_some_and(|actual| {
        expected.dev() == actual.dev()
            && expected.ino() == actual.ino()
            && expected.len() == actual.len()
            && expected.modified().ok() == actual.modified().ok()
    })
}

fn open_shared_locked_file(path: &Path) -> Result<StdFile, String> {
    let file = StdFile::open(path)
        .map_err(|error| format!("元ファイルを読み取り専用で開けませんでした: {error}"))?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) };
    if result != 0 {
        return Err(format!(
            "元ファイルの共有ロックを取得できませんでした: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(file)
}

fn sha256sum_file(mut file: StdFile) -> Result<String, String> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("SHA-256計算の準備に失敗しました: {error}"))?;
    let output = Command::new("sha256sum")
        .stdin(Stdio::from(file))
        .output()
        .map_err(|error| format!("sha256sumを実行できませんでした: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "sha256sumに失敗しました: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let digest = output
        .stdout
        .get(..64)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .filter(|digest| digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| "sha256sumの出力を解釈できませんでした。".to_string())?;
    Ok(digest.to_ascii_lowercase())
}

async fn hash_file(path: &Path) -> Result<String, String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = StdFile::open(&path)
            .map_err(|error| format!("保存済みアーカイブを開けませんでした: {error}"))?;
        sha256sum_file(file)
            .map_err(|error| format!("保存済みアーカイブを検証できませんでした: {error}"))
    })
    .await
    .map_err(|error| format!("保存済みアーカイブの検証処理に失敗しました: {error}"))?
}

async fn install_archive(
    source: &Path,
    destination: &Path,
    staging: &Path,
    archive_id: &str,
) -> Result<bool, String> {
    match fs::hard_link(source, destination).await {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            validate_existing_archive(destination, archive_id).await
        }
        Err(link_error) => {
            let staging_directory = staging
                .parent()
                .ok_or_else(|| "インポート用一時ディレクトリが不正です。".to_string())?;
            fs::create_dir_all(staging_directory)
                .await
                .map_err(|error| {
                    format!("インポート用一時ディレクトリを作成できませんでした: {error}")
                })?;
            if let Err(copy_error) = copy_with_cp(source, staging).await {
                remove_if_exists(staging).await;
                return Err(format!(
                    "元ファイルを保存先へ直接配置できませんでした ({link_error})。代替コピーにも失敗しました: {copy_error}"
                ));
            }
            match fs::hard_link(staging, destination).await {
                Ok(()) => {
                    if let Err(error) = fs::remove_file(staging).await {
                        let rollback = fs::remove_file(destination).await;
                        let rollback_message = rollback
                            .err()
                            .map(|rollback_error| {
                                format!(" 保存先の取り消しにも失敗しました: {rollback_error}")
                            })
                            .unwrap_or_default();
                        return Err(format!(
                            "アーカイブの一時ファイルを整理できませんでした: {error}{rollback_message}"
                        ));
                    }
                    Ok(true)
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    remove_if_exists(staging).await;
                    validate_existing_archive(destination, archive_id).await
                }
                Err(error) => {
                    remove_if_exists(staging).await;
                    Err(format!("アーカイブを保存先へ配置できませんでした: {error}"))
                }
            }
        }
    }
}

async fn validate_existing_archive(destination: &Path, archive_id: &str) -> Result<bool, String> {
    match hash_file(destination).await {
        Ok(existing_hash) if existing_hash == archive_id => Ok(false),
        Ok(_) => Err("同じ分散ディレクトリに同名で内容の異なるアーカイブが既にあります。ファイル名はリネームしないため、元ファイルの名前を変更してからインポートしてください。".to_string()),
        Err(error) => Err(format!(
            "保存先にファイルがありますが内容を検証できませんでした: {error}"
        )),
    }
}

async fn copy_with_cp(source: &Path, destination: &Path) -> Result<(), String> {
    let source = source.to_path_buf();
    let destination = destination.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let output = Command::new("cp")
            .arg("--reflink=auto")
            .arg("--no-preserve=mode")
            .arg("--")
            .arg(&source)
            .arg(&destination)
            .output()
            .map_err(|error| format!("cpを実行できませんでした: {error}"))?;
        if output.status.success() {
            let copied_file = StdFile::options()
                .write(true)
                .open(&destination)
                .map_err(|error| format!("コピーしたアーカイブを開けませんでした: {error}"))?;
            copied_file.sync_all().map_err(|error| {
                format!("コピーしたアーカイブをディスクへ保存できませんでした: {error}")
            })
        } else {
            Err(format!(
                "cpに失敗しました: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    })
    .await
    .map_err(|error| format!("アーカイブのコピー処理に失敗しました: {error}"))?
}

async fn place_thumbnail(staging: &Path, destination: &Path) -> Result<bool, String> {
    match fs::hard_link(staging, destination).await {
        Ok(()) => {
            if let Err(error) = fs::remove_file(staging).await {
                let rollback = fs::remove_file(destination).await;
                let rollback_message = rollback
                    .err()
                    .map(|rollback_error| {
                        format!(" サムネイル配置の取り消しにも失敗しました: {rollback_error}")
                    })
                    .unwrap_or_default();
                return Err(format!(
                    "サムネイル一時ファイルを整理できませんでした: {error}{rollback_message}"
                ));
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            remove_if_exists(staging).await;
            let existing_path = destination.to_path_buf();
            tokio::task::spawn_blocking(move || {
                crate::thumbnails::validate_existing_thumbnail(&existing_path)
            })
            .await
            .map_err(|error| format!("既存サムネイルの検証に失敗しました: {error}"))??;
            Ok(false)
        }
        Err(_link_error) if destination.exists() => {
            remove_if_exists(staging).await;
            let existing_path = destination.to_path_buf();
            tokio::task::spawn_blocking(move || {
                crate::thumbnails::validate_existing_thumbnail(&existing_path)
            })
            .await
            .map_err(|error| format!("既存サムネイルの検証に失敗しました: {error}"))??;
            Ok(false)
        }
        Err(link_error) => {
            if let Err(copy_error) = copy_staged_file(staging, destination).await {
                return Err(format!(
                    "サムネイルの配置に失敗しました ({link_error}): {copy_error}"
                ));
            }
            if let Err(error) = fs::remove_file(staging).await {
                let rollback = fs::remove_file(destination).await;
                let rollback_message = rollback
                    .err()
                    .map(|rollback_error| {
                        format!(" サムネイル配置の取り消しにも失敗しました: {rollback_error}")
                    })
                    .unwrap_or_default();
                return Err(format!(
                    "サムネイル一時ファイルを整理できませんでした: {error}{rollback_message}"
                ));
            }
            Ok(true)
        }
    }
}

async fn rollback_archive(path: &Path, created: bool) -> Option<String> {
    if !created {
        return None;
    }
    let result = match fs::remove_file(path).await {
        Ok(()) => None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => Some(format!(
            "作成したファイルを取り消せませんでした ({}): {error}",
            path.display()
        )),
    };
    if let Some(parent) = path.parent() {
        remove_empty_directory(parent).await;
    }
    result
}

// 空でない場合は削除に失敗するため、他のファイルは巻き込まない。
async fn remove_empty_directory(path: &Path) {
    let _ = fs::remove_dir(path).await;
}

async fn copy_staged_file(source: &Path, destination: &Path) -> Result<(), String> {
    let mut staged_file = File::open(source)
        .await
        .map_err(|error| format!("一時アーカイブを開けませんでした: {error}"))?;
    let mut destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .await
        .map_err(|error| format!("保存先ファイルを安全に作成できませんでした: {error}"))?;

    let result = async {
        tokio::io::copy(&mut staged_file, &mut destination_file)
            .await
            .map_err(|error| format!("アーカイブを保存先へコピーできませんでした: {error}"))?;
        destination_file
            .sync_all()
            .await
            .map_err(|error| format!("アーカイブをディスクへ保存できませんでした: {error}"))
    }
    .await;

    drop(destination_file);
    if result.is_err() {
        remove_if_exists(destination).await;
    }
    result
}

fn validate_signature(extension: &str, header: &[u8]) -> Result<(), String> {
    let is_valid = match extension {
        "pdf" => header.starts_with(b"%PDF-"),
        "7z" => header.starts_with(&[0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c]),
        "zip" | "cbz" => {
            header.starts_with(b"PK\x03\x04")
                || header.starts_with(b"PK\x05\x06")
                || header.starts_with(b"PK\x07\x08")
        }
        "rar" | "cbr" => {
            header.starts_with(b"Rar!\x1a\x07\x00") || header.starts_with(b"Rar!\x1a\x07\x01\x00")
        }
        _ => false,
    };

    if is_valid {
        Ok(())
    } else {
        Err(format!(
            "拡張子 .{extension} とアーカイブのファイルヘッダーが一致しません。ファイル形式と破損の有無を確認してください。"
        ))
    }
}

fn find_archive(database_path: &Path, archive_id: &str) -> Result<Option<String>, String> {
    let connection = Connection::open(database_path)
        .map_err(|error| format!("ライブラリDBを開けませんでした: {error}"))?;
    connection
        .query_row(
            "SELECT relative_path FROM archives WHERE archive_id = ?1 OR converted_from = ?1",
            [archive_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("重複チェックに失敗しました: {error}"))
}

fn register_archive(
    database_path: &Path,
    archive_id: &str,
    filename: &str,
    relative_path: &str,
    thumbnail_relative_path: &str,
    format: &str,
    file_size: u64,
    page_count: u64,
) -> Result<(), String> {
    let file_size = i64::try_from(file_size)
        .map_err(|_| "アーカイブのサイズがDBで扱える範囲を超えています。".to_string())?;
    let page_count = i64::try_from(page_count)
        .map_err(|_| "ページ数がDBで扱える範囲を超えています。".to_string())?;
    let mut connection = Connection::open(database_path)
        .map_err(|error| format!("ライブラリDBを開けませんでした: {error}"))?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("DB登録を開始できませんでした: {error}"))?;
    transaction
        .execute(
            "INSERT INTO archives (
                archive_id, original_filename, original_path, relative_path,
                format, file_size, thumbnail_relative_path, imported_at, page_count
            ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?7)",
            params![
                archive_id,
                filename,
                relative_path,
                format,
                file_size,
                thumbnail_relative_path,
                page_count
            ],
        )
        .map_err(|error| format!("アーカイブ情報をDBへ登録できませんでした: {error}"))?;
    let (classification, author) = parse_filename_tags(filename);
    for (category, name) in [("classification", classification), ("author", author)]
        .into_iter()
        .filter_map(|(category, name)| name.map(|name| (category, name)))
    {
        transaction
            .execute(
                "INSERT OR IGNORE INTO tags (name, category) VALUES (?1, ?2)",
                params![name, category],
            )
            .map_err(|error| format!("タグを作成できませんでした: {error}"))?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO archive_tags (archive_id, tag_id)
                 SELECT ?1, id FROM tags WHERE name = ?2 AND category = ?3",
                params![archive_id, name, category],
            )
            .map_err(|error| format!("タグを付けられませんでした: {error}"))?;
    }
    transaction
        .commit()
        .map_err(|error| format!("アーカイブ情報の登録を確定できませんでした: {error}"))
}

const MAX_TAG_NAME_CHARS: usize = 100;

// 先頭の (分類) と [作者名] をそれぞれ解析する。
fn parse_filename_tags(filename: &str) -> (Option<&str>, Option<&str>) {
    let valid = |name: &str| !name.is_empty() && name.chars().count() <= MAX_TAG_NAME_CHARS;
    let filename = filename.trim_start();
    let (classification, rest) = if let Some(rest) = filename.strip_prefix('(') {
        match rest.split_once(')') {
            Some((name, rest)) => {
                let name = name.trim();
                (valid(name).then_some(name), rest.trim_start())
            }
            None => (None, ""),
        }
    } else {
        (None, filename)
    };
    let author = rest
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .map(|(name, _)| name.trim())
        .filter(|name| valid(name));
    (classification, author)
}

fn next_staging_name() -> String {
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{timestamp}-{sequence}.part", std::process::id())
}

async fn remove_if_exists(path: &Path) {
    if let Err(error) = fs::remove_file(path).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!(
                "一時ファイルを削除できませんでした ({}): {error}",
                path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_filename_tags;
    use super::validate_signature;
    use super::{import_archive, ImportOutcome};

    #[test]
    fn parses_classification_and_author_independently_from_filename_prefix() {
        assert_eq!(
            parse_filename_tags("(一般コミック)[山田太郎]タイトル.zip"),
            (Some("一般コミック"), Some("山田太郎"))
        );
        assert_eq!(
            parse_filename_tags("(コミック)タイトル.zip"),
            (Some("コミック"), None)
        );
        assert_eq!(
            parse_filename_tags("[作者名]タイトル.zip"),
            (None, Some("作者名"))
        );
        assert_eq!(
            parse_filename_tags("タイトル(分類)[作者].zip"),
            (None, None)
        );
        assert_eq!(
            parse_filename_tags("(コミック) [作者名] とらのあな限定 小冊子.zip"),
            (Some("コミック"), Some("作者名"))
        );
        assert_eq!(
            parse_filename_tags("( コミック )  [ 作者名 ]タイトル.zip"),
            (Some("コミック"), Some("作者名"))
        );
        assert_eq!(parse_filename_tags("()[作者]x.zip"), (None, Some("作者")));
        assert_eq!(parse_filename_tags("[]x.zip"), (None, None));
        assert_eq!(parse_filename_tags("(分類)[ ]x.zip"), (Some("分類"), None));
    }

    #[test]
    fn validates_pdf_header() {
        assert!(validate_signature("pdf", b"%PDF-1.7").is_ok());
        assert!(validate_signature("pdf", b"not a PDF").is_err());
    }

    use crate::database;
    use image::{ImageEncoder, RgbImage};
    use sha2::{Digest, Sha256};
    use std::{
        io::{Cursor, Write},
        path::Path,
    };
    use zip::{write::SimpleFileOptions, ZipWriter};

    fn initialize_repository(root: &Path) {
        for directory in ["archives", "thumbnails", "database"] {
            std::fs::create_dir_all(root.join(directory)).expect("create repository directory");
        }
        database::initialize(&root.join("database/library.sqlite3"))
            .expect("initialize repository database");
    }

    fn cbz_bytes(color: [u8; 3]) -> Vec<u8> {
        let image = RgbImage::from_pixel(16, 24, image::Rgb(color));
        let mut png = Cursor::new(Vec::new());
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgb8,
            )
            .expect("encode test cover");

        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("cover.png", SimpleFileOptions::default())
            .expect("start cover entry");
        zip.write_all(png.get_ref()).expect("write cover entry");
        zip.finish().expect("finish test archive").into_inner()
    }

    #[tokio::test]
    async fn imports_archive_with_hash_sharding_and_original_filename() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let source_directory = tempfile::tempdir().expect("temporary source");
        initialize_repository(repository.path());
        let source_path = source_directory.path().join("My Comic.CBZ");
        std::fs::write(&source_path, cbz_bytes([120, 80, 200])).expect("write source archive");

        let outcome = import_archive(repository.path(), &source_path)
            .await
            .expect("import archive");

        let ImportOutcome::Imported {
            archive_id,
            relative_path,
            source_removed,
            ..
        } = outcome
        else {
            panic!("expected archive to be imported");
        };
        assert_eq!(
            relative_path,
            format!("archives/{}/My Comic.CBZ", &archive_id[..2])
        );
        assert!(!source_path.exists());
        assert!(source_removed);
        assert!(repository.path().join(relative_path).exists());
    }

    #[tokio::test]
    async fn imports_pdf_with_page_count_and_first_page_thumbnail() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let source_directory = tempfile::tempdir().expect("temporary source");
        initialize_repository(repository.path());
        let source_path = source_directory.path().join("Comic.pdf");
        std::fs::write(
            &source_path,
            crate::thumbnails::test_pdf_bytes(&["1 0 0", "0 0 1"]),
        )
        .expect("write source PDF");

        let ImportOutcome::Imported {
            archive_id,
            relative_path,
            source_removed,
            ..
        } = import_archive(repository.path(), &source_path)
            .await
            .expect("import PDF")
        else {
            panic!("expected PDF to be imported");
        };
        assert!(source_removed);
        assert!(!source_path.exists());
        assert!(repository.path().join(relative_path).exists());

        let connection =
            rusqlite::Connection::open(repository.path().join("database/library.sqlite3"))
                .expect("open database");
        let (format, page_count, thumbnail): (String, u64, String) = connection
            .query_row(
                "SELECT format, page_count, thumbnail_relative_path
                 FROM archives WHERE archive_id = ?1",
                [&archive_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read imported PDF");
        assert_eq!(format, "pdf");
        assert_eq!(page_count, 2);
        crate::thumbnails::validate_existing_thumbnail(&repository.path().join(thumbnail))
            .expect("validate imported PDF thumbnail");
    }

    #[tokio::test]
    async fn duplicate_hash_is_not_registered_twice_or_removed_from_source() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let source_directory = tempfile::tempdir().expect("temporary source");
        initialize_repository(repository.path());
        let first_source = source_directory.path().join("first.cbz");
        let second_source = source_directory.path().join("renamed.zip");
        let archive_bytes = cbz_bytes([120, 80, 200]);
        std::fs::write(&first_source, &archive_bytes).expect("write first archive");
        std::fs::write(&second_source, &archive_bytes).expect("write second archive");

        let first = import_archive(repository.path(), &first_source)
            .await
            .expect("import first archive");
        let duplicate = import_archive(repository.path(), &second_source)
            .await
            .expect("check duplicate archive");

        let ImportOutcome::Imported { archive_id, .. } = first else {
            panic!("expected first archive to be imported");
        };
        let ImportOutcome::Duplicate {
            archive_id: duplicate_id,
            ..
        } = duplicate
        else {
            panic!("expected duplicate archive");
        };
        assert_eq!(archive_id, duplicate_id);
        assert!(second_source.exists());
        assert!(!repository.path().join("archives/.staging").exists());
        let database_path = repository.path().join("database/library.sqlite3");
        assert_eq!(database::initialize(&database_path).expect("count"), 1);
        let connection = rusqlite::Connection::open(database_path).expect("open database");
        let thumbnail_path: String = connection
            .query_row(
                "SELECT thumbnail_relative_path FROM archives WHERE archive_id = ?1",
                [&archive_id],
                |row| row.get(0),
            )
            .expect("read thumbnail path");
        assert_eq!(
            thumbnail_path,
            format!("thumbnails/{}/{archive_id}.webp", &archive_id[..2])
        );
        crate::thumbnails::validate_existing_thumbnail(&repository.path().join(thumbnail_path))
            .expect("validate registered WebP thumbnail");
    }

    #[test]
    fn source_file_is_opened_read_only_with_a_shared_lock() {
        use std::os::fd::AsRawFd;

        let directory = tempfile::tempdir().expect("temporary source");
        let source_path = directory.path().join("archive.cbz");
        std::fs::write(&source_path, b"archive").expect("write source");
        let shared_lock = super::open_shared_locked_file(&source_path).expect("shared lock");
        let writer = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&source_path)
            .expect("open writer");

        let lock_result = unsafe { libc::flock(writer.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(lock_result, -1);
        assert_eq!(
            std::io::Error::last_os_error().kind(),
            std::io::ErrorKind::WouldBlock
        );

        drop(shared_lock);
        assert_eq!(
            unsafe { libc::flock(writer.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
    }

    #[tokio::test]
    async fn rejects_a_different_archive_with_the_same_name_in_the_same_prefix() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let source_directory = tempfile::tempdir().expect("temporary source");
        initialize_repository(repository.path());
        let archive_bytes = cbz_bytes([80, 120, 200]);
        let archive_id = format!("{:x}", Sha256::digest(&archive_bytes));
        let existing_path = repository
            .path()
            .join(format!("archives/{}/Comic.cbz", &archive_id[..2]));
        std::fs::create_dir_all(existing_path.parent().expect("prefix directory"))
            .expect("create prefix directory");
        std::fs::write(&existing_path, b"different existing archive").expect("write existing");
        let source_path = source_directory.path().join("Comic.cbz");
        std::fs::write(&source_path, &archive_bytes).expect("write source archive");

        let error = import_archive(repository.path(), &source_path)
            .await
            .expect_err("reject name collision");

        assert!(error.contains("同名"));
        assert!(source_path.exists());
        assert_eq!(
            std::fs::read(existing_path).expect("existing file is untouched"),
            b"different existing archive"
        );
        assert_eq!(
            database::initialize(&repository.path().join("database/library.sqlite3"))
                .expect("count registered archives"),
            0
        );
    }

    #[tokio::test]
    async fn failed_cover_extraction_leaves_no_empty_prefix_directories() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let source_directory = tempfile::tempdir().expect("temporary source");
        initialize_repository(repository.path());
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("readme.txt", SimpleFileOptions::default())
            .expect("start entry");
        zip.write_all(b"no images").expect("write entry");
        let source_path = source_directory.path().join("NoImage.cbz");
        std::fs::write(&source_path, zip.finish().expect("finish").into_inner())
            .expect("write archive");

        import_archive(repository.path(), &source_path)
            .await
            .expect_err("reject archive without cover");

        assert!(source_path.exists());
        for directory in ["archives", "thumbnails"] {
            let prefixes: Vec<_> = std::fs::read_dir(repository.path().join(directory))
                .expect("read directory")
                .map(|entry| entry.expect("entry").file_name())
                .filter(|name| name != ".staging")
                .collect();
            assert!(prefixes.is_empty(), "{directory}: {prefixes:?}");
        }
    }

    #[tokio::test]
    async fn rejects_unsupported_files_without_moving_them() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let source_directory = tempfile::tempdir().expect("temporary source");
        initialize_repository(repository.path());
        let source_path = source_directory.path().join("notes.txt");
        std::fs::write(&source_path, b"not an archive").expect("write unsupported file");

        let error = import_archive(repository.path(), &source_path)
            .await
            .expect_err("reject unsupported file");

        assert!(error.contains("対応していない"));
        assert!(source_path.exists());
    }

    #[tokio::test]
    async fn rejects_a_file_with_a_mismatched_archive_signature() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let source_directory = tempfile::tempdir().expect("temporary source");
        initialize_repository(repository.path());
        let source_path = source_directory.path().join("renamed.cbz");
        std::fs::write(&source_path, b"not a zip archive").expect("write invalid archive");

        let error = import_archive(repository.path(), &source_path)
            .await
            .expect_err("reject mismatched signature");

        assert!(error.contains("ヘッダーが一致しません"));
        assert!(source_path.exists());
        assert_eq!(
            database::initialize(&repository.path().join("database/library.sqlite3"))
                .expect("count registered archives"),
            0
        );
    }

    #[tokio::test]
    async fn recovers_a_matching_file_left_before_database_registration() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let source_directory = tempfile::tempdir().expect("temporary source");
        initialize_repository(repository.path());
        let archive_bytes = cbz_bytes([120, 80, 200]);
        let archive_id = format!("{:x}", Sha256::digest(&archive_bytes));
        let relative_path = format!("archives/{}/Comic.cbz", &archive_id[..2]);
        let orphan_path = repository.path().join(&relative_path);
        std::fs::create_dir_all(orphan_path.parent().expect("orphan parent"))
            .expect("create orphan directory");
        std::fs::write(&orphan_path, &archive_bytes).expect("write orphan");

        let source_path = source_directory.path().join("Comic.cbz");
        std::fs::write(&source_path, &archive_bytes).expect("write import source");
        let outcome = import_archive(repository.path(), &source_path)
            .await
            .expect("recover orphan and register archive");

        assert!(matches!(outcome, ImportOutcome::Imported { .. }));
        assert!(!source_path.exists());
        assert_eq!(
            database::initialize(&repository.path().join("database/library.sqlite3"))
                .expect("count registered archives"),
            1
        );
        assert_eq!(
            std::fs::read(orphan_path).expect("read recovered archive"),
            archive_bytes
        );
    }
}
