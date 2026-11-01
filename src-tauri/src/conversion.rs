use image::{
    codecs::avif::AvifEncoder, metadata::Orientation, DynamicImage, ExtendedColorType,
    ImageDecoder, ImageEncoder, ImageReader, Limits,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, BufReader, Cursor, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use zip::ZipArchive;

const DATABASE: &str = "database/library.sqlite3";
const MAX_IMAGE_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_IMAGE_WIDTH: u32 = 20_000;
const MAX_IMAGE_HEIGHT: u32 = 30_000;
const MAX_DECODE_BYTES: u64 = 256 * 1024 * 1024;
const AVIF_QUALITY: u8 = 70;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertProgress {
    pub phase: &'static str,
    pub done_entries: u64,
    pub total_entries: u64,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub current_name: Option<String>,
    pub cancellable: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ConvertOutcome {
    #[serde(rename_all = "camelCase")]
    Converted {
        archive_id: String,
        relative_path: String,
        original_size: u64,
        new_size: u64,
        converted_images: u64,
        unchanged_entries: u64,
        warning: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Duplicate {
        archive_id: String,
        relative_path: String,
    },
    #[serde(rename_all = "camelCase")]
    NotSmaller {
        original_size: u64,
        new_size: u64,
    },
    Cancelled,
}

struct Reporter<'a> {
    sink: &'a mut dyn FnMut(ConvertProgress),
    last_sent: Option<Instant>,
}

impl Reporter<'_> {
    // 工程の切り替えは間引かず、画像ごとの更新だけを間引く。
    fn send(&mut self, progress: ConvertProgress, force: bool) {
        let due = self
            .last_sent
            .is_none_or(|sent| sent.elapsed() >= PROGRESS_INTERVAL);
        if force || due {
            (self.sink)(progress);
            self.last_sent = Some(Instant::now());
        }
    }
}

struct Record {
    original_filename: String,
    original_path: Option<String>,
    relative_path: String,
    format: String,
    page_count: Option<u64>,
    thumbnail_relative_path: Option<String>,
    is_favorite: i64,
    imported_at: String,
    last_read_at: Option<String>,
    read_count: i64,
}

struct PlanItem {
    index: usize,
    name: String,
    size: u64,
    avif_name: Option<String>,
}

struct Expected {
    name: String,
    size: u64,
    is_avif: bool,
}

fn is_archive_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn load_record(connection: &Connection, archive_id: &str) -> Result<Record, String> {
    if !is_archive_id(archive_id) {
        return Err("アーカイブIDが不正です。".to_string());
    }
    connection
        .query_row(
            "SELECT original_filename, original_path, relative_path, format, page_count,
                    thumbnail_relative_path, is_favorite, imported_at, last_read_at, read_count
             FROM archives WHERE archive_id = ?1",
            [archive_id],
            |row| {
                Ok(Record {
                    original_filename: row.get(0)?,
                    original_path: row.get(1)?,
                    relative_path: row.get(2)?,
                    format: row.get(3)?,
                    page_count: row
                        .get::<_, Option<i64>>(4)?
                        .map(|count| count.max(0) as u64),
                    thumbnail_relative_path: row.get(5)?,
                    is_favorite: row.get(6)?,
                    imported_at: row.get(7)?,
                    last_read_at: row.get(8)?,
                    read_count: row.get(9)?,
                })
            },
        )
        .map_err(|_| "対象のアーカイブが見つかりません。".to_string())
}

fn is_safe_relative(path: &str) -> bool {
    crate::maintenance::is_safe_relative(path)
}

fn lowercase_extension(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
}

// PNG/JPEGだけを変換対象にする。変換後の名前が他のエントリーと衝突する場合は変換しない。
fn plan_entries(archive: &mut ZipArchive<File>) -> Result<Vec<PlanItem>, String> {
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(format!(
            "アーカイブのエントリー数が上限（{MAX_ARCHIVE_ENTRIES}件）を超えています。"
        ));
    }
    let mut items = Vec::new();
    let mut taken = HashSet::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("ZIP内のエントリーを読み込めませんでした: {error}"))?;
        if entry.is_dir() {
            continue;
        }
        if entry.enclosed_name().is_none() {
            return Err(format!(
                "安全でないパスのエントリーがあるため変換できません: {}",
                entry.name()
            ));
        }
        taken.insert(entry.name().to_lowercase());
        items.push(PlanItem {
            index,
            name: entry.name().to_string(),
            size: entry.size(),
            avif_name: None,
        });
    }
    for item in &mut items {
        let convertible = matches!(
            lowercase_extension(&item.name).as_deref(),
            Some("png" | "jpg" | "jpeg")
        ) && item.size > 0
            && item.size <= MAX_IMAGE_ENTRY_BYTES;
        if !convertible {
            continue;
        }
        let target = Path::new(&item.name)
            .with_extension("avif")
            .to_string_lossy()
            .replace('\\', "/");
        if taken.insert(target.to_lowercase()) {
            item.avif_name = Some(target);
        }
    }
    Ok(items)
}

fn is_animated_png(bytes: &[u8]) -> bool {
    if bytes.len() < 8 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return false;
    }
    let mut position = 8;
    while position + 8 <= bytes.len() {
        let length = u32::from_be_bytes([
            bytes[position],
            bytes[position + 1],
            bytes[position + 2],
            bytes[position + 3],
        ]) as usize;
        match &bytes[position + 4..position + 8] {
            b"acTL" => return true,
            b"IDAT" => return false,
            _ => {}
        }
        position = match position.checked_add(12 + length) {
            Some(next) => next,
            None => return false,
        };
    }
    false
}

fn decode_with_orientation(bytes: &[u8]) -> Result<DynamicImage, String> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("画像形式を判定できませんでした: {error}"))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|error| format!("画像を開けませんでした: {error}"))?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_WIDTH);
    limits.max_image_height = Some(MAX_IMAGE_HEIGHT);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    decoder
        .set_limits(limits)
        .map_err(|error| format!("画像のデコード上限を超えています: {error}"))?;
    let mut image = DynamicImage::from_decoder(decoder)
        .map_err(|error| format!("画像をデコードできませんでした: {error}"))?;
    image.apply_orientation(orientation);
    Ok(image)
}

// 変換できない画像（アニメーション・破損・上限超過）は None を返し、そのままコピーする。
fn encode_avif(bytes: &[u8], speed: u8) -> Option<Vec<u8>> {
    if is_animated_png(bytes) {
        return None;
    }
    let image = decode_with_orientation(bytes).ok()?;
    let (width, height) = (image.width(), image.height());
    let mut output = Vec::new();
    let encoder = AvifEncoder::new_with_speed_quality(&mut output, speed, AVIF_QUALITY);
    let written = if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        encoder.write_image(rgba.as_raw(), width, height, ExtendedColorType::Rgba8)
    } else {
        let rgb = image.to_rgb8();
        encoder.write_image(rgb.as_raw(), width, height, ExtendedColorType::Rgb8)
    };
    written.ok().map(|_| output)
}

fn new_entry(name: &str) -> sevenz_rust::SevenZArchiveEntry {
    let mut entry = sevenz_rust::SevenZArchiveEntry::new();
    entry.name = name.to_string();
    entry.has_stream = true;
    entry
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut reader = BufReader::new(
        File::open(path).map_err(|error| format!("変換結果を開けませんでした: {error}"))?,
    );
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("変換結果を読み取れませんでした: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

// 書き出した7zを開き直し、エントリー名・順序・サイズ・内容の読み取り（CRC検証）・AVIFの形式を確認する。
fn verify_output(path: &Path, expected: &[Expected]) -> Result<(), String> {
    let mut reader = sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty())
        .map_err(|error| format!("変換結果の7zを開き直せませんでした: {error}"))?;
    let actual: Vec<&str> = reader
        .archive()
        .files
        .iter()
        .filter(|entry| !entry.is_directory())
        .map(|entry| entry.name())
        .collect();
    let wanted: Vec<&str> = expected.iter().map(|entry| entry.name.as_str()).collect();
    if actual != wanted {
        return Err("変換結果のエントリー一覧が元と一致しません。".to_string());
    }

    let mut streamed = expected.iter().filter(|entry| entry.size > 0);
    reader
        .for_each_entries(|entry, content| {
            let Some(want) = streamed.next() else {
                return Err(sevenz_rust::Error::other("余分なエントリーがあります"));
            };
            let mut header = [0_u8; 12];
            let mut filled = 0;
            while filled < header.len() {
                let read = content.read(&mut header[filled..])?;
                if read == 0 {
                    break;
                }
                filled += read;
            }
            let rest = io::copy(content, &mut io::sink())?;
            if entry.name() != want.name || filled as u64 + rest != want.size {
                return Err(sevenz_rust::Error::other(format!(
                    "エントリーの内容が一致しません: {}",
                    want.name
                )));
            }
            if want.is_avif && !(filled == header.len() && &header[4..12] == b"ftypavif") {
                return Err(sevenz_rust::Error::other(format!(
                    "AVIF画像として読み取れません: {}",
                    want.name
                )));
            }
            Ok(true)
        })
        .map_err(|error| format!("変換結果を検証できませんでした: {error}"))?;
    if streamed.next().is_some() {
        return Err("変換結果にエントリーが不足しています。".to_string());
    }
    Ok(())
}

pub fn convert_archive(
    root: &Path,
    archive_id: &str,
    avif_speed: u8,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(ConvertProgress),
) -> Result<ConvertOutcome, String> {
    if !(1..=10).contains(&avif_speed) {
        return Err("AVIF速度は1〜10で指定してください。".to_string());
    }
    let connection = Connection::open(root.join(DATABASE))
        .map_err(|error| format!("SQLiteデータベースを開けませんでした: {error}"))?;
    let record = load_record(&connection, archive_id)?;
    let extension = match record.format.as_str() {
        "zip" => "7z",
        "cbz" => "cb7",
        other => return Err(format!("ZIP/CBZ以外は変換できません: {other}")),
    };
    if !is_safe_relative(&record.relative_path) {
        return Err("アーカイブの保存先パスが不正です。".to_string());
    }
    let source_path = root.join(&record.relative_path);
    if !source_path.is_file() {
        return Err(format!(
            "アーカイブ本体が見つかりません（{}）。",
            record.relative_path
        ));
    }

    let staging_directory = root.join("archives/.staging");
    fs::create_dir_all(&staging_directory)
        .map_err(|error| format!("変換用一時ディレクトリを作成できませんでした: {error}"))?;
    let staging = staging_directory.join(format!("{archive_id}.convert.part"));

    let mut reporter = Reporter {
        sink: progress,
        last_sent: None,
    };
    let result = run_conversion(
        root,
        &connection,
        archive_id,
        &record,
        extension,
        &source_path,
        &staging,
        avif_speed,
        cancel,
        &mut reporter,
    );
    if !matches!(result, Ok(ConvertOutcome::Converted { .. })) {
        let _ = fs::remove_file(&staging);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn run_conversion(
    root: &Path,
    connection: &Connection,
    archive_id: &str,
    record: &Record,
    extension: &str,
    source_path: &Path,
    staging: &Path,
    avif_speed: u8,
    cancel: &AtomicBool,
    reporter: &mut Reporter,
) -> Result<ConvertOutcome, String> {
    let mut state = ConvertProgress {
        phase: "analyze",
        done_entries: 0,
        total_entries: 0,
        done_bytes: 0,
        total_bytes: 0,
        current_name: None,
        cancellable: true,
    };
    reporter.send(state.clone(), true);

    let file = File::open(source_path)
        .map_err(|error| format!("ZIPアーカイブを開けませんでした: {error}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("ZIPアーカイブが不正です: {error}"))?;
    let plan = plan_entries(&mut archive)?;
    if plan.is_empty() {
        return Err("変換できるファイルがアーカイブ内にありません。".to_string());
    }
    state.phase = "convert";
    state.total_entries = plan.len() as u64;
    state.total_bytes = plan.iter().map(|item| item.size).sum();
    reporter.send(state.clone(), true);

    let mut writer = sevenz_rust::SevenZWriter::create(staging)
        .map_err(|error| format!("変換用の7zを作成できませんでした: {error}"))?;
    let mut expected = Vec::with_capacity(plan.len());
    let (mut converted, mut unchanged) = (0_u64, 0_u64);

    for item in &plan {
        if cancel.load(Ordering::SeqCst) {
            return Ok(ConvertOutcome::Cancelled);
        }
        state.current_name = Some(item.name.clone());
        reporter.send(state.clone(), false);

        let mut entry = archive
            .by_index(item.index)
            .map_err(|error| format!("ZIP内のエントリーを展開できませんでした: {error}"))?;
        let write_error = |error: sevenz_rust::Error| {
            format!("7zへ書き込めませんでした ({}): {error}", item.name)
        };

        let mut handled = false;
        if let Some(avif_name) = &item.avif_name {
            let mut bytes = Vec::with_capacity(item.size as usize);
            (&mut entry)
                .take(MAX_IMAGE_ENTRY_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| {
                    format!("ZIP内の画像を展開できませんでした ({}): {error}", item.name)
                })?;
            match encode_avif(&bytes, avif_speed).filter(|avif| avif.len() < bytes.len()) {
                Some(avif) => {
                    let written = writer
                        .push_archive_entry(new_entry(avif_name), Some(Cursor::new(&avif)))
                        .map_err(write_error)?;
                    expected.push(Expected {
                        name: avif_name.clone(),
                        size: written.size(),
                        is_avif: true,
                    });
                    converted += 1;
                }
                None => {
                    let written = writer
                        .push_archive_entry(new_entry(&item.name), Some(Cursor::new(&bytes)))
                        .map_err(write_error)?;
                    expected.push(Expected {
                        name: item.name.clone(),
                        size: written.size(),
                        is_avif: false,
                    });
                    unchanged += 1;
                }
            }
            handled = true;
        }
        if !handled {
            let written = writer
                .push_archive_entry(new_entry(&item.name), Some(&mut entry))
                .map_err(write_error)?;
            expected.push(Expected {
                name: item.name.clone(),
                size: written.size(),
                is_avif: false,
            });
            unchanged += 1;
        }
        state.done_entries += 1;
        state.done_bytes += item.size;
        reporter.send(state.clone(), false);
    }

    state.phase = "write";
    state.current_name = None;
    reporter.send(state.clone(), true);
    writer
        .finish()
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("7zを書き出せませんでした: {error}"))?;
    drop(archive);

    let original_size = fs::metadata(source_path)
        .map_err(|error| format!("元のアーカイブを確認できませんでした: {error}"))?
        .len();
    let new_size = fs::metadata(staging)
        .map_err(|error| format!("変換結果を確認できませんでした: {error}"))?
        .len();
    if new_size >= original_size {
        return Ok(ConvertOutcome::NotSmaller {
            original_size,
            new_size,
        });
    }
    if cancel.load(Ordering::SeqCst) {
        return Ok(ConvertOutcome::Cancelled);
    }

    state.phase = "verify";
    reporter.send(state.clone(), true);
    verify_output(staging, &expected)?;
    if cancel.load(Ordering::SeqCst) {
        return Ok(ConvertOutcome::Cancelled);
    }

    // ここから先はファイルとDBを入れ替えるため、中断できない。
    state.phase = "finalize";
    state.cancellable = false;
    reporter.send(state.clone(), true);
    finalize(
        root,
        connection,
        archive_id,
        record,
        extension,
        source_path,
        staging,
        (original_size, new_size, converted, unchanged),
    )
}

#[allow(clippy::too_many_arguments)]
fn finalize(
    root: &Path,
    connection: &Connection,
    old_id: &str,
    record: &Record,
    extension: &str,
    source_path: &Path,
    staging: &Path,
    (original_size, new_size, converted, unchanged): (u64, u64, u64, u64),
) -> Result<ConvertOutcome, String> {
    let new_id = hash_file(staging)?;
    let existing: Option<String> = connection
        .query_row(
            "SELECT relative_path FROM archives WHERE archive_id = ?1 OR converted_from = ?1",
            [&new_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("重複チェックに失敗しました: {error}"))?;
    if let Some(relative_path) = existing {
        return Ok(ConvertOutcome::Duplicate {
            archive_id: new_id,
            relative_path,
        });
    }

    let stem = Path::new(&record.original_filename)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| "元のファイル名から変換後の名前を作れませんでした。".to_string())?;
    let new_filename = format!("{stem}.{extension}");
    let archive_relative = format!("archives/{}/{}", &new_id[..2], new_filename);
    let archive_destination = root.join(&archive_relative);
    if archive_destination.exists() {
        return Err(format!(
            "保存先に同名のファイルが既にあるため確定できません（{archive_relative}）。元のアーカイブはそのまま残しています。"
        ));
    }

    let thumbnail_relative = format!("thumbnails/{}/{}.webp", &new_id[..2], new_id);
    let thumbnail_destination = root.join(&thumbnail_relative);
    let thumbnail_staging_directory = root.join("thumbnails/.staging");
    fs::create_dir_all(&thumbnail_staging_directory)
        .map_err(|error| format!("サムネイル用一時ディレクトリを作成できませんでした: {error}"))?;
    let thumbnail_staging = thumbnail_staging_directory.join(format!("{new_id}.convert.part"));
    let old_thumbnail = record
        .thumbnail_relative_path
        .as_deref()
        .filter(|path| is_safe_relative(path))
        .map(|path| root.join(path))
        .filter(|path| path.is_file());
    // AVIFは表紙の再抽出に使えないため、既存のサムネイルを新しいIDの名前へ引き継ぐ。
    let thumbnail_ready = match &old_thumbnail {
        Some(path) => fs::copy(path, &thumbnail_staging)
            .map(|_| ())
            .map_err(|error| format!("サムネイルを引き継げませんでした: {error}")),
        None => {
            crate::thumbnails::create_thumbnail(source_path, &record.format, &thumbnail_staging)
        }
    };
    if let Err(error) = thumbnail_ready {
        let _ = fs::remove_file(&thumbnail_staging);
        return Err(error);
    }

    let placed = place(staging, &archive_destination)
        .and_then(|_| place(&thumbnail_staging, &thumbnail_destination));
    if let Err(error) = placed {
        let _ = fs::remove_file(&thumbnail_staging);
        let _ = fs::remove_file(&archive_destination);
        let _ = fs::remove_file(&thumbnail_destination);
        return Err(error);
    }

    let registered = register(
        connection,
        old_id,
        &new_id,
        record,
        &new_filename,
        &archive_relative,
        &thumbnail_relative,
        new_size,
    );
    if let Err(error) = registered {
        let _ = fs::remove_file(&archive_destination);
        let _ = fs::remove_file(&thumbnail_destination);
        return Err(error);
    }

    let mut leftovers = Vec::new();
    if let Err(error) = fs::remove_file(source_path) {
        leftovers.push(format!("元のZIPを削除できませんでした: {error}"));
    }
    if let Some(path) = &old_thumbnail {
        if let Err(error) = fs::remove_file(path) {
            leftovers.push(format!("旧サムネイルを削除できませんでした: {error}"));
        }
    }
    for path in [Some(source_path), old_thumbnail.as_deref()]
        .into_iter()
        .flatten()
    {
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir(parent);
        }
    }

    Ok(ConvertOutcome::Converted {
        archive_id: new_id,
        relative_path: archive_relative,
        original_size,
        new_size,
        converted_images: converted,
        unchanged_entries: unchanged,
        warning: (!leftovers.is_empty()).then(|| {
            format!(
                "変換は完了しましたが、{} 手動で削除してください。",
                leftovers.join(" ")
            )
        }),
    })
}

fn place(from: &Path, to: &Path) -> Result<(), String> {
    let directory: PathBuf = to
        .parent()
        .ok_or_else(|| "保存先を作成できませんでした。".to_string())?
        .to_path_buf();
    fs::create_dir_all(&directory)
        .and_then(|_| fs::rename(from, to))
        .map_err(|error| format!("ファイルを保存先へ配置できませんでした: {error}"))
}

// 新しい行の追加・タグとお気に入りの引き継ぎ・旧行の削除を1つのトランザクションで確定する。
#[allow(clippy::too_many_arguments)]
fn register(
    connection: &Connection,
    old_id: &str,
    new_id: &str,
    record: &Record,
    filename: &str,
    relative_path: &str,
    thumbnail_relative: &str,
    file_size: u64,
) -> Result<(), String> {
    let file_size = i64::try_from(file_size)
        .map_err(|_| "アーカイブのサイズがDBで扱える範囲を超えています。".to_string())?;
    let page_count = record
        .page_count
        .map(i64::try_from)
        .transpose()
        .map_err(|_| "ページ数がDBで扱える範囲を超えています。".to_string())?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("DBトランザクションを開始できませんでした: {error}"))?;
    let work = (|| -> rusqlite::Result<()> {
        transaction.execute(
            "INSERT INTO archives (
                archive_id, original_filename, original_path, relative_path, format,
                file_size, thumbnail_relative_path, is_favorite, imported_at, converted_from,
                page_count, last_read_at, read_count
             ) VALUES (?1, ?2, ?3, ?4, '7z', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                new_id,
                filename,
                record.original_path,
                relative_path,
                file_size,
                thumbnail_relative,
                record.is_favorite,
                record.imported_at,
                old_id,
                page_count,
                record.last_read_at,
                record.read_count,
            ],
        )?;
        transaction.execute(
            "INSERT INTO archive_tags (archive_id, tag_id)
             SELECT ?1, tag_id FROM archive_tags WHERE archive_id = ?2",
            params![new_id, old_id],
        )?;
        transaction.execute("DELETE FROM archive_tags WHERE archive_id = ?1", [old_id])?;
        transaction.execute("DELETE FROM archives WHERE archive_id = ?1", [old_id])?;
        Ok(())
    })();
    work.map_err(|error| format!("DBへ変換結果を記録できませんでした: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("DBへの記録を確定できませんでした: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut image = image::RgbImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            let noise = (x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503)) >> 7 & 31;
            *pixel = image::Rgb([
                ((x / 2 + noise) % 256) as u8,
                ((y / 3 + noise) % 256) as u8,
                (128 + noise) as u8,
            ]);
        }
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .expect("png");
        bytes
    }

    fn setup(entries: &[(&str, Vec<u8>)]) -> (tempfile::TempDir, String) {
        let root = tempfile::tempdir().expect("repository");
        std::fs::create_dir_all(root.path().join("database")).expect("db dir");
        crate::database::initialize(&root.path().join(DATABASE)).expect("init");
        let id = "ab".repeat(32);
        let archive_dir = root.path().join("archives/ab");
        std::fs::create_dir_all(&archive_dir).expect("archive dir");
        let file = File::create(archive_dir.join("sample.cbz")).expect("zip");
        let mut zip = zip::ZipWriter::new(file);
        for (name, bytes) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .expect("entry");
            zip.write_all(bytes).expect("write");
        }
        zip.finish().expect("finish");
        let size = std::fs::metadata(archive_dir.join("sample.cbz"))
            .expect("meta")
            .len();
        let thumbnail_dir = root.path().join("thumbnails/ab");
        std::fs::create_dir_all(&thumbnail_dir).expect("thumb dir");
        std::fs::write(thumbnail_dir.join(format!("{id}.webp")), b"RIFFthumb").expect("thumb");

        let connection = Connection::open(root.path().join(DATABASE)).expect("open");
        let page_count = entries
            .iter()
            .filter(|(name, _)| {
                Path::new(name)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        matches!(
                            extension.to_ascii_lowercase().as_str(),
                            "jpg" | "jpeg" | "png"
                        )
                    })
            })
            .count() as i64;
        connection
            .execute(
                "INSERT INTO archives (archive_id, original_filename, relative_path, format,
                    file_size, thumbnail_relative_path, is_favorite, imported_at, page_count)
                 VALUES (?1, 'sample.cbz', 'archives/ab/sample.cbz', 'cbz', ?2, ?3, 1, '2026-01-01T00:00:00Z', ?4)",
                params![id, size as i64, format!("thumbnails/ab/{id}.webp"), page_count],
            )
            .expect("insert");
        connection
            .execute(
                "INSERT INTO tags (id, name, category) VALUES (1, 'author', 'author')",
                [],
            )
            .expect("tag");
        connection
            .execute(
                "INSERT INTO archive_tags (archive_id, tag_id) VALUES (?1, 1)",
                [&id],
            )
            .expect("link");
        (root, id)
    }

    #[test]
    fn converts_images_and_carries_over_metadata() {
        let note = vec![b'x'; 4000];
        let (root, old_id) = setup(&[
            ("b/002.png", png_bytes(300, 400)),
            ("a/001.png", png_bytes(300, 400)),
            ("notes.txt", note.clone()),
            ("same.png", png_bytes(40, 40)),
            ("same.jpg", b"not-a-jpeg".to_vec()),
        ]);
        let cancel = AtomicBool::new(false);
        let mut phases = Vec::new();
        let outcome = convert_archive(root.path(), &old_id, 6, &cancel, &mut |progress| {
            phases.push(progress.phase)
        })
        .expect("convert");
        let ConvertOutcome::Converted {
            archive_id,
            relative_path,
            converted_images,
            ..
        } = outcome
        else {
            panic!("expected conversion, got {outcome:?}");
        };
        assert!(converted_images >= 2);
        assert!(relative_path.ends_with("/sample.cb7"));
        assert!(phases.contains(&"verify") && phases.contains(&"finalize"));

        let connection = Connection::open(root.path().join(DATABASE)).expect("open");
        let (favorite, converted_from, format, page_count): (i64, String, String, i64) = connection
            .query_row(
                "SELECT is_favorite, converted_from, format, page_count
                 FROM archives WHERE archive_id = ?1",
                [&archive_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("new row");
        assert_eq!(
            (favorite, converted_from, format, page_count),
            (1, old_id.clone(), "7z".to_string(), 4)
        );
        let tags: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM archive_tags WHERE archive_id = ?1",
                [&archive_id],
                |row| row.get(0),
            )
            .expect("tags");
        assert_eq!(tags, 1);
        let old_rows: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM archives WHERE archive_id = ?1",
                [&old_id],
                |row| row.get(0),
            )
            .expect("old");
        assert_eq!(old_rows, 0);
        assert!(!root.path().join("archives/ab/sample.cbz").exists());
        assert!(root
            .path()
            .join(format!(
                "thumbnails/{}/{}.webp",
                &archive_id[..2],
                archive_id
            ))
            .is_file());
        assert!(!root
            .path()
            .join(format!("thumbnails/ab/{old_id}.webp"))
            .exists());

        let mut reader = sevenz_rust::SevenZReader::open(
            root.path().join(&relative_path),
            sevenz_rust::Password::empty(),
        )
        .expect("open 7z");
        let names: Vec<String> = reader
            .archive()
            .files
            .iter()
            .map(|entry| entry.name().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "b/002.avif",
                "a/001.avif",
                "notes.txt",
                "same.avif",
                "same.jpg"
            ]
        );
        let mut notes = Vec::new();
        reader
            .for_each_entries(|entry, content| {
                if entry.name() == "notes.txt" {
                    content.read_to_end(&mut notes)?;
                } else {
                    io::copy(content, &mut io::sink())?;
                }
                Ok(true)
            })
            .expect("read");
        assert_eq!(notes, note);
    }

    #[test]
    fn cancellation_keeps_the_original() {
        let (root, id) = setup(&[("a.png", png_bytes(100, 100))]);
        let cancel = AtomicBool::new(true);
        let outcome = convert_archive(root.path(), &id, 6, &cancel, &mut |_| {}).expect("convert");
        assert_eq!(outcome, ConvertOutcome::Cancelled);
        assert!(root.path().join("archives/ab/sample.cbz").is_file());
        assert!(!root
            .path()
            .join(format!("archives/.staging/{id}.convert.part"))
            .exists());
    }

    #[test]
    fn rejects_avif_speed_outside_supported_range() {
        let cancel = AtomicBool::new(false);
        let error = convert_archive(
            Path::new("missing-repository"),
            "id",
            0,
            &cancel,
            &mut |_| {},
        )
        .expect_err("invalid speed should be rejected");
        assert_eq!(error, "AVIF速度は1〜10で指定してください。");
    }

    #[test]
    fn detects_animated_png() {
        assert!(!is_animated_png(&png_bytes(8, 8)));
        let mut animated = b"\x89PNG\r\n\x1a\n".to_vec();
        animated.extend_from_slice(&[0, 0, 0, 0]);
        animated.extend_from_slice(b"acTL");
        animated.extend_from_slice(&[0; 4]);
        assert!(is_animated_png(&animated));
    }
}
