use image::{imageops::FilterType, ImageReader, Limits};
use std::{
    fs::File,
    io::{Cursor, Read, Write},
    path::Path,
};
use zip::ZipArchive;

const MAX_IMAGE_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_IMAGE_WIDTH: u32 = 20_000;
const MAX_IMAGE_HEIGHT: u32 = 30_000;
const MAX_DECODE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_THUMBNAIL_FILE_BYTES: u64 = 32 * 1024 * 1024;
const THUMBNAIL_MAX_WIDTH: u32 = 400;
const THUMBNAIL_MAX_HEIGHT: u32 = 600;
const THUMBNAIL_QUALITY: f32 = 80.0;

pub fn create_thumbnail(
    archive_path: &Path,
    format: &str,
    output_path: &Path,
) -> Result<(), String> {
    let image_bytes = match format {
        "zip" | "cbz" => extract_zip_cover(archive_path)?,
        "7z" => extract_7z_cover(archive_path)?,
        "rar" | "cbr" => extract_rar_cover(archive_path)?,
        _ => return Err(format!("サムネイルを作成できない形式です: {format}")),
    };

    let mut reader = ImageReader::new(Cursor::new(image_bytes))
        .with_guessed_format()
        .map_err(|error| format!("表紙画像の形式を判定できませんでした: {error}"))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_WIDTH);
    limits.max_image_height = Some(MAX_IMAGE_HEIGHT);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| format!("表紙画像をデコードできませんでした: {error}"))?;
    let thumbnail = image.resize(
        THUMBNAIL_MAX_WIDTH,
        THUMBNAIL_MAX_HEIGHT,
        FilterType::Lanczos3,
    );
    let thumbnail = thumbnail.to_rgba8();

    let encoded =
        webp::Encoder::from_rgba(thumbnail.as_raw(), thumbnail.width(), thumbnail.height())
            .encode(THUMBNAIL_QUALITY);
    let mut file = File::create(output_path)
        .map_err(|error| format!("サムネイルの一時ファイルを作成できませんでした: {error}"))?;
    file.write_all(&encoded)
        .map_err(|error| format!("WebPサムネイルを書き込めませんでした: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("WebPサムネイルをディスクへ保存できませんでした: {error}"))
}

fn extract_zip_cover(archive_path: &Path) -> Result<Vec<u8>, String> {
    let file = File::open(archive_path)
        .map_err(|error| format!("ZIPアーカイブを開けませんでした: {error}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("ZIPアーカイブが不正です: {error}"))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(format!(
            "アーカイブのエントリー数が上限（{MAX_ARCHIVE_ENTRIES}件）を超えています。"
        ));
    }

    let mut candidates = Vec::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("ZIP内のファイル一覧を読み込めませんでした: {error}"))?;
        if entry.is_file() && is_supported_image(&entry.name()) && entry.size() > 0 {
            candidates.push((index, cover_priority(entry.name()), entry.size()));
        }
    }
    candidates.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));

    let mut last_error = None;
    for (index, _, declared_size) in candidates {
        if declared_size > MAX_IMAGE_ENTRY_BYTES {
            continue;
        }
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("ZIP内の表紙候補を開けませんでした: {error}"))?;
        let mut bytes = Vec::with_capacity(declared_size as usize);
        if let Err(error) = entry
            .take(MAX_IMAGE_ENTRY_BYTES + 1)
            .read_to_end(&mut bytes)
        {
            last_error = Some(format!("ZIP内の画像を展開できませんでした: {error}"));
            continue;
        }
        if bytes.len() as u64 > MAX_IMAGE_ENTRY_BYTES {
            continue;
        }
        match validate_image(&bytes) {
            Ok(()) => return Ok(bytes),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        "アーカイブ内にサムネイルに使える画像ファイルが見つかりませんでした（または画像が大きすぎます）。"
            .to_string()
    }))
}

fn extract_7z_cover(archive_path: &Path) -> Result<Vec<u8>, String> {
    let mut reader = sevenz_rust::SevenZReader::open(archive_path, sevenz_rust::Password::empty())
        .map_err(|error| format!("7zアーカイブを開けませんでした: {error}"))?;
    let mut cover: Option<(u8, Vec<u8>)> = None;
    let mut image_error = None;
    let mut entry_count = 0_usize;

    reader
        .for_each_entries(|entry, contents| {
            entry_count += 1;
            if entry_count > MAX_ARCHIVE_ENTRIES {
                return Err(sevenz_rust::Error::other(format!(
                    "アーカイブのエントリー数が上限（{MAX_ARCHIVE_ENTRIES}件）を超えています"
                )));
            }
            if entry.is_directory() || !is_supported_image(entry.name()) || !entry.has_stream() {
                std::io::copy(contents, &mut std::io::sink()).map_err(sevenz_rust::Error::io)?;
                return Ok(true);
            }
            if entry.size() == 0 || entry.size() > MAX_IMAGE_ENTRY_BYTES {
                std::io::copy(contents, &mut std::io::sink()).map_err(sevenz_rust::Error::io)?;
                return Ok(true);
            }

            let mut bytes = Vec::with_capacity(entry.size() as usize);
            contents
                .take(MAX_IMAGE_ENTRY_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(sevenz_rust::Error::io)?;
            if bytes.len() as u64 > MAX_IMAGE_ENTRY_BYTES {
                return Ok(true);
            }
            match validate_image(&bytes) {
                Ok(()) => {
                    let priority = cover_priority(entry.name());
                    if cover
                        .as_ref()
                        .is_none_or(|(best_priority, _)| priority < *best_priority)
                    {
                        cover = Some((priority, bytes));
                    }
                    Ok(true)
                }
                Err(error) => {
                    image_error = Some(error);
                    Ok(true)
                }
            }
        })
        .map_err(|error| format!("7z内のファイルを読み込めませんでした: {error}"))?;

    cover.map(|(_, bytes)| bytes).ok_or_else(|| {
        image_error.unwrap_or_else(|| {
            "7zアーカイブ内にサムネイルに使える画像ファイルが見つかりませんでした（または画像が大きすぎます）。"
                .to_string()
        })
    })
}

fn extract_rar_cover(archive_path: &Path) -> Result<Vec<u8>, String> {
    let mut archive = unrar::Archive::new(archive_path)
        .open_for_processing()
        .map_err(|error| format!("RARアーカイブを開けませんでした: {error}"))?;
    let mut last_error = None;
    let mut cover: Option<(u8, Vec<u8>)> = None;
    let mut entry_count = 0_usize;

    loop {
        let Some(entry) = archive
            .read_header()
            .map_err(|error| format!("RAR内のファイル一覧を読み込めませんでした: {error}"))?
        else {
            break;
        };
        entry_count += 1;
        if entry_count > MAX_ARCHIVE_ENTRIES {
            return Err(format!(
                "アーカイブのエントリー数が上限（{MAX_ARCHIVE_ENTRIES}件）を超えています。"
            ));
        }
        let header = entry.entry();
        let is_candidate = !header.is_directory()
            && !header.is_split()
            && is_supported_image(&header.filename.to_string_lossy())
            && header.unpacked_size > 0
            && header.unpacked_size <= MAX_IMAGE_ENTRY_BYTES;
        let priority = cover_priority(&header.filename.to_string_lossy());

        if is_candidate {
            let (bytes, next) = entry
                .read()
                .map_err(|error| format!("RAR内の表紙候補を展開できませんでした: {error}"))?;
            match validate_image(&bytes) {
                Ok(()) => {
                    if cover
                        .as_ref()
                        .is_none_or(|(best_priority, _)| priority < *best_priority)
                    {
                        cover = Some((priority, bytes));
                    }
                }
                Err(error) => last_error = Some(error),
            }
            archive = next;
        } else {
            archive = entry
                .skip()
                .map_err(|error| format!("RAR内のファイルを読み飛ばせませんでした: {error}"))?;
        }
    }

    cover.map(|(_, bytes)| bytes).ok_or_else(|| last_error.unwrap_or_else(|| {
        "RARアーカイブ内にサムネイルに使える画像ファイルが見つかりませんでした（暗号化・分割ファイル・サイズ上限も確認してください）。"
            .to_string()
    }))
}

fn validate_image(bytes: &[u8]) -> Result<(), String> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("表紙候補の画像形式を判定できませんでした: {error}"))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_WIDTH);
    limits.max_image_height = Some(MAX_IMAGE_HEIGHT);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    reader
        .decode()
        .map(|_| ())
        .map_err(|error| format!("表紙候補を画像として読み込めませんでした: {error}"))
}

fn is_supported_image(name: &str) -> bool {
    matches!(
        Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp")
    )
}

fn cover_priority(name: &str) -> u8 {
    let name = name.to_lowercase();
    let basename = name.rsplit(['/', '\\']).next().unwrap_or(&name);
    if basename.contains("cover") || basename.contains("jacket") || basename.contains("front") {
        0
    } else if basename.starts_with("表紙") {
        0
    } else {
        1
    }
}

// 旧仕様（AVIF）で保存したサムネイルを、アーカイブからWebPへ作り直す。失敗したものは次回起動時に再試行する。
pub fn migrate_avif_thumbnails(repository_root: &Path) -> Result<u64, String> {
    let connection = rusqlite::Connection::open(repository_root.join("database/library.sqlite3"))
        .map_err(|error| format!("SQLiteデータベースを開けませんでした: {error}"))?;
    let legacy: Vec<(String, String, String, String)> = {
        let mut statement = connection
            .prepare(
                "SELECT archive_id, relative_path, format, thumbnail_relative_path
                 FROM archives WHERE thumbnail_relative_path LIKE '%.avif'",
            )
            .map_err(|error| format!("旧サムネイルを検索できませんでした: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|error| format!("旧サムネイルを読み取れませんでした: {error}"))?;
        rows
    };
    if legacy.is_empty() {
        return Ok(0);
    }

    let staging_directory = repository_root.join("thumbnails/.staging");
    std::fs::create_dir_all(&staging_directory)
        .map_err(|error| format!("サムネイル用一時ディレクトリを作成できませんでした: {error}"))?;
    let mut migrated = 0;
    for (archive_id, archive_path, format, old_path) in legacy {
        let relative = format!("thumbnails/{}/{archive_id}.webp", &archive_id[..2]);
        let destination = repository_root.join(&relative);
        let staging = staging_directory.join(format!("migrate-{archive_id}.webp.part"));
        let result = create_thumbnail(&repository_root.join(&archive_path), &format, &staging)
            .and_then(|()| {
                std::fs::create_dir_all(destination.parent().ok_or("保存先が不正です")?)
                    .and_then(|()| std::fs::rename(&staging, &destination))
                    .map_err(|error| format!("サムネイルを配置できませんでした: {error}"))
            })
            .and_then(|()| {
                connection
                    .execute(
                        "UPDATE archives SET thumbnail_relative_path = ?1 WHERE archive_id = ?2",
                        [&relative, &archive_id],
                    )
                    .map(|_| ())
                    .map_err(|error| format!("サムネイルのパスを更新できませんでした: {error}"))
            });
        match result {
            Ok(()) => {
                let _ = std::fs::remove_file(repository_root.join(old_path));
                migrated += 1;
            }
            Err(error) => {
                let _ = std::fs::remove_file(&staging);
                eprintln!("AVIFサムネイルをWebPへ移行できませんでした ({archive_id}): {error}");
            }
        }
    }
    Ok(migrated)
}

pub fn validate_existing_thumbnail(path: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("保存済みサムネイルの情報を取得できませんでした: {error}"))?;
    if metadata.len() > MAX_THUMBNAIL_FILE_BYTES {
        return Err("保存済みサムネイルのファイルサイズが上限を超えています。".to_string());
    }
    let mut reader = ImageReader::open(path)
        .map_err(|error| format!("保存済みサムネイルを開けませんでした: {error}"))?
        .with_guessed_format()
        .map_err(|error| format!("保存済みサムネイルの形式を判定できませんでした: {error}"))?;
    if reader.format() != Some(image::ImageFormat::WebP) {
        return Err("保存済みサムネイルがWebP形式ではありません。".to_string());
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(THUMBNAIL_MAX_WIDTH);
    limits.max_image_height = Some(THUMBNAIL_MAX_HEIGHT);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| format!("保存済みサムネイルが破損しています: {error}"))?;
    if image.width() > THUMBNAIL_MAX_WIDTH || image.height() > THUMBNAIL_MAX_HEIGHT {
        return Err("保存済みサムネイルの寸法が想定より大きいです。".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        cover_priority, create_thumbnail, migrate_avif_thumbnails, validate_existing_thumbnail,
        THUMBNAIL_MAX_HEIGHT, THUMBNAIL_MAX_WIDTH,
    };
    use image::{ImageEncoder, RgbImage};
    use std::{
        fs::File,
        io::{Cursor, Write},
        path::Path,
    };
    use zip::{write::SimpleFileOptions, ZipWriter};

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = RgbImage::from_pixel(width, height, image::Rgb([120, 80, 200]));
        let mut bytes = Cursor::new(Vec::new());
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                image.as_raw(),
                width,
                height,
                image::ExtendedColorType::Rgb8,
            )
            .expect("encode test image");
        bytes.into_inner()
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = ZipWriter::new(File::create(path).expect("create zip"));
        for (name, bytes) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .expect("start zip entry");
            writer.write_all(bytes).expect("write zip entry");
        }
        writer.finish().expect("finish zip");
    }

    #[test]
    fn writes_a_resized_webp_cover_from_cbz() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let archive_path = directory.path().join("comic.cbz");
        let output_path = directory.path().join("cover.webp");
        write_zip(
            &archive_path,
            &[
                ("pages/001.png", &png_bytes(900, 1400)),
                ("pages/cover.png", &png_bytes(800, 1200)),
            ],
        );

        create_thumbnail(&archive_path, "cbz", &output_path).expect("create thumbnail");
        validate_existing_thumbnail(&output_path).expect("validate WebP thumbnail");
        let image = image::open(output_path).expect("decode WebP");
        assert_eq!(image.width(), THUMBNAIL_MAX_WIDTH);
        assert!(image.height() <= THUMBNAIL_MAX_HEIGHT);
    }

    #[test]
    fn migrates_legacy_avif_records_to_webp() {
        let repository = tempfile::tempdir().expect("temporary repository");
        std::fs::create_dir_all(repository.path().join("database")).expect("database dir");
        let database_path = repository.path().join("database/library.sqlite3");
        crate::database::initialize(&database_path).expect("initialize database");
        let id = "cd".repeat(32);
        let archive_relative = "archives/cd/Legacy.cbz";
        let archive_path = repository.path().join(archive_relative);
        std::fs::create_dir_all(archive_path.parent().expect("parent")).expect("archive dir");
        write_zip(&archive_path, &[("cover.png", &png_bytes(300, 450))]);
        let old_relative = format!("thumbnails/cd/{id}.avif");
        std::fs::create_dir_all(repository.path().join("thumbnails/cd")).expect("thumbnail dir");
        std::fs::write(repository.path().join(&old_relative), b"legacy").expect("old avif");
        let connection = rusqlite::Connection::open(&database_path).expect("open database");
        connection
            .execute(
                "INSERT INTO archives (archive_id, original_filename, relative_path, format,
                    file_size, thumbnail_relative_path, imported_at)
                 VALUES (?1, 'Legacy.cbz', ?2, 'cbz', 1, ?3, '2026-01-01T00:00:00Z')",
                rusqlite::params![id, archive_relative, old_relative],
            )
            .expect("insert legacy archive");

        assert_eq!(
            migrate_avif_thumbnails(repository.path()).expect("migrate"),
            1
        );
        assert_eq!(
            migrate_avif_thumbnails(repository.path()).expect("rerun"),
            0
        );

        let new_relative = format!("thumbnails/cd/{id}.webp");
        let stored: String = connection
            .query_row(
                "SELECT thumbnail_relative_path FROM archives WHERE archive_id = ?1",
                [&id],
                |row| row.get(0),
            )
            .expect("read path");
        assert_eq!(stored, new_relative);
        validate_existing_thumbnail(&repository.path().join(new_relative)).expect("valid WebP");
        assert!(!repository.path().join(old_relative).exists());
    }

    #[test]
    fn chooses_named_cover_before_first_page_for_zip() {
        assert_eq!(cover_priority("pages/001.jpg"), 1);
        assert_eq!(cover_priority("pages/cover.jpg"), 0);
        assert_eq!(cover_priority("表紙.png"), 0);
    }
}
