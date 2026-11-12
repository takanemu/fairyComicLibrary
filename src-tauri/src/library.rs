use rusqlite::{named_params, params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

const TAG_CATEGORIES: [&str; 3] = ["author", "series", "classification"];
const MAX_TAG_NAME_CHARS: usize = 100;
const RECENT_LIMIT: i64 = 50;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TagSummary {
    pub id: i64,
    pub name: String,
    pub category: String,
}

pub fn record_read(repository_root: &Path, archive_id: &str) -> Result<(), String> {
    let connection = open(repository_root)?;
    let changed = connection
        .execute(
            "UPDATE archives
             SET last_read_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 read_count = read_count + 1
             WHERE archive_id = ?1 AND read_count < 9223372036854775807",
            [archive_id],
        )
        .map_err(|error| format!("読書履歴を更新できませんでした: {error}"))?;
    if changed == 0 {
        return Err("対象のアーカイブが見つからないか、読書回数が上限に達しています。".to_string());
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagWithCount {
    pub id: i64,
    pub name: String,
    pub category: String,
    pub archive_count: u64,
    pub group_id: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagGroup {
    pub id: i64,
    pub name: String,
    pub category: String,
    pub parent_id: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveSummary {
    pub archive_id: String,
    pub filename: String,
    pub format: String,
    pub page_count: Option<u64>,
    pub file_size: u64,
    pub imported_at: String,
    pub last_read_at: Option<String>,
    pub read_count: u64,
    pub is_favorite: bool,
    pub has_thumbnail: bool,
    pub tags: Vec<TagSummary>,
}

#[derive(Debug, Default, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SortBy {
    FileSize,
    #[default]
    ImportedAt,
    LastReadAt,
    ReadCount,
    Name,
}

#[derive(Debug, Default, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SortDirection {
    Asc,
    #[default]
    Desc,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ArchiveQuery {
    pub search: Option<String>,
    pub favorites_only: bool,
    pub tag_ids: Vec<i64>,
    pub recent_only: bool,
    pub sort_by: SortBy,
    pub sort_direction: SortDirection,
}

fn open(repository_root: &Path) -> Result<Connection, String> {
    let connection = Connection::open(repository_root.join("database/library.sqlite3"))
        .map_err(|error| format!("SQLiteデータベースを開けませんでした: {error}"))?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|error| format!("SQLite外部キー制約を有効にできませんでした: {error}"))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| format!("SQLiteの待機時間を設定できませんでした: {error}"))?;
    Ok(connection)
}

fn escape_like(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() + 2);
    escaped.push('%');
    for character in text.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    escaped
}

pub fn list_archives(
    repository_root: &Path,
    query: &ArchiveQuery,
) -> Result<Vec<ArchiveSummary>, String> {
    let connection = open(repository_root)?;
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(escape_like);
    let sort_column = match query.sort_by {
        SortBy::FileSize => "a.file_size",
        SortBy::ImportedAt => "a.imported_at",
        SortBy::LastReadAt => "a.last_read_at",
        SortBy::ReadCount => "a.read_count",
        SortBy::Name => "a.original_filename COLLATE NOCASE",
    };
    let direction = match query.sort_direction {
        SortDirection::Asc => "ASC",
        SortDirection::Desc => "DESC",
    };
    let order_by = if query.sort_by == SortBy::LastReadAt {
        format!("(a.last_read_at IS NULL) ASC, {sort_column} {direction}, a.archive_id ASC")
    } else {
        format!("{sort_column} {direction}, a.archive_id ASC")
    };
    let limit = if query.recent_only { RECENT_LIMIT } else { -1 };

    // 同じ分類内は OR、異なる分類の間は AND。数値のみを埋め込むためSQLインジェクションは起きない。
    let tag_filter = if query.tag_ids.is_empty() {
        String::new()
    } else {
        let ids = query
            .tag_ids
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "AND (SELECT COUNT(DISTINCT t.category) FROM archive_tags x
                  JOIN tags t ON t.id = x.tag_id
                  WHERE x.archive_id = a.archive_id AND t.id IN ({ids}))
                = (SELECT COUNT(DISTINCT category) FROM tags WHERE id IN ({ids}))"
        )
    };
    let sql = format!(
        "WITH matching AS (
             SELECT a.* FROM archives a
             WHERE (:favorites = 0 OR a.is_favorite = 1)
               {tag_filter}
               AND (:search IS NULL
                    OR a.original_filename LIKE :search ESCAPE '\\'
                    OR EXISTS (
                        SELECT 1 FROM archive_tags x JOIN tags t ON t.id = x.tag_id
                        WHERE x.archive_id = a.archive_id AND t.name LIKE :search ESCAPE '\\'))
         )
         SELECT a.archive_id, a.original_filename, a.format, a.page_count, a.file_size,
                a.imported_at, a.last_read_at, a.read_count, a.is_favorite,
                a.thumbnail_relative_path IS NOT NULL
         FROM matching a
         WHERE (:recent_only = 0 OR a.archive_id IN (
                SELECT archive_id FROM matching
                ORDER BY imported_at DESC, archive_id ASC LIMIT {RECENT_LIMIT}))
         ORDER BY {order_by}
         LIMIT :limit"
    );
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("アーカイブ一覧の問い合わせを準備できませんでした: {error}"))?;
    let mut archives = statement
        .query_map(
            named_params! {
                ":favorites": query.favorites_only as i64,
                ":recent_only": query.recent_only as i64,
                ":search": search,
                ":limit": limit,
            },
            |row| {
                Ok(ArchiveSummary {
                    archive_id: row.get(0)?,
                    filename: row.get(1)?,
                    format: row.get(2)?,
                    page_count: row
                        .get::<_, Option<i64>>(3)?
                        .map(|count| count.max(0) as u64),
                    file_size: row.get::<_, i64>(4)?.max(0) as u64,
                    imported_at: row.get(5)?,
                    last_read_at: row.get(6)?,
                    read_count: row.get::<_, i64>(7)?.max(0) as u64,
                    is_favorite: row.get::<_, i64>(8)? != 0,
                    has_thumbnail: row.get::<_, i64>(9)? != 0,
                    tags: Vec::new(),
                })
            },
        )
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .map_err(|error| format!("アーカイブ一覧を読み取れませんでした: {error}"))?;

    let mut tag_statement = connection
        .prepare(
            "SELECT x.archive_id, t.id, t.name, t.category
             FROM archive_tags x JOIN tags t ON t.id = x.tag_id
             ORDER BY t.category, t.name COLLATE NOCASE",
        )
        .map_err(|error| format!("タグの問い合わせを準備できませんでした: {error}"))?;
    let mut tags_by_archive: HashMap<String, Vec<TagSummary>> = HashMap::new();
    let tag_rows = tag_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                TagSummary {
                    id: row.get(1)?,
                    name: row.get(2)?,
                    category: row.get(3)?,
                },
            ))
        })
        .map_err(|error| format!("タグを読み取れませんでした: {error}"))?;
    for row in tag_rows {
        let (archive_id, tag) =
            row.map_err(|error| format!("タグを読み取れませんでした: {error}"))?;
        tags_by_archive.entry(archive_id).or_default().push(tag);
    }
    for archive in &mut archives {
        archive.tags = tags_by_archive
            .remove(&archive.archive_id)
            .unwrap_or_default();
    }
    Ok(archives)
}

pub fn backfill_page_counts(repository_root: &Path) -> Result<Vec<String>, String> {
    let connection = open(repository_root)?;
    let mut statement = connection
        .prepare(
            "SELECT archive_id, original_filename, relative_path, format
             FROM archives
             WHERE page_count IS NULL AND format NOT IN ('epub', 'azw3')",
        )
        .map_err(|error| format!("ページ数が未登録のアーカイブを確認できませんでした: {error}"))?;
    let records = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .map_err(|error| format!("ページ数が未登録のアーカイブを読み取れませんでした: {error}"))?;
    drop(statement);

    let mut failures = Vec::new();
    for (archive_id, filename, relative_path, format) in records {
        if !crate::maintenance::is_safe_relative(&relative_path) {
            failures.push(format!("{filename}: 保存先パスが不正です"));
            continue;
        }
        let archive_path = repository_root.join(&relative_path);
        match crate::thumbnails::count_pages(&archive_path, &format) {
            Ok(Some(page_count)) => {
                let page_count = i64::try_from(page_count)
                    .map_err(|_| format!("{filename}: ページ数がDBで扱える範囲を超えています"))?;
                connection
                    .execute(
                        "UPDATE archives SET page_count = ?1
                         WHERE archive_id = ?2 AND page_count IS NULL",
                        params![page_count, archive_id],
                    )
                    .map_err(|error| {
                        format!("{filename}: ページ数を保存できませんでした: {error}")
                    })?;
            }
            Ok(None) => {}
            Err(error) => failures.push(format!("{filename}: {error}")),
        }
    }

    if failures.is_empty() {
        return Ok(Vec::new());
    }
    let samples = failures
        .iter()
        .take(5)
        .cloned()
        .collect::<Vec<_>>()
        .join("、");
    let more = if failures.len() > 5 { " ほか" } else { "" };
    Ok(vec![format!(
        "ページ数を取得できないアーカイブが{}件あります（{samples}{more}）。アーカイブの状態を確認してください。",
        failures.len()
    )])
}

pub fn list_tags(repository_root: &Path) -> Result<Vec<TagWithCount>, String> {
    let connection = open(repository_root)?;
    let mut statement = connection
        .prepare(
            "SELECT t.id, t.name, t.category, COUNT(x.archive_id), t.group_id
             FROM tags t LEFT JOIN archive_tags x ON x.tag_id = t.id
             GROUP BY t.id
             ORDER BY t.category, t.name COLLATE NOCASE",
        )
        .map_err(|error| format!("タグ一覧の問い合わせを準備できませんでした: {error}"))?;
    let tags = statement
        .query_map([], |row| {
            Ok(TagWithCount {
                id: row.get(0)?,
                name: row.get(1)?,
                category: row.get(2)?,
                archive_count: row.get::<_, i64>(3)?.max(0) as u64,
                group_id: row.get(4)?,
            })
        })
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .map_err(|error| format!("タグ一覧を読み取れませんでした: {error}"))?;
    Ok(tags)
}

pub fn list_tag_groups(repository_root: &Path) -> Result<Vec<TagGroup>, String> {
    let connection = open(repository_root)?;
    let mut statement = connection
        .prepare(
            "SELECT id, name, category, parent_id FROM tag_groups
             ORDER BY category, name COLLATE NOCASE, id",
        )
        .map_err(|error| format!("タググループ一覧の問い合わせを準備できませんでした: {error}"))?;
    statement
        .query_map([], |row| {
            Ok(TagGroup {
                id: row.get(0)?,
                name: row.get(1)?,
                category: row.get(2)?,
                parent_id: row.get(3)?,
            })
        })
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .map_err(|error| format!("タググループ一覧を読み取れませんでした: {error}"))
}

fn validate_group_parent(
    connection: &Connection,
    category: &str,
    parent_id: Option<i64>,
) -> Result<(), String> {
    if let Some(parent_id) = parent_id {
        let parent_category = connection
            .query_row(
                "SELECT category FROM tag_groups WHERE id = ?1",
                [parent_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("親グループを確認できませんでした: {error}"))?
            .ok_or_else(|| "移動先のグループが見つかりません。".to_string())?;
        if parent_category != category {
            return Err("異なる分類のグループ間では移動できません。".to_string());
        }
    }
    Ok(())
}

fn validate_group_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("グループ名を入力してください。".to_string());
    }
    if name.chars().count() > MAX_TAG_NAME_CHARS {
        return Err(format!(
            "グループ名は{MAX_TAG_NAME_CHARS}文字以内にしてください。"
        ));
    }
    Ok(name.to_string())
}

pub fn create_tag_group(
    repository_root: &Path,
    category: &str,
    name: &str,
    parent_id: Option<i64>,
) -> Result<TagGroup, String> {
    if !TAG_CATEGORIES.contains(&category) {
        return Err("タグの分類が不正です。".to_string());
    }
    let name = validate_group_name(name)?;
    let mut connection = open(repository_root)?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("タググループの作成を開始できませんでした: {error}"))?;
    validate_group_parent(&transaction, category, parent_id)?;
    transaction
        .execute(
            "INSERT INTO tag_groups (name, category, parent_id) VALUES (?1, ?2, ?3)",
            params![name, category, parent_id],
        )
        .map_err(|error| format!("タググループを作成できませんでした: {error}"))?;
    let id = transaction.last_insert_rowid();
    transaction
        .commit()
        .map_err(|error| format!("タググループの作成を確定できませんでした: {error}"))?;
    Ok(TagGroup {
        id,
        name,
        category: category.to_string(),
        parent_id,
    })
}

pub fn rename_tag_group(repository_root: &Path, group_id: i64, name: &str) -> Result<(), String> {
    let name = validate_group_name(name)?;
    let connection = open(repository_root)?;
    let changed = connection
        .execute(
            "UPDATE tag_groups SET name = ?1 WHERE id = ?2",
            params![name, group_id],
        )
        .map_err(|error| format!("タググループ名を変更できませんでした: {error}"))?;
    if changed == 0 {
        return Err("タググループが見つかりません。".to_string());
    }
    Ok(())
}

pub fn delete_tag_group(repository_root: &Path, group_id: i64) -> Result<(), String> {
    let connection = open(repository_root)?;
    let occupied: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tag_groups WHERE parent_id = ?1)
                OR EXISTS(SELECT 1 FROM tags WHERE group_id = ?1)",
            [group_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("タググループの内容を確認できませんでした: {error}"))?;
    if occupied {
        return Err(
            "タグまたはサブグループがあるグループは削除できません。先に移動してください。"
                .to_string(),
        );
    }
    let changed = connection
        .execute("DELETE FROM tag_groups WHERE id = ?1", [group_id])
        .map_err(|error| format!("タググループを削除できませんでした: {error}"))?;
    if changed == 0 {
        return Err("タググループが見つかりません。".to_string());
    }
    Ok(())
}

pub fn move_tag(repository_root: &Path, tag_id: i64, parent_id: Option<i64>) -> Result<(), String> {
    let connection = open(repository_root)?;
    let category = connection
        .query_row("SELECT category FROM tags WHERE id = ?1", [tag_id], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(|error| format!("タグを確認できませんでした: {error}"))?
        .ok_or_else(|| "移動するタグが見つかりません。".to_string())?;
    validate_group_parent(&connection, &category, parent_id)?;
    connection
        .execute(
            "UPDATE tags SET group_id = ?1 WHERE id = ?2",
            params![parent_id, tag_id],
        )
        .map_err(|error| format!("タグを移動できませんでした: {error}"))?;
    Ok(())
}

pub fn move_tag_group(
    repository_root: &Path,
    group_id: i64,
    parent_id: Option<i64>,
) -> Result<(), String> {
    let connection = open(repository_root)?;
    let category = connection
        .query_row(
            "SELECT category FROM tag_groups WHERE id = ?1",
            [group_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("タググループを確認できませんでした: {error}"))?
        .ok_or_else(|| "移動するタググループが見つかりません。".to_string())?;
    validate_group_parent(&connection, &category, parent_id)?;
    if let Some(parent_id) = parent_id {
        let is_descendant: bool = connection
            .query_row(
                "WITH RECURSIVE descendants(id) AS (
                    SELECT id FROM tag_groups WHERE id = ?1
                    UNION
                    SELECT child.id FROM tag_groups child
                    JOIN descendants parent ON child.parent_id = parent.id
                 )
                 SELECT EXISTS(SELECT 1 FROM descendants WHERE id = ?2)",
                params![group_id, parent_id],
                |row| row.get(0),
            )
            .map_err(|error| format!("移動先の階層を確認できませんでした: {error}"))?;
        if is_descendant {
            return Err("グループを自分自身または配下へ移動することはできません。".to_string());
        }
    }
    connection
        .execute(
            "UPDATE tag_groups SET parent_id = ?1 WHERE id = ?2",
            params![parent_id, group_id],
        )
        .map_err(|error| format!("タググループを移動できませんでした: {error}"))?;
    Ok(())
}

pub fn set_favorite(
    repository_root: &Path,
    archive_id: &str,
    is_favorite: bool,
) -> Result<(), String> {
    let connection = open(repository_root)?;
    let changed = connection
        .execute(
            "UPDATE archives SET is_favorite = ?1 WHERE archive_id = ?2",
            params![is_favorite as i64, archive_id],
        )
        .map_err(|error| format!("お気に入りを更新できませんでした: {error}"))?;
    if changed == 0 {
        return Err("対象のアーカイブが見つかりません。".to_string());
    }
    Ok(())
}

pub fn add_tag(
    repository_root: &Path,
    archive_id: &str,
    category: &str,
    name: &str,
) -> Result<TagSummary, String> {
    add_tag_to_archives(repository_root, &[archive_id.to_string()], category, name)
}

pub fn add_tag_to_archives(
    repository_root: &Path,
    archive_ids: &[String],
    category: &str,
    name: &str,
) -> Result<TagSummary, String> {
    if !TAG_CATEGORIES.contains(&category) {
        return Err("タグの分類が不正です。".to_string());
    }
    let name = name.trim();
    if name.is_empty() {
        return Err("タグ名を入力してください。".to_string());
    }
    if name.chars().count() > MAX_TAG_NAME_CHARS {
        return Err(format!(
            "タグ名は{MAX_TAG_NAME_CHARS}文字以内にしてください。"
        ));
    }
    let archive_ids = archive_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if archive_ids.is_empty() {
        return Err("タグを付ける作品が選択されていません。".to_string());
    }

    let mut connection = open(repository_root)?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("タグの更新を開始できませんでした: {error}"))?;
    {
        let mut statement = transaction
            .prepare_cached("SELECT 1 FROM archives WHERE archive_id = ?1")
            .map_err(|error| format!("対象のアーカイブを確認できませんでした: {error}"))?;
        for archive_id in &archive_ids {
            let exists = statement
                .query_row([archive_id], |_| Ok(()))
                .optional()
                .map_err(|error| format!("対象のアーカイブを確認できませんでした: {error}"))?
                .is_some();
            if !exists {
                return Err("対象のアーカイブが見つかりません。".to_string());
            }
        }
    }
    transaction
        .execute(
            "INSERT OR IGNORE INTO tags (name, category) VALUES (?1, ?2)",
            params![name, category],
        )
        .map_err(|error| format!("タグを作成できませんでした: {error}"))?;
    let id: i64 = transaction
        .query_row(
            "SELECT id FROM tags WHERE name = ?1 AND category = ?2",
            params![name, category],
            |row| row.get(0),
        )
        .map_err(|error| format!("タグを確認できませんでした: {error}"))?;
    {
        let mut statement = transaction
            .prepare_cached(
                "INSERT OR IGNORE INTO archive_tags (archive_id, tag_id) VALUES (?1, ?2)",
            )
            .map_err(|error| format!("タグを付けられませんでした: {error}"))?;
        for archive_id in archive_ids {
            statement
                .execute(params![archive_id, id])
                .map_err(|error| format!("タグを付けられませんでした: {error}"))?;
        }
    }
    transaction
        .commit()
        .map_err(|error| format!("タグの更新を確定できませんでした: {error}"))?;
    Ok(TagSummary {
        id,
        name: name.to_string(),
        category: category.to_string(),
    })
}

// どのアーカイブにも付いていないタグは、フィルター一覧が散らからないよう同時に削除する。
pub fn remove_tag(repository_root: &Path, archive_id: &str, tag_id: i64) -> Result<(), String> {
    remove_tag_from_archives(repository_root, &[archive_id.to_string()], tag_id)
}

pub fn remove_tag_from_archives(
    repository_root: &Path,
    archive_ids: &[String],
    tag_id: i64,
) -> Result<(), String> {
    let archive_ids = archive_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if archive_ids.is_empty() {
        return Err("タグを外す作品が選択されていません。".to_string());
    }

    let mut connection = open(repository_root)?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("タグの更新を開始できませんでした: {error}"))?;
    {
        let mut statement = transaction
            .prepare_cached("DELETE FROM archive_tags WHERE archive_id = ?1 AND tag_id = ?2")
            .map_err(|error| format!("タグを外せませんでした: {error}"))?;
        for archive_id in archive_ids {
            statement
                .execute(params![archive_id, tag_id])
                .map_err(|error| format!("タグを外せませんでした: {error}"))?;
        }
    }
    transaction
        .execute(
            "DELETE FROM tags WHERE id = ?1
             AND NOT EXISTS (SELECT 1 FROM archive_tags WHERE tag_id = ?1)",
            [tag_id],
        )
        .map_err(|error| format!("未使用のタグを整理できませんでした: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("タグの更新を確定できませんでした: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{
        add_tag, add_tag_to_archives, create_tag_group, delete_tag_group, list_archives,
        list_tag_groups, list_tags, move_tag, move_tag_group, remove_tag, remove_tag_from_archives,
        rename_tag_group, set_favorite, ArchiveQuery, SortBy, SortDirection,
    };
    use crate::database;
    use rusqlite::{params, Connection};
    use std::path::Path;

    fn repository_with_archives() -> tempfile::TempDir {
        let repository = tempfile::tempdir().expect("temporary repository");
        std::fs::create_dir_all(repository.path().join("database")).expect("database directory");
        let database_path = repository.path().join("database/library.sqlite3");
        database::initialize(&database_path).expect("initialize database");
        let connection = Connection::open(database_path).expect("open database");
        for (index, (name, imported_at, file_size, last_read_at, read_count)) in [
            (
                "Alpha_100%.cbz",
                "2026-01-01T00:00:00Z",
                100,
                Some("2026-03-01T00:00:00Z"),
                3,
            ),
            ("beta.zip", "2026-03-01T00:00:00Z", 300, None, 0),
            (
                "Gamma.7z",
                "2026-02-01T00:00:00Z",
                200,
                Some("2026-02-01T00:00:00Z"),
                7,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let id = format!("{index:x}").repeat(64);
            connection
                .execute(
                    "INSERT INTO archives (archive_id, original_filename, relative_path, format,
                        file_size, thumbnail_relative_path, imported_at, last_read_at, read_count)
                     VALUES (?1, ?2, ?3, 'cbz', ?4, ?5, ?6, ?7, ?8)",
                    params![
                        id,
                        name,
                        format!("archives/{}/{name}", &id[..2]),
                        file_size,
                        format!("thumbnails/{}/{id}.webp", &id[..2]),
                        imported_at,
                        last_read_at,
                        read_count
                    ],
                )
                .expect("insert archive");
        }
        repository
    }

    #[test]
    fn backfills_page_counts_for_existing_archives() {
        use std::io::Write;
        use zip::{write::SimpleFileOptions, ZipWriter};

        let repository = repository_with_archives();
        let id = id(0);
        let archive_directory = repository.path().join("archives/00");
        std::fs::create_dir_all(&archive_directory).expect("archive directory");
        let mut zip = ZipWriter::new(
            std::fs::File::create(archive_directory.join("Alpha_100%.cbz")).expect("archive"),
        );
        for name in ["001.png", "002.jpg", "notes.txt"] {
            zip.start_file(name, SimpleFileOptions::default())
                .expect("start entry");
            zip.write_all(b"file").expect("write entry");
        }
        zip.finish().expect("finish archive");

        Connection::open(repository.path().join("database/library.sqlite3"))
            .expect("open database")
            .execute(
                "UPDATE archives SET page_count = 0 WHERE archive_id != ?1",
                [&id],
            )
            .expect("mark other page counts as known");
        assert!(super::backfill_page_counts(repository.path())
            .expect("backfill")
            .is_empty());
        let archive = list_archives(repository.path(), &ArchiveQuery::default())
            .expect("list")
            .into_iter()
            .find(|archive| archive.archive_id == id)
            .expect("archive summary");
        assert_eq!(archive.page_count, Some(2));
    }

    fn id(index: usize) -> String {
        format!("{index:x}").repeat(64)
    }

    fn names(root: &Path, query: &ArchiveQuery) -> Vec<String> {
        list_archives(root, query)
            .expect("list archives")
            .into_iter()
            .map(|archive| archive.filename)
            .collect()
    }

    #[test]
    fn sorts_archives_by_selected_field_and_direction() {
        let repository = repository_with_archives();
        assert_eq!(
            names(repository.path(), &ArchiveQuery::default()),
            ["beta.zip", "Gamma.7z", "Alpha_100%.cbz"]
        );
        let query = |sort_by, sort_direction| ArchiveQuery {
            sort_by,
            sort_direction,
            ..Default::default()
        };
        assert_eq!(
            names(
                repository.path(),
                &query(SortBy::FileSize, SortDirection::Asc)
            ),
            ["Alpha_100%.cbz", "Gamma.7z", "beta.zip"]
        );
        assert_eq!(
            names(
                repository.path(),
                &query(SortBy::FileSize, SortDirection::Desc)
            ),
            ["beta.zip", "Gamma.7z", "Alpha_100%.cbz"]
        );
        assert_eq!(
            names(
                repository.path(),
                &query(SortBy::ImportedAt, SortDirection::Asc)
            ),
            ["Alpha_100%.cbz", "Gamma.7z", "beta.zip"]
        );
        assert_eq!(
            names(
                repository.path(),
                &query(SortBy::LastReadAt, SortDirection::Desc)
            ),
            ["Alpha_100%.cbz", "Gamma.7z", "beta.zip"]
        );
        assert_eq!(
            names(
                repository.path(),
                &query(SortBy::LastReadAt, SortDirection::Asc)
            ),
            ["Gamma.7z", "Alpha_100%.cbz", "beta.zip"]
        );
        assert_eq!(
            names(
                repository.path(),
                &query(SortBy::ReadCount, SortDirection::Asc)
            ),
            ["beta.zip", "Alpha_100%.cbz", "Gamma.7z"]
        );
        assert_eq!(
            names(
                repository.path(),
                &query(SortBy::ReadCount, SortDirection::Desc)
            ),
            ["Gamma.7z", "Alpha_100%.cbz", "beta.zip"]
        );
        assert_eq!(
            names(repository.path(), &query(SortBy::Name, SortDirection::Desc)),
            ["Gamma.7z", "beta.zip", "Alpha_100%.cbz"]
        );
    }

    #[test]
    fn records_successful_reads_for_sorting() {
        let repository = repository_with_archives();
        super::record_read(repository.path(), &id(1)).expect("record read");
        let archive = list_archives(
            repository.path(),
            &ArchiveQuery {
                sort_by: SortBy::LastReadAt,
                ..Default::default()
            },
        )
        .expect("list")
        .into_iter()
        .find(|archive| archive.archive_id == id(1))
        .expect("read archive");
        assert_eq!(archive.read_count, 1);
        assert!(archive.last_read_at.is_some());
    }

    #[test]
    fn filters_by_favorite_and_toggles_it() {
        let repository = repository_with_archives();
        set_favorite(repository.path(), &id(1), true).expect("favorite");
        let favorites = ArchiveQuery {
            favorites_only: true,
            ..Default::default()
        };
        assert_eq!(names(repository.path(), &favorites), ["beta.zip"]);
        set_favorite(repository.path(), &id(1), false).expect("unfavorite");
        assert!(names(repository.path(), &favorites).is_empty());
        assert!(set_favorite(repository.path(), &"f".repeat(64), true).is_err());
    }

    #[test]
    fn searches_filenames_literally_and_by_tag_name() {
        let repository = repository_with_archives();
        let search = |text: &str| {
            names(
                repository.path(),
                &ArchiveQuery {
                    search: Some(text.to_string()),
                    ..Default::default()
                },
            )
        };
        assert_eq!(search("100%"), ["Alpha_100%.cbz"]);
        assert!(search("%").len() == 1, "the percent sign is not a wildcard");
        assert!(search("_").len() == 1, "the underscore is not a wildcard");
        add_tag(repository.path(), &id(2), "author", "山田太郎").expect("tag");
        assert_eq!(search("山田"), ["Gamma.7z"]);
    }

    #[test]
    fn adds_filters_and_removes_tags_and_cleans_up_unused_ones() {
        let repository = repository_with_archives();
        let tag = add_tag(repository.path(), &id(0), "series", "  Fairy  ").expect("add tag");
        assert_eq!(tag.name, "Fairy");
        let same = add_tag(repository.path(), &id(1), "series", "Fairy").expect("reuse tag");
        assert_eq!(same.id, tag.id);
        add_tag(repository.path(), &id(1), "series", "Fairy").expect("duplicate is ignored");

        let tags = list_tags(repository.path()).expect("list tags");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].archive_count, 2);

        let filtered = ArchiveQuery {
            tag_ids: vec![tag.id],
            ..Default::default()
        };
        assert_eq!(
            names(repository.path(), &filtered),
            ["beta.zip", "Alpha_100%.cbz"]
        );

        remove_tag(repository.path(), &id(0), tag.id).expect("remove from first");
        assert_eq!(list_tags(repository.path()).expect("tags").len(), 1);
        remove_tag(repository.path(), &id(1), tag.id).expect("remove from second");
        assert!(list_tags(repository.path()).expect("tags").is_empty());
    }

    #[test]
    fn adds_and_removes_a_tag_for_multiple_archives_in_one_operation() {
        let repository = repository_with_archives();
        let ids = vec![id(0), id(1), id(0)];
        let tag = add_tag_to_archives(repository.path(), &ids, "series", "Fairy").expect("add tag");

        let tags = list_tags(repository.path()).expect("list tags");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].id, tag.id);
        assert_eq!(tags[0].archive_count, 2);

        remove_tag_from_archives(repository.path(), &ids, tag.id).expect("remove tag");
        assert!(list_tags(repository.path()).expect("list tags").is_empty());
    }

    #[test]
    fn groups_tags_without_changing_tag_identity_or_unused_tag_cleanup() {
        let repository = repository_with_archives();
        let root = repository.path();
        let parent = create_tag_group(root, "author", "Regions", None).expect("create group");
        let child =
            create_tag_group(root, "author", "Japan", Some(parent.id)).expect("create subgroup");
        let first = id(0);
        let second = id(1);
        let tag = add_tag(root, &first, "author", "Shared author").expect("add tag");
        let same_tag =
            add_tag(root, &second, "author", "Shared author").expect("reuse same-category tag");
        assert_eq!(tag.id, same_tag.id);

        move_tag(root, tag.id, Some(child.id)).expect("move tag into group");
        assert_eq!(
            list_tags(root).expect("list tags")[0].group_id,
            Some(child.id)
        );
        assert!(
            move_tag_group(root, parent.id, Some(child.id)).is_err(),
            "a group cannot be moved into its descendant"
        );
        rename_tag_group(root, child.id, "Domestic").expect("rename group");
        assert_eq!(
            list_tag_groups(root)
                .expect("list groups")
                .into_iter()
                .find(|group| group.id == child.id)
                .expect("renamed group")
                .name,
            "Domestic"
        );

        remove_tag_from_archives(root, &[first, second], tag.id).expect("remove tag links");
        assert!(list_tags(root).expect("list tags after cleanup").is_empty());
        assert_eq!(
            list_tag_groups(root)
                .expect("groups remain after tag cleanup")
                .len(),
            2
        );
        delete_tag_group(root, child.id).expect("delete empty subgroup");
        delete_tag_group(root, parent.id).expect("delete empty parent");
        assert!(list_tag_groups(root)
            .expect("list deleted groups")
            .is_empty());
    }

    #[test]
    fn adding_a_tag_to_multiple_archives_rolls_back_if_any_archive_is_missing() {
        let repository = repository_with_archives();
        let ids = vec![id(0), "f".repeat(64)];

        assert!(add_tag_to_archives(repository.path(), &ids, "series", "Fairy").is_err());
        assert!(list_tags(repository.path()).expect("list tags").is_empty());
    }

    #[test]
    fn combines_tags_with_or_within_category_and_and_across_categories() {
        let repository = repository_with_archives();
        let root = repository.path();
        let a1 = add_tag(root, &id(0), "author", "A1").unwrap();
        let c1 = add_tag(root, &id(0), "classification", "C1").unwrap();
        let a2 = add_tag(root, &id(1), "author", "A2").unwrap();
        add_tag(root, &id(1), "classification", "C2").unwrap();
        let query = |ids: Vec<i64>| ArchiveQuery {
            tag_ids: ids,
            ..Default::default()
        };
        assert_eq!(names(root, &query(vec![a1.id, a2.id])).len(), 2);
        assert_eq!(names(root, &query(vec![a1.id, c1.id])), ["Alpha_100%.cbz"]);
        assert!(names(root, &query(vec![a2.id, c1.id])).is_empty());
    }

    #[test]
    fn rejects_invalid_tags() {
        let repository = repository_with_archives();
        assert!(add_tag(repository.path(), &id(0), "unknown", "x").is_err());
        assert!(add_tag(repository.path(), &id(0), "author", "   ").is_err());
        assert!(add_tag(repository.path(), &id(0), "author", &"あ".repeat(101)).is_err());
        assert!(add_tag(repository.path(), &"f".repeat(64), "author", "x").is_err());
    }
}
