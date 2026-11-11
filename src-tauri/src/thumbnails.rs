use image::{imageops::FilterType, ImageReader, Limits};
use pdfium_render::prelude::{PdfRenderConfig, Pdfium};
use quick_xml::{events::Event, Reader as XmlReader};
use std::{
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::OnceLock,
};
use zip::ZipArchive;

static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

const MAX_IMAGE_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_IMAGE_WIDTH: u32 = 20_000;
const MAX_IMAGE_HEIGHT: u32 = 30_000;
const MAX_DECODE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_THUMBNAIL_FILE_BYTES: u64 = 32 * 1024 * 1024;
const THUMBNAIL_MAX_WIDTH: u32 = 400;
const THUMBNAIL_MAX_HEIGHT: u32 = 600;
const THUMBNAIL_QUALITY: f32 = 80.0;
const PDF_RENDER_MAX_WIDTH: i32 = 1200;
const PDF_RENDER_MAX_HEIGHT: i32 = 1800;

pub fn create_thumbnail(
    archive_path: &Path,
    format: &str,
    output_path: &Path,
) -> Result<(), String> {
    let image_bytes = match format {
        "zip" | "cbz" => extract_zip_cover(archive_path)?,
        "7z" => extract_7z_cover(archive_path)?,
        "rar" | "cbr" => extract_rar_cover(archive_path)?,
        "pdf" => return create_pdf_thumbnail(archive_path, output_path),
        "epub" => extract_epub_cover(archive_path)?,
        "azw3" => extract_azw3_cover(archive_path)?,
        _ => return Err(format!("サムネイルを作成できない形式です: {format}")),
    };
    write_thumbnail(image_bytes, output_path)
}

pub fn create_thumbnail_from_file(image_path: &Path, output_path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(image_path)
        .map_err(|error| format!("パッケージ画像を確認できませんでした: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(
            "パッケージ画像にはシンボリックリンクではない通常ファイルを選択してください。"
                .to_string(),
        );
    }
    let extension = image_path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "対応する画像形式（PNG、WebP、JPEGなど）を選択してください。".to_string())?;
    if !matches!(
        extension.as_str(),
        "png" | "webp" | "jpg" | "jpeg" | "gif" | "bmp"
    ) {
        return Err(format!(
            "対応していないパッケージ画像形式です: .{extension}"
        ));
    }
    let mut image_file = File::open(image_path)
        .map_err(|error| format!("パッケージ画像を開けませんでした: {error}"))?;
    let size = image_file
        .metadata()
        .map_err(|error| format!("パッケージ画像の情報を取得できませんでした: {error}"))?
        .len();
    if size > MAX_IMAGE_ENTRY_BYTES {
        return Err(format!(
            "パッケージ画像のサイズが上限（{MAX_IMAGE_ENTRY_BYTES} bytes）を超えています。"
        ));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    image_file
        .read_to_end(&mut bytes)
        .map_err(|error| format!("パッケージ画像を読み込めませんでした: {error}"))?;
    write_thumbnail(bytes, output_path)
}

fn pdfium() -> Result<&'static Pdfium, String> {
    PDFIUM
        .get_or_init(|| {
            let bundled_path = std::env::current_exe()
                .ok()
                .and_then(|path| path.parent().map(|directory| directory.join("libpdfium")));
            let development_path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("binaries/libpdfium-x86_64-unknown-linux-gnu");
            let library_path = bundled_path
                .filter(|path| path.is_file())
                .or_else(|| development_path.is_file().then_some(development_path))
                .ok_or_else(|| {
                    "PDF処理ライブラリ（libpdfium）が見つかりません。アプリを再ビルドしてください。"
                        .to_string()
                })?;
            let bindings = Pdfium::bind_to_library(&library_path)
                .map_err(|error| format!("PDF処理ライブラリを読み込めませんでした: {error}"))?;
            Ok(Pdfium::new(bindings))
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn load_pdf(path: &Path) -> Result<pdfium_render::prelude::PdfDocument<'static>, String> {
    pdfium()?
        .load_pdf_from_file(path, None)
        .map_err(|error| format!("PDFを開けませんでした: {error}"))
}

fn create_pdf_thumbnail(archive_path: &Path, output_path: &Path) -> Result<(), String> {
    let pdfium = pdfium()?;
    let document = pdfium
        .load_pdf_from_file(archive_path, None)
        .map_err(|error| format!("PDFを開けませんでした: {error}"))?;
    let page = document
        .pages()
        .get(0)
        .map_err(|error| format!("PDFの先頭ページを読み込めませんでした: {error}"))?;
    let image = page
        .render_with_config(
            &PdfRenderConfig::new()
                .set_target_width(PDF_RENDER_MAX_WIDTH)
                .set_maximum_width(PDF_RENDER_MAX_WIDTH)
                .set_maximum_height(PDF_RENDER_MAX_HEIGHT),
        )
        .and_then(|bitmap| bitmap.as_image())
        .map_err(|error| format!("PDFの先頭ページを画像化できませんでした: {error}"))?;
    write_thumbnail_image(image, output_path)
}

// 自動抽出の規則に合わない作品向けに、利用者が選んだエントリーから作る。
pub fn create_thumbnail_from_entry(
    archive_path: &Path,
    format: &str,
    entry_name: &str,
    output_path: &Path,
) -> Result<(), String> {
    write_thumbnail(read_entry(archive_path, format, entry_name)?, output_path)
}

fn decode_limited(image_bytes: Vec<u8>) -> Result<image::DynamicImage, String> {
    let mut reader = ImageReader::new(Cursor::new(image_bytes))
        .with_guessed_format()
        .map_err(|error| format!("表紙画像の形式を判定できませんでした: {error}"))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_WIDTH);
    limits.max_image_height = Some(MAX_IMAGE_HEIGHT);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|error| format!("表紙画像をデコードできませんでした: {error}"))
}

fn encode_webp(
    image_bytes: Vec<u8>,
    max_width: u32,
    max_height: u32,
    quality: f32,
) -> Result<Vec<u8>, String> {
    let image = decode_limited(image_bytes)?;
    Ok(encode_webp_image(image, max_width, max_height, quality))
}

fn encode_webp_image(
    image: image::DynamicImage,
    max_width: u32,
    max_height: u32,
    quality: f32,
) -> Vec<u8> {
    let resized = image
        .resize(max_width, max_height, FilterType::Lanczos3)
        .to_rgba8();
    webp::Encoder::from_rgba(resized.as_raw(), resized.width(), resized.height())
        .encode(quality)
        .to_vec()
}

fn write_thumbnail(image_bytes: Vec<u8>, output_path: &Path) -> Result<(), String> {
    let encoded = encode_webp(
        image_bytes,
        THUMBNAIL_MAX_WIDTH,
        THUMBNAIL_MAX_HEIGHT,
        THUMBNAIL_QUALITY,
    )?;
    write_webp(encoded, output_path)
}

fn write_thumbnail_image(image: image::DynamicImage, output_path: &Path) -> Result<(), String> {
    let encoded = encode_webp_image(
        image,
        THUMBNAIL_MAX_WIDTH,
        THUMBNAIL_MAX_HEIGHT,
        THUMBNAIL_QUALITY,
    );
    write_webp(encoded, output_path)
}

fn write_webp(encoded: Vec<u8>, output_path: &Path) -> Result<(), String> {
    let mut file = File::create(output_path)
        .map_err(|error| format!("サムネイルの一時ファイルを作成できませんでした: {error}"))?;
    file.write_all(&encoded)
        .map_err(|error| format!("WebPサムネイルを書き込めませんでした: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("WebPサムネイルをディスクへ保存できませんでした: {error}"))
}

fn unsupported(format: &str) -> String {
    format!("サムネイルを作成できない形式です: {format}")
}

fn is_selectable_image(name: &str) -> bool {
    is_supported_image(name) && !name.split(['/', '\\']).any(|part| part == "..")
}

fn is_page_image(name: &str) -> bool {
    matches!(
        Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "avif")
    ) && !name.split(['/', '\\']).any(|part| part == "..")
}

pub fn count_pages(archive_path: &Path, format: &str) -> Result<Option<u64>, String> {
    let count = match format {
        "pdf" => {
            let document = load_pdf(archive_path)?;
            Some(document.pages().len() as u64)
        }
        "zip" | "cbz" => {
            let file = File::open(archive_path)
                .map_err(|error| format!("ZIPアーカイブを開けませんでした: {error}"))?;
            let archive = ZipArchive::new(file)
                .map_err(|error| format!("ZIPアーカイブが不正です: {error}"))?;
            if archive.len() > MAX_ARCHIVE_ENTRIES {
                return Err(format!(
                    "アーカイブのエントリー数が上限（{MAX_ARCHIVE_ENTRIES}件）を超えています。"
                ));
            }
            Some(
                archive
                    .file_names()
                    .filter(|name| !name.ends_with('/') && is_page_image(name))
                    .count() as u64,
            )
        }
        "7z" => {
            let reader =
                sevenz_rust::SevenZReader::open(archive_path, sevenz_rust::Password::empty())
                    .map_err(|error| format!("7zアーカイブを開けませんでした: {error}"))?;
            let entries = &reader.archive().files;
            if entries.len() > MAX_ARCHIVE_ENTRIES {
                return Err(format!(
                    "アーカイブのエントリー数が上限（{MAX_ARCHIVE_ENTRIES}件）を超えています。"
                ));
            }
            Some(
                entries
                    .iter()
                    .filter(|entry| !entry.is_directory() && is_page_image(entry.name()))
                    .count() as u64,
            )
        }
        "rar" | "cbr" => {
            let mut archive = unrar::Archive::new(archive_path)
                .open_for_processing()
                .map_err(|error| format!("RARアーカイブを開けませんでした: {error}"))?;
            let mut count = 0_u64;
            let mut entries = 0_usize;
            while let Some(entry) = archive
                .read_header()
                .map_err(|error| format!("RAR内のファイル一覧を読み込めませんでした: {error}"))?
            {
                entries += 1;
                if entries > MAX_ARCHIVE_ENTRIES {
                    return Err(format!(
                        "アーカイブのエントリー数が上限（{MAX_ARCHIVE_ENTRIES}件）を超えています。"
                    ));
                }
                let header = entry.entry();
                let name = header.filename.to_string_lossy();
                if !header.is_directory() && is_page_image(&name) {
                    count += 1;
                }
                archive = entry
                    .skip()
                    .map_err(|error| format!("RAR内のファイルを読み飛ばせませんでした: {error}"))?;
            }
            Some(count)
        }
        "epub" | "azw3" | "mkv" | "mp4" => None,
        _ => return Err(unsupported(format)),
    };
    Ok(count)
}

const MAX_EBOOK_METADATA_BYTES: u64 = 8 * 1024 * 1024;
const MAX_EPUB_CONTAINER_BYTES: u64 = 1024 * 1024;

fn read_zip_member(
    archive: &mut ZipArchive<File>,
    name: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, String> {
    let entry = archive
        .by_name(name)
        .map_err(|error| format!("EPUB内の {name} を開けませんでした: {error}"))?;
    if entry.size() == 0 || entry.size() > max_bytes {
        return Err(format!("EPUB内の {name} が空、または大きすぎます。"));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("EPUB内の {name} を読み込めませんでした: {error}"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!("EPUB内の {name} が大きすぎます。"));
    }
    Ok(bytes)
}

fn xml_attribute(
    element: &quick_xml::events::BytesStart<'_>,
    key: &str,
) -> Result<Option<String>, String> {
    for attribute in element.attributes().with_checks(false) {
        let attribute =
            attribute.map_err(|error| format!("EPUBのメタデータを解析できません: {error}"))?;
        if attribute.key.local_name().as_ref() == key.as_bytes() {
            return attribute
                .unescape_value()
                .map(|value| Some(value.into_owned()))
                .map_err(|error| format!("EPUBのメタデータを解析できません: {error}"));
        }
    }
    Ok(None)
}

fn epub_package_path(container: &str) -> Result<String, String> {
    let mut reader = XmlReader::from_str(container);
    reader.config_mut().trim_text(true);
    loop {
        match reader
            .read_event()
            .map_err(|error| format!("EPUBのcontainer.xmlを解析できません: {error}"))?
        {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == b"rootfile" =>
            {
                if let Some(path) = xml_attribute(&element, "full-path")? {
                    if !path.is_empty() {
                        return Ok(path);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Err("EPUBのcontainer.xmlにOPFファイルの指定がありません。".to_string())
}

fn epub_cover_candidates(package: &str) -> Result<Vec<String>, String> {
    let mut reader = XmlReader::from_str(package);
    reader.config_mut().trim_text(true);
    let mut cover_id = None;
    let mut manifest = Vec::new();
    loop {
        match reader
            .read_event()
            .map_err(|error| format!("EPUBのOPFメタデータを解析できません: {error}"))?
        {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == b"meta" =>
            {
                let name = xml_attribute(&element, "name")?;
                let content = xml_attribute(&element, "content")?;
                if name.as_deref() == Some("cover") {
                    cover_id = content;
                }
            }
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == b"item" =>
            {
                manifest.push((
                    xml_attribute(&element, "id")?,
                    xml_attribute(&element, "href")?,
                    xml_attribute(&element, "properties")?,
                ));
            }
            Event::Eof => break,
            _ => {}
        }
    }

    let mut candidates = Vec::new();
    if let Some(cover_id) = cover_id {
        if let Some((_, Some(href), _)) = manifest
            .iter()
            .find(|(id, _, _)| id.as_deref() == Some(cover_id.as_str()))
        {
            candidates.push(href.clone());
        }
    }
    candidates.extend(manifest.into_iter().filter_map(|(_, href, properties)| {
        properties
            .as_deref()
            .is_some_and(|properties| {
                properties
                    .split_ascii_whitespace()
                    .any(|p| p == "cover-image")
            })
            .then_some(href)
            .flatten()
    }));
    Ok(candidates)
}

fn decode_epub_href(href: &str) -> Result<String, String> {
    let href = href.split(['#', '?']).next().unwrap_or_default();
    let bytes = href.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3).ok_or_else(|| {
                "EPUBのカバー画像パスに不正なURLエスケープがあります。".to_string()
            })?;
            let value = std::str::from_utf8(hex)
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .ok_or_else(|| {
                    "EPUBのカバー画像パスに不正なURLエスケープがあります。".to_string()
                })?;
            decoded.push(value);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded)
        .map_err(|_| "EPUBのカバー画像パスがUTF-8ではありません。".to_string())
}

fn resolve_epub_href(package_path: &str, href: &str) -> Result<String, String> {
    let href = decode_epub_href(href)?;
    if href.starts_with('/') || href.contains('\\') {
        return Err("EPUBのカバー画像パスが不正です。".to_string());
    }
    let mut components = package_path
        .rsplit_once('/')
        .map(|(parent, _)| parent.split('/').collect::<Vec<_>>())
        .unwrap_or_default();
    for component in href.split('/') {
        match component {
            "" | "." => {}
            ".." if components.pop().is_none() => {
                return Err("EPUBのカバー画像パスがルート外を参照しています。".to_string())
            }
            ".." => {}
            part => components.push(part),
        }
    }
    if components.is_empty() {
        return Err("EPUBのカバー画像パスが空です。".to_string());
    }
    Ok(components.join("/"))
}

fn extract_epub_cover(archive_path: &Path) -> Result<Vec<u8>, String> {
    let file =
        File::open(archive_path).map_err(|error| format!("EPUBを開けませんでした: {error}"))?;
    let mut archive = ZipArchive::new(file).map_err(|error| format!("EPUBが不正です: {error}"))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(format!(
            "EPUBのエントリー数が上限（{MAX_ARCHIVE_ENTRIES}件）を超えています。"
        ));
    }
    let container = read_zip_member(
        &mut archive,
        "META-INF/container.xml",
        MAX_EPUB_CONTAINER_BYTES,
    )?;
    let container = std::str::from_utf8(&container)
        .map_err(|error| format!("EPUBのcontainer.xmlがUTF-8ではありません: {error}"))?;
    let package_path = epub_package_path(container)?;
    let package = read_zip_member(&mut archive, &package_path, MAX_EBOOK_METADATA_BYTES)?;
    let package = std::str::from_utf8(&package)
        .map_err(|error| format!("EPUBのOPFファイルがUTF-8ではありません: {error}"))?;
    let candidates = epub_cover_candidates(package)?;

    let mut last_error = None;
    for candidate in candidates {
        let relative = resolve_epub_href(&package_path, &candidate)?;
        match read_zip_member(&mut archive, &relative, MAX_IMAGE_ENTRY_BYTES) {
            Ok(bytes) => match validate_image(&bytes) {
                Ok(()) => return Ok(bytes),
                Err(error) => last_error = Some(error),
            },
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error
        .unwrap_or_else(|| "EPUBのメタデータにカバー画像が指定されていません。".to_string()))
}

fn extract_azw3_cover(archive_path: &Path) -> Result<Vec<u8>, String> {
    let metadata = mobi::headers::MobiMetadata::from_path(archive_path)
        .map_err(|error| format!("AZW3のメタデータを読み込めませんでした: {error}"))?;
    let cover_index = metadata
        .exth
        .get_record_position(201)
        .and_then(|values| values.first())
        .and_then(|bytes| <[u8; 4]>::try_from(bytes.as_slice()).ok())
        .map(u32::from_be_bytes)
        .and_then(|offset| (metadata.mobi.first_image_index as usize).checked_add(offset as usize));
    let index = cover_index
        .ok_or_else(|| "AZW3のメタデータにカバー画像の指定がありません。".to_string())?;
    let record = metadata
        .records
        .records
        .get(index)
        .ok_or_else(|| "AZW3のカバー画像レコードが見つかりません。".to_string())?;
    let file_length = std::fs::metadata(archive_path)
        .map_err(|error| format!("AZW3のファイル情報を取得できませんでした: {error}"))?
        .len();
    let start = u64::from(record.offset);
    let end = metadata
        .records
        .records
        .get(index + 1)
        .map(|next| u64::from(next.offset))
        .unwrap_or(file_length);
    let end = if index + 1 < metadata.records.records.len() {
        end.saturating_sub(u64::from(metadata.records.extra_bytes()))
    } else {
        end
    };
    let size = end.saturating_sub(start);
    if size == 0 || size > MAX_IMAGE_ENTRY_BYTES {
        return Err("AZW3のカバー画像が空、または大きすぎます。".to_string());
    }

    let mut file =
        File::open(archive_path).map_err(|error| format!("AZW3を開けませんでした: {error}"))?;
    file.seek(SeekFrom::Start(start))
        .map_err(|error| format!("AZW3のカバー画像へ移動できませんでした: {error}"))?;
    let mut bytes = vec![0; size as usize];
    file.read_exact(&mut bytes)
        .map_err(|error| format!("AZW3のカバー画像を読み込めませんでした: {error}"))?;
    validate_image(&bytes)?;
    Ok(bytes)
}

// 表紙の選択肢として、アーカイブ内の画像エントリー名を返す（展開はしない）。
pub fn list_images(archive_path: &Path, format: &str) -> Result<Vec<String>, String> {
    let mut names = match format {
        "zip" | "cbz" => {
            let file = File::open(archive_path)
                .map_err(|error| format!("ZIPアーカイブを開けませんでした: {error}"))?;
            let archive = ZipArchive::new(file)
                .map_err(|error| format!("ZIPアーカイブが不正です: {error}"))?;
            if archive.len() > MAX_ARCHIVE_ENTRIES {
                return Err(format!(
                    "アーカイブのエントリー数が上限（{MAX_ARCHIVE_ENTRIES}件）を超えています。"
                ));
            }
            archive
                .file_names()
                .filter(|name| !name.ends_with('/') && is_selectable_image(name))
                .map(str::to_string)
                .collect::<Vec<_>>()
        }
        "7z" => {
            let mut reader =
                sevenz_rust::SevenZReader::open(archive_path, sevenz_rust::Password::empty())
                    .map_err(|error| format!("7zアーカイブを開けませんでした: {error}"))?;
            let mut names = Vec::new();
            reader
                .for_each_entries(|entry, contents| {
                    if names.len() >= MAX_ARCHIVE_ENTRIES {
                        return Err(sevenz_rust::Error::other(
                            "エントリー数が上限を超えています",
                        ));
                    }
                    if !entry.is_directory() && is_selectable_image(entry.name()) {
                        names.push(entry.name().to_string());
                    }
                    std::io::copy(contents, &mut std::io::sink())
                        .map_err(sevenz_rust::Error::io)?;
                    Ok(true)
                })
                .map_err(|error| format!("7z内のファイルを読み込めませんでした: {error}"))?;
            names
        }
        "rar" | "cbr" => {
            let mut archive = unrar::Archive::new(archive_path)
                .open_for_processing()
                .map_err(|error| format!("RARアーカイブを開けませんでした: {error}"))?;
            let mut names = Vec::new();
            let mut count = 0_usize;
            while let Some(entry) = archive
                .read_header()
                .map_err(|error| format!("RAR内のファイル一覧を読み込めませんでした: {error}"))?
            {
                count += 1;
                if count > MAX_ARCHIVE_ENTRIES {
                    return Err(format!(
                        "アーカイブのエントリー数が上限（{MAX_ARCHIVE_ENTRIES}件）を超えています。"
                    ));
                }
                let header = entry.entry();
                let name = header.filename.to_string_lossy().into_owned();
                if !header.is_directory() && is_selectable_image(&name) {
                    names.push(name);
                }
                archive = entry
                    .skip()
                    .map_err(|error| format!("RAR内のファイルを読み飛ばせませんでした: {error}"))?;
            }
            names
        }
        _ => return Err(unsupported(format)),
    };
    names.sort_by_key(|name| name.to_lowercase());
    Ok(names)
}

// 名前を指定してエントリー1件を読み出す。サイズ上限と画像検証は自動抽出と同じ。
pub fn read_entry(archive_path: &Path, format: &str, entry_name: &str) -> Result<Vec<u8>, String> {
    if !is_selectable_image(entry_name) {
        return Err("画像として扱えないエントリーです。".to_string());
    }
    let bytes = match format {
        "zip" | "cbz" => {
            let file = File::open(archive_path)
                .map_err(|error| format!("ZIPアーカイブを開けませんでした: {error}"))?;
            let mut archive = ZipArchive::new(file)
                .map_err(|error| format!("ZIPアーカイブが不正です: {error}"))?;
            let entry = archive
                .by_name(entry_name)
                .map_err(|error| format!("指定した画像が見つかりません: {error}"))?;
            if entry.size() > MAX_IMAGE_ENTRY_BYTES {
                return Err("指定した画像が大きすぎます。".to_string());
            }
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry
                .take(MAX_IMAGE_ENTRY_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| format!("ZIP内の画像を展開できませんでした: {error}"))?;
            bytes
        }
        "7z" => {
            let mut reader =
                sevenz_rust::SevenZReader::open(archive_path, sevenz_rust::Password::empty())
                    .map_err(|error| format!("7zアーカイブを開けませんでした: {error}"))?;
            let mut found: Option<Vec<u8>> = None;
            reader
                .for_each_entries(|entry, contents| {
                    if entry.name() == entry_name && !entry.is_directory() {
                        let mut bytes = Vec::new();
                        contents
                            .take(MAX_IMAGE_ENTRY_BYTES + 1)
                            .read_to_end(&mut bytes)
                            .map_err(sevenz_rust::Error::io)?;
                        found = Some(bytes);
                        return Ok(false);
                    }
                    std::io::copy(contents, &mut std::io::sink())
                        .map_err(sevenz_rust::Error::io)?;
                    Ok(true)
                })
                .map_err(|error| format!("7z内のファイルを読み込めませんでした: {error}"))?;
            found.ok_or_else(|| "指定した画像が見つかりません。".to_string())?
        }
        "rar" | "cbr" => {
            let mut archive = unrar::Archive::new(archive_path)
                .open_for_processing()
                .map_err(|error| format!("RARアーカイブを開けませんでした: {error}"))?;
            let mut found = None;
            while let Some(entry) = archive
                .read_header()
                .map_err(|error| format!("RAR内のファイル一覧を読み込めませんでした: {error}"))?
            {
                let header = entry.entry();
                if !header.is_directory()
                    && header.filename.to_string_lossy() == entry_name
                    && header.unpacked_size <= MAX_IMAGE_ENTRY_BYTES
                {
                    let (bytes, _) = entry
                        .read()
                        .map_err(|error| format!("RAR内の画像を展開できませんでした: {error}"))?;
                    found = Some(bytes);
                    break;
                }
                archive = entry
                    .skip()
                    .map_err(|error| format!("RAR内のファイルを読み飛ばせませんでした: {error}"))?;
            }
            found.ok_or_else(|| "指定した画像が見つからないか、大きすぎます。".to_string())?
        }
        _ => return Err(unsupported(format)),
    };
    if bytes.is_empty() || bytes.len() as u64 > MAX_IMAGE_ENTRY_BYTES {
        return Err("指定した画像が空、または大きすぎます。".to_string());
    }
    validate_image(&bytes)?;
    Ok(bytes)
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
        if entry.is_file() && is_supported_image(entry.name()) && entry.size() > 0 {
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
        Some("jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "avif")
    )
}

fn cover_priority(name: &str) -> u8 {
    let name = name.to_lowercase();
    let basename = name.rsplit(['/', '\\']).next().unwrap_or(&name);
    if basename.contains("cover")
        || basename.contains("jacket")
        || basename.contains("front")
        || basename.starts_with("表紙")
    {
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
pub(crate) fn test_pdf_bytes(colors: &[&str]) -> Vec<u8> {
    let page_ids = (0..colors.len())
        .map(|index| 3 + index * 2)
        .collect::<Vec<_>>();
    let kids = page_ids
        .iter()
        .map(|id| format!("{id} 0 R"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", colors.len()),
    ];
    for (index, color) in colors.iter().enumerate() {
        let page_id = page_ids[index];
        let content_id = page_id + 1;
        let content = format!("{color} rg\n0 0 100 150 re\nf\n");
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 150] /Resources << >> /Contents {content_id} 0 R >>"
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ));
    }

    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }

    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", offsets.len()).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );
    pdf
}

#[cfg(test)]
mod tests {
    use super::{
        cover_priority, create_thumbnail, migrate_avif_thumbnails, validate_existing_thumbnail,
        THUMBNAIL_MAX_HEIGHT, THUMBNAIL_MAX_WIDTH,
    };
    use image::{GenericImageView, ImageEncoder, RgbImage};
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
    fn lists_and_reads_selected_cover_entries() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let archive_path = directory.path().join("comic.cbz");
        write_zip(
            &archive_path,
            &[
                ("b/002.png", &png_bytes(10, 20)),
                ("a/001.PNG", &png_bytes(30, 40)),
                ("notes.txt", b"text"),
                ("../evil.png", &png_bytes(5, 5)),
            ],
        );

        assert_eq!(
            super::list_images(&archive_path, "cbz").expect("list"),
            ["a/001.PNG", "b/002.png"]
        );
        assert!(super::read_entry(&archive_path, "cbz", "a/001.PNG").is_ok());
        assert!(super::read_entry(&archive_path, "cbz", "notes.txt").is_err());
        assert!(super::read_entry(&archive_path, "cbz", "../evil.png").is_err());
        assert!(super::read_entry(&archive_path, "cbz", "missing.png").is_err());

        let output = directory.path().join("picked.webp");
        super::create_thumbnail_from_entry(&archive_path, "cbz", "b/002.png", &output)
            .expect("thumbnail from entry");
        assert!(output.metadata().expect("output").len() > 0);
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
    fn uses_epub_cover_metadata_and_resolves_encoded_relative_paths() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let archive_path = directory.path().join("book.epub");
        let thumbnail_path = directory.path().join("cover.webp");
        let image = png_bytes(64, 96);
        write_zip(
            &archive_path,
            &[
                (
                    "META-INF/container.xml",
                    br#"<?xml version="1.0"?><container><rootfiles><rootfile full-path="OPS/book.opf"/></rootfiles></container>"#,
                ),
                (
                    "OPS/book.opf",
                    br#"<package><metadata><meta name="cover" content="main-cover"/></metadata><manifest><item id="main-cover" href="images/cover%20image.png" media-type="image/png"/></manifest></package>"#,
                ),
                ("OPS/images/cover image.png", &image),
            ],
        );

        create_thumbnail(&archive_path, "epub", &thumbnail_path).expect("extract EPUB cover");
        validate_existing_thumbnail(&thumbnail_path).expect("validate EPUB thumbnail");
        assert_eq!(
            super::count_pages(&archive_path, "epub").expect("no page count"),
            None
        );
    }

    #[test]
    fn uses_epub3_cover_image_property() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let archive_path = directory.path().join("book.epub");
        let thumbnail_path = directory.path().join("cover.webp");
        write_zip(
            &archive_path,
            &[
                (
                    "META-INF/container.xml",
                    br#"<container><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#,
                ),
                (
                    "book.opf",
                    br#"<package><manifest><item id="cover" href="cover.png" media-type="image/png" properties="cover-image"/></manifest></package>"#,
                ),
                ("cover.png", &png_bytes(32, 48)),
            ],
        );

        create_thumbnail(&archive_path, "epub", &thumbnail_path).expect("extract EPUB3 cover");
        validate_existing_thumbnail(&thumbnail_path).expect("validate EPUB3 thumbnail");
    }

    #[test]
    fn lists_and_decodes_avif_covers() {
        let image = RgbImage::from_pixel(64, 96, image::Rgb([30, 160, 90]));
        let mut avif = Vec::new();
        image::codecs::avif::AvifEncoder::new_with_speed_quality(&mut avif, 10, 60)
            .write_image(image.as_raw(), 64, 96, image::ExtendedColorType::Rgb8)
            .expect("encode avif");

        let directory = tempfile::tempdir().expect("temporary directory");
        let archive_path = directory.path().join("comic.cbz");
        write_zip(
            &archive_path,
            &[("001.webp", &png_bytes(10, 10)), ("002.avif", &avif)],
        );

        assert_eq!(
            super::list_images(&archive_path, "cbz").expect("list"),
            ["001.webp", "002.avif"]
        );
        let output = directory.path().join("avif.webp");
        super::create_thumbnail_from_entry(&archive_path, "cbz", "002.avif", &output)
            .expect("thumbnail from avif");
        validate_existing_thumbnail(&output).expect("valid WebP");
    }

    #[test]
    fn counts_image_entries_as_pages_and_includes_converted_avif() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let archive_path = directory.path().join("comic.cbz");
        write_zip(
            &archive_path,
            &[
                ("pages/001.png", b"png"),
                ("pages/002.JPG", b"jpg"),
                ("pages/003.avif", b"avif"),
                ("notes.txt", b"text"),
                ("../outside.png", b"unsafe"),
            ],
        );

        assert_eq!(
            super::count_pages(&archive_path, "cbz").expect("count pages"),
            Some(3)
        );
    }

    #[test]
    fn counts_pdf_pages_and_uses_the_first_page_as_cover() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let pdf_path = directory.path().join("comic.pdf");
        let thumbnail_path = directory.path().join("cover.webp");
        std::fs::write(&pdf_path, super::test_pdf_bytes(&["1 0 0", "0 0 1"])).expect("write PDF");

        assert_eq!(
            super::count_pages(&pdf_path, "pdf").expect("count PDF pages"),
            Some(2)
        );
        create_thumbnail(&pdf_path, "pdf", &thumbnail_path).expect("render first page");
        validate_existing_thumbnail(&thumbnail_path).expect("validate WebP thumbnail");
        let image = image::open(thumbnail_path).expect("decode WebP");
        let center = image.get_pixel(image.width() / 2, image.height() / 2);
        assert!(
            center[0] > center[2],
            "expected red first page, got {center:?}"
        );
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
