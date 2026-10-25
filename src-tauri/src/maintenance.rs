use rusqlite::Connection;
use serde::Serialize;
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

const MAX_REPORTED_FILES: usize = 5;
const MANIFEST: &str = "repository.json";
const DATABASE: &str = "database/library.sqlite3";

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VerificationReport {
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupSummary {
    pub destination: String,
    pub archive_count: u64,
    pub file_count: u64,
}

pub(crate) fn is_safe_relative(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn describe_missing(label: &str, missing: &[String]) -> Option<String> {
    if missing.is_empty() {
        return None;
    }
    let samples = missing
        .iter()
        .take(MAX_REPORTED_FILES)
        .cloned()
        .collect::<Vec<_>>()
        .join("、");
    let more = if missing.len() > MAX_REPORTED_FILES {
        " ほか"
    } else {
        ""
    };
    Some(format!(
        "{label}が{}件見つかりません（{samples}{more}）。コピー元のリポジトリから該当ファイルをコピーし直してください。",
        missing.len()
    ))
}

// 読み込み時の検証。問題があってもリポジトリは開き、内容と対処方法を警告として返す。
pub fn verify(root: &Path) -> Result<VerificationReport, String> {
    let connection = Connection::open(root.join(DATABASE))
        .map_err(|error| format!("SQLiteデータベースを開けませんでした: {error}"))?;
    let mut report = VerificationReport::default();

    let integrity: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|error| format!("DBの整合性を確認できませんでした: {error}"))?;
    if integrity != "ok" {
        report.warnings.push(format!(
            "DBの整合性チェックで問題が見つかりました（{integrity}）。バックアップからリポジトリを復元してください。"
        ));
    }

    let mut statement = connection
        .prepare("SELECT relative_path, thumbnail_relative_path FROM archives")
        .map_err(|error| format!("アーカイブ情報を確認できませんでした: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(|error| format!("アーカイブ情報を確認できませんでした: {error}"))?;

    let mut missing_archives = Vec::new();
    let mut missing_thumbnails = Vec::new();
    for row in rows {
        let (archive, thumbnail) =
            row.map_err(|error| format!("アーカイブ情報を読み取れませんでした: {error}"))?;
        if !is_safe_relative(&archive) || !root.join(&archive).is_file() {
            missing_archives.push(archive);
        }
        if let Some(thumbnail) = thumbnail {
            if !is_safe_relative(&thumbnail) || !root.join(&thumbnail).is_file() {
                missing_thumbnails.push(thumbnail);
            }
        }
    }

    report
        .warnings
        .extend(describe_missing("アーカイブ", &missing_archives));
    report
        .warnings
        .extend(describe_missing("サムネイル", &missing_thumbnails));
    Ok(report)
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RegenerateSummary {
    pub regenerated: u64,
    pub failed: u64,
    pub first_failure: Option<String>,
}

struct ArchiveRecord {
    relative_path: String,
    format: String,
}

fn is_archive_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn archive_record(connection: &Connection, archive_id: &str) -> Result<ArchiveRecord, String> {
    if !is_archive_id(archive_id) {
        return Err("アーカイブIDが不正です。".to_string());
    }
    connection
        .query_row(
            "SELECT relative_path, format FROM archives WHERE archive_id = ?1",
            [archive_id],
            |row| {
                Ok(ArchiveRecord {
                    relative_path: row.get(0)?,
                    format: row.get(1)?,
                })
            },
        )
        .map_err(|_| "対象のアーカイブが見つかりません。".to_string())
}

fn archive_file(root: &Path, record: &ArchiveRecord) -> Result<PathBuf, String> {
    if !is_safe_relative(&record.relative_path) {
        return Err("アーカイブの保存先パスが不正です。".to_string());
    }
    let path = root.join(&record.relative_path);
    if !path.is_file() {
        return Err(format!(
            "アーカイブ本体が見つかりません（{}）。コピー元から復元してください。",
            record.relative_path
        ));
    }
    Ok(path)
}

pub fn list_cover_candidates(root: &Path, archive_id: &str) -> Result<Vec<String>, String> {
    let connection = Connection::open(root.join(DATABASE))
        .map_err(|error| format!("SQLiteデータベースを開けませんでした: {error}"))?;
    let record = archive_record(&connection, archive_id)?;
    crate::thumbnails::list_images(&archive_file(root, &record)?, &record.format)
}

// entry_name が None なら自動抽出の規則、Some なら利用者が選んだ画像から作り直す。
// 新しいサムネイルを完成させてから置き換えるため、失敗しても既存のサムネイルは変わらない。
pub fn regenerate_thumbnail(
    root: &Path,
    archive_id: &str,
    entry_name: Option<&str>,
) -> Result<(), String> {
    let connection = Connection::open(root.join(DATABASE))
        .map_err(|error| format!("SQLiteデータベースを開けませんでした: {error}"))?;
    let record = archive_record(&connection, archive_id)?;
    let archive_path = archive_file(root, &record)?;

    let staging_directory = root.join("thumbnails/.staging");
    fs::create_dir_all(&staging_directory)
        .map_err(|error| format!("サムネイル用一時ディレクトリを作成できませんでした: {error}"))?;
    let staging = staging_directory.join(format!("{archive_id}.regenerate.part"));
    let created = match entry_name {
        Some(entry) => crate::thumbnails::create_thumbnail_from_entry(
            &archive_path,
            &record.format,
            entry,
            &staging,
        ),
        None => crate::thumbnails::create_thumbnail(&archive_path, &record.format, &staging),
    };
    if let Err(error) = created {
        let _ = fs::remove_file(&staging);
        return Err(error);
    }

    let relative = format!("thumbnails/{}/{}.webp", &archive_id[..2], archive_id);
    let destination = root.join(&relative);
    let placed = fs::create_dir_all(destination.parent().unwrap_or(root))
        .and_then(|_| fs::rename(&staging, &destination));
    if let Err(error) = placed {
        let _ = fs::remove_file(&staging);
        return Err(format!("サムネイルを保存できませんでした: {error}"));
    }
    connection
        .execute(
            "UPDATE archives SET thumbnail_relative_path = ?1 WHERE archive_id = ?2",
            rusqlite::params![relative, archive_id],
        )
        .map_err(|error| format!("サムネイル情報をDBへ記録できませんでした: {error}"))?;
    Ok(())
}

// サムネイルが未登録、またはファイルが欠けているアーカイブを、自動抽出の規則で作り直す。
pub fn regenerate_missing_thumbnails(root: &Path) -> Result<RegenerateSummary, String> {
    let ids = {
        let connection = Connection::open(root.join(DATABASE))
            .map_err(|error| format!("SQLiteデータベースを開けませんでした: {error}"))?;
        let mut statement = connection
            .prepare("SELECT archive_id, thumbnail_relative_path FROM archives")
            .map_err(|error| format!("アーカイブ情報を確認できませんでした: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .map_err(|error| format!("アーカイブ情報を確認できませんでした: {error}"))?;
        let mut ids = Vec::new();
        for row in rows {
            let (id, thumbnail) =
                row.map_err(|error| format!("アーカイブ情報を読み取れませんでした: {error}"))?;
            let present = thumbnail
                .as_deref()
                .is_some_and(|path| is_safe_relative(path) && root.join(path).is_file());
            if !present {
                ids.push(id);
            }
        }
        ids
    };

    let mut summary = RegenerateSummary {
        regenerated: 0,
        failed: 0,
        first_failure: None,
    };
    for id in ids {
        match regenerate_thumbnail(root, &id, None) {
            Ok(()) => summary.regenerated += 1,
            Err(error) => {
                summary.failed += 1;
                summary
                    .first_failure
                    .get_or_insert(format!("{}…: {error}", &id[..8.min(id.len())]));
            }
        }
    }
    Ok(summary)
}

fn is_inside(path: &Path, parent: &Path) -> bool {
    path.starts_with(parent)
}

fn copy_tree(source: &Path, destination: &Path, copied: &mut u64) -> Result<(), String> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("{} を作成できませんでした: {error}", destination.display()))?;
    let entries = fs::read_dir(source)
        .map_err(|error| format!("{} を読み取れませんでした: {error}", source.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("ファイル一覧を読み取れませんでした: {error}"))?;
        let name = entry.file_name();
        // 一時ファイル置き場は未完了の作業データなので含めない。
        if name == ".staging" {
            continue;
        }
        let target = destination.join(&name);
        let file_type = entry
            .file_type()
            .map_err(|error| format!("ファイル種別を確認できませんでした: {error}"))?;
        if file_type.is_dir() {
            copy_tree(&entry.path(), &target, copied)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target).map_err(|error| {
                format!(
                    "{} をコピーできませんでした: {error}",
                    entry.path().display()
                )
            })?;
            *copied += 1;
        }
    }
    Ok(())
}

// リポジトリ全体を、そのまま別PCで開ける自己完結した形でコピーする。
// 呼び出し側でインポートを止めた状態で呼ぶこと。
pub fn backup(root: &Path, destination: &Path) -> Result<BackupSummary, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("リポジトリの場所を確定できませんでした: {error}"))?;
    if destination.exists() {
        let is_empty = fs::read_dir(destination)
            .map_err(|error| format!("バックアップ先を確認できませんでした: {error}"))?
            .next()
            .is_none();
        if !is_empty {
            return Err("バックアップ先は空のディレクトリを選択してください。".to_string());
        }
    }
    let created = !destination.exists();
    fs::create_dir_all(destination)
        .map_err(|error| format!("バックアップ先を作成できませんでした: {error}"))?;
    let destination = destination
        .canonicalize()
        .map_err(|error| format!("バックアップ先を確定できませんでした: {error}"))?;
    if is_inside(&destination, &root) || is_inside(&root, &destination) {
        if created {
            let _ = fs::remove_dir(&destination);
        }
        return Err(
            "バックアップ先にリポジトリの内側、またはリポジトリを含む場所は選べません。"
                .to_string(),
        );
    }

    let result = copy_repository(&root, &destination);
    if result.is_err() {
        // 不完全なバックアップを有効なリポジトリに見せないため、作成した内容を片付ける。
        if let Ok(entries) = fs::read_dir(&destination) {
            for entry in entries.flatten() {
                let path = entry.path();
                let _ = if path.is_dir() {
                    fs::remove_dir_all(path)
                } else {
                    fs::remove_file(path)
                };
            }
        }
    }
    result
}

fn copy_repository(root: &Path, destination: &Path) -> Result<BackupSummary, String> {
    let mut copied = 0;
    copy_tree(
        &root.join("archives"),
        &destination.join("archives"),
        &mut copied,
    )?;
    copy_tree(
        &root.join("thumbnails"),
        &destination.join("thumbnails"),
        &mut copied,
    )?;

    // 実行中のDBを単純コピーせず、SQLiteが一貫したスナップショットを書き出す。
    fs::create_dir_all(destination.join("database"))
        .map_err(|error| format!("バックアップ先を作成できませんでした: {error}"))?;
    let connection = Connection::open(root.join(DATABASE))
        .map_err(|error| format!("SQLiteデータベースを開けませんでした: {error}"))?;
    let database_target: PathBuf = destination.join(DATABASE);
    connection
        .execute(
            "VACUUM INTO ?1",
            [database_target.to_string_lossy().as_ref()],
        )
        .map_err(|error| format!("DBのバックアップに失敗しました: {error}"))?;
    let archive_count: u64 = connection
        .query_row("SELECT COUNT(*) FROM archives", [], |row| row.get(0))
        .map_err(|error| format!("アーカイブ数を確認できませんでした: {error}"))?;

    // 最後にリポジトリ情報を置き、途中で失敗したバックアップが有効なリポジトリに見えないようにする。
    fs::copy(root.join(MANIFEST), destination.join(MANIFEST))
        .map_err(|error| format!("リポジトリ情報をコピーできませんでした: {error}"))?;

    Ok(BackupSummary {
        destination: destination.to_string_lossy().into_owned(),
        archive_count,
        file_count: copied,
    })
}

#[cfg(test)]
mod tests {
    use super::{backup, verify};
    use crate::repository;
    use rusqlite::Connection;
    use std::fs;

    fn repository_with_archive() -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temporary directory");
        repository::open(directory.path()).expect("initialize");
        fs::create_dir_all(directory.path().join("archives/ab")).unwrap();
        fs::create_dir_all(directory.path().join("thumbnails/ab")).unwrap();
        fs::write(directory.path().join("archives/ab/book.cbz"), b"data").unwrap();
        fs::write(directory.path().join("thumbnails/ab/x.webp"), b"img").unwrap();
        Connection::open(directory.path().join("database/library.sqlite3"))
            .unwrap()
            .execute(
                "INSERT INTO archives (archive_id, original_filename, relative_path, format,
                    file_size, thumbnail_relative_path, imported_at)
                 VALUES (?1, 'book.cbz', 'archives/ab/book.cbz', 'cbz', 4,
                    'thumbnails/ab/x.webp', '2026-01-01T00:00:00Z')",
                ["ab".repeat(32)],
            )
            .unwrap();
        directory
    }

    #[test]
    fn verifies_clean_and_damaged_repositories() {
        let repo = repository_with_archive();
        assert!(verify(repo.path()).unwrap().warnings.is_empty());

        fs::remove_file(repo.path().join("thumbnails/ab/x.webp")).unwrap();
        fs::remove_file(repo.path().join("archives/ab/book.cbz")).unwrap();
        let warnings = verify(repo.path()).unwrap().warnings;
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("archives/ab/book.cbz"));
        assert!(warnings[1].contains("thumbnails/ab/x.webp"));
    }

    #[test]
    fn regenerates_missing_thumbnails_and_picks_a_cover() {
        use image::ImageEncoder;
        use std::io::Write;
        use zip::{write::SimpleFileOptions, ZipWriter};

        let png = |color: u8| {
            let image = image::RgbImage::from_pixel(40, 60, image::Rgb([color, 10, 10]));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::codecs::png::PngEncoder::new(&mut bytes)
                .write_image(image.as_raw(), 40, 60, image::ExtendedColorType::Rgb8)
                .unwrap();
            bytes.into_inner()
        };
        let repo = repository_with_archive();
        let root = repo.path();
        let mut writer =
            ZipWriter::new(fs::File::create(root.join("archives/ab/book.cbz")).unwrap());
        for (name, color) in [("p1.png", 200_u8), ("p2.png", 50)] {
            writer
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&png(color)).unwrap();
        }
        writer.finish().unwrap();
        let id = "ab".repeat(32);

        fs::remove_file(root.join("thumbnails/ab/x.webp")).unwrap();
        let connection = Connection::open(root.join("database/library.sqlite3")).unwrap();
        connection
            .execute("UPDATE archives SET thumbnail_relative_path = NULL", [])
            .unwrap();
        drop(connection);

        let summary = super::regenerate_missing_thumbnails(root).unwrap();
        assert_eq!((summary.regenerated, summary.failed), (1, 0));
        let thumbnail = root.join(format!("thumbnails/ab/{id}.webp"));
        let automatic = fs::read(&thumbnail).unwrap();
        assert_eq!(
            super::regenerate_missing_thumbnails(root)
                .unwrap()
                .regenerated,
            0
        );

        assert_eq!(
            super::list_cover_candidates(root, &id).unwrap(),
            ["p1.png", "p2.png"]
        );
        super::regenerate_thumbnail(root, &id, Some("p2.png")).unwrap();
        assert_ne!(fs::read(&thumbnail).unwrap(), automatic);
        assert!(verify(root).unwrap().warnings.is_empty());

        assert!(super::regenerate_thumbnail(root, &id, Some("missing.png")).is_err());
        assert!(thumbnail.exists());
        assert!(super::regenerate_thumbnail(root, "zz", None).is_err());
    }

    #[test]
    fn backup_is_a_self_contained_repository() {
        let repo = repository_with_archive();
        let target = tempfile::tempdir().unwrap();
        let destination = target.path().join("copy");

        let summary = backup(repo.path(), &destination).expect("backup");
        assert_eq!(summary.archive_count, 1);
        assert_eq!(summary.file_count, 2);

        let restored = repository::open(&destination).expect("open backup");
        assert_eq!(restored.archive_count, 1);
        assert!(verify(&destination).unwrap().warnings.is_empty());
    }

    #[test]
    fn backup_rejects_non_empty_or_nested_destinations() {
        let repo = repository_with_archive();
        let target = tempfile::tempdir().unwrap();
        fs::write(target.path().join("keep.txt"), "x").unwrap();
        assert!(backup(repo.path(), target.path()).is_err());
        assert!(target.path().join("keep.txt").exists());

        let inside = repo.path().join("backup");
        assert!(backup(repo.path(), &inside).is_err());
    }
}
