use rusqlite::{
    named_params, params, Connection, OptionalExtension, Transaction, TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
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
    pub revision: i64,
}

pub fn record_read(repository_root: &Path, archive_id: &str) -> Result<(), String> {
    let mut connection = open(repository_root)?;
    let transaction = begin_immediate(&mut connection, "読書履歴の更新")?;
    let changed = transaction
        .execute(
            "UPDATE archives
             SET last_read_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 read_count = read_count + 1,
                 revision = revision + 1
             WHERE archive_id = ?1 AND read_count < 9223372036854775807",
            [archive_id],
        )
        .map_err(|error| format!("読書履歴を更新できませんでした: {error}"))?;
    if changed == 0 {
        return Err("対象のアーカイブが見つからないか、読書回数が上限に達しています。".to_string());
    }
    crate::database::record_change(
        &transaction,
        "archive",
        archive_id,
        "read",
        crate::database::ChangeSource::Gui,
        "読書履歴を更新",
    )?;
    transaction
        .commit()
        .map_err(|error| format!("読書履歴の更新を確定できませんでした: {error}"))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagWithCount {
    pub id: i64,
    pub name: String,
    pub category: String,
    pub archive_count: u64,
    pub group_id: Option<i64>,
    pub revision: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagGroup {
    pub id: i64,
    pub name: String,
    pub category: String,
    pub parent_id: Option<i64>,
    pub revision: i64,
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
    pub revision: i64,
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

fn group_tags_by_category(
    connection: &Connection,
    tag_ids: &[i64],
) -> Result<HashMap<String, Vec<i64>>, String> {
    if tag_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let ids_str = tag_ids
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut statement = connection
        .prepare(&format!(
            "SELECT category, id FROM tags WHERE id IN ({}) ORDER BY category",
            ids_str
        ))
        .map_err(|error| format!("タグカテゴリの取得に失敗しました: {error}"))?;

    let mut categories: HashMap<String, Vec<i64>> = HashMap::new();
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| format!("タグを読み取れませんでした: {error}"))?;

    for row in rows {
        let (category, tag_id) =
            row.map_err(|error| format!("タグを読み取れませんでした: {error}"))?;
        categories.entry(category).or_default().push(tag_id);
    }

    Ok(categories)
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
        .filter(|text| !text.is_empty());
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

    // タグをカテゴリごとにグループ化し、各カテゴリについて EXISTS 条件を生成
    let tag_filter = if query.tag_ids.is_empty() {
        String::new()
    } else {
        let tag_by_category = group_tags_by_category(&connection, &query.tag_ids)?;
        let mut conditions = Vec::new();

        for (category, ids) in tag_by_category.iter() {
            let ids_str = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
            conditions.push(format!(
                "EXISTS(SELECT 1 FROM archive_tags x JOIN tags t ON t.id = x.tag_id
                        WHERE x.archive_id = a.archive_id AND t.category = '{category}' AND t.id IN ({ids_str}))"
            ));
        }

        format!("AND {}", conditions.join(" AND "))
    };

    let (sql, search_param) = if let Some(search_term) = search {
        let search_condition = "AND (
            a.original_filename LIKE :search ESCAPE '\\'
            OR EXISTS (
                SELECT 1 FROM archive_tags x
                JOIN tags t ON t.id = x.tag_id
                WHERE x.archive_id = a.archive_id AND t.name LIKE :search ESCAPE '\\'
            )
        )";
        let sql = if query.recent_only {
            format!(
                "SELECT a.archive_id, a.original_filename, a.format, a.page_count, a.file_size,
                        a.imported_at, a.last_read_at, a.read_count, a.is_favorite,
                        a.thumbnail_relative_path IS NOT NULL, a.revision
                 FROM archives a
                 WHERE (:favorites = 0 OR a.is_favorite = 1)
                   {tag_filter}
                   {search_condition}
                 ORDER BY a.imported_at DESC, a.archive_id ASC
                 LIMIT {RECENT_LIMIT}"
            )
        } else {
            format!(
                "SELECT a.archive_id, a.original_filename, a.format, a.page_count, a.file_size,
                        a.imported_at, a.last_read_at, a.read_count, a.is_favorite,
                        a.thumbnail_relative_path IS NOT NULL, a.revision
                 FROM archives a
                 WHERE (:favorites = 0 OR a.is_favorite = 1)
                   {tag_filter}
                   {search_condition}
                 ORDER BY {order_by}"
            )
        };
        (sql, Some(escape_like(search_term)))
    } else {
        let sql = if query.recent_only {
            format!(
                "SELECT a.archive_id, a.original_filename, a.format, a.page_count, a.file_size,
                        a.imported_at, a.last_read_at, a.read_count, a.is_favorite,
                        a.thumbnail_relative_path IS NOT NULL, a.revision
                 FROM archives a
                 WHERE (:favorites = 0 OR a.is_favorite = 1)
                   {tag_filter}
                 ORDER BY a.imported_at DESC, a.archive_id ASC
                 LIMIT {RECENT_LIMIT}"
            )
        } else {
            format!(
                "SELECT a.archive_id, a.original_filename, a.format, a.page_count, a.file_size,
                        a.imported_at, a.last_read_at, a.read_count, a.is_favorite,
                        a.thumbnail_relative_path IS NOT NULL, a.revision
                 FROM archives a
                 WHERE (:favorites = 0 OR a.is_favorite = 1)
                   {tag_filter}
                 ORDER BY {order_by}
                 "
            )
        };
        (sql, None)
    };

    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("アーカイブ一覧の問い合わせを準備できませんでした: {error}"))?;

    let mut archives = if let Some(search_param) = search_param {
        statement
            .query_map(
                named_params! {
                    ":favorites": query.favorites_only as i64,
                    ":search": search_param,
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
                        revision: row.get(10)?,
                        tags: Vec::new(),
                    })
                },
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
    } else {
        statement
            .query_map(
                named_params! {
                    ":favorites": query.favorites_only as i64,
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
                        revision: row.get(10)?,
                        tags: Vec::new(),
                    })
                },
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
    }
    .map_err(|error| format!("アーカイブ一覧を読み取れませんでした: {error}"))?;

    if !archives.is_empty() {
        let archive_ids = archives
            .iter()
            .map(|a| a.archive_id.clone())
            .collect::<Vec<_>>();
        let ids_param = archive_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let tag_sql = format!(
            "SELECT x.archive_id, t.id, t.name, t.category, t.revision
             FROM archive_tags x JOIN tags t ON t.id = x.tag_id
             WHERE x.archive_id IN ({})",
            ids_param
        );
        let mut tag_statement = connection
            .prepare(&tag_sql)
            .map_err(|error| format!("タグの問い合わせを準備できませんでした: {error}"))?;
        let mut tags_by_archive: HashMap<String, Vec<TagSummary>> = HashMap::new();
        let tag_rows = tag_statement
            .query_map(rusqlite::params_from_iter(archive_ids.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    TagSummary {
                        id: row.get(1)?,
                        name: row.get(2)?,
                        category: row.get(3)?,
                        revision: row.get(4)?,
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
            if let Some(mut tags) = tags_by_archive.remove(&archive.archive_id) {
                tags.sort_by(|a, b| {
                    a.category
                        .cmp(&b.category)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
                archive.tags = tags;
            }
        }
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
            "SELECT t.id, t.name, t.category, COUNT(x.archive_id), t.group_id, t.revision
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
                revision: row.get(5)?,
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
            "SELECT id, name, category, parent_id, revision FROM tag_groups
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
                revision: row.get(4)?,
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
    create_tag_group_with_source(
        repository_root,
        category,
        name,
        parent_id,
        crate::database::ChangeSource::Gui,
    )
}

pub fn create_tag_group_with_source(
    repository_root: &Path,
    category: &str,
    name: &str,
    parent_id: Option<i64>,
    source: crate::database::ChangeSource,
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
    crate::database::record_change(
        &transaction,
        "tag_group",
        &id.to_string(),
        "created",
        source,
        &format!("タググループ「{name}」を作成"),
    )?;
    transaction
        .commit()
        .map_err(|error| format!("タググループの作成を確定できませんでした: {error}"))?;
    Ok(TagGroup {
        id,
        name,
        category: category.to_string(),
        parent_id,
        revision: 0,
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
        revision: 0,
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

fn begin_immediate<'a>(
    connection: &'a mut Connection,
    operation: &str,
) -> Result<Transaction<'a>, String> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("{operation}を開始できませんでした: {error}"))
}

fn conflict(entity: &str) -> String {
    format!(
        "{entity}は他のユーザーまたはAIによって変更されています。最新の状態を読み直してください。"
    )
}

fn bump_rows(
    transaction: &Transaction<'_>,
    table: &str,
    id_column: &str,
    entity_ids: &[String],
    expected_revisions: &BTreeMap<String, i64>,
    entity: &str,
) -> Result<(), String> {
    for entity_id in entity_ids {
        let expected = expected_revisions
            .get(entity_id)
            .ok_or_else(|| format!("{entity}の期待リビジョンが指定されていません。"))?;
        let sql = format!(
            "UPDATE {table} SET revision = revision + 1
             WHERE {id_column} = ?1 AND revision = ?2"
        );
        let changed = transaction
            .execute(&sql, params![entity_id, expected])
            .map_err(|error| format!("{entity}のリビジョンを更新できませんでした: {error}"))?;
        if changed != 1 {
            return Err(conflict(entity));
        }
    }
    Ok(())
}

pub fn set_favorite_with_revision(
    repository_root: &Path,
    archive_id: &str,
    is_favorite: bool,
    expected_revision: i64,
    source: crate::database::ChangeSource,
) -> Result<(), String> {
    let mut connection = open(repository_root)?;
    let transaction = begin_immediate(&mut connection, "お気に入りの更新")?;
    let changed = transaction
        .execute(
            "UPDATE archives SET is_favorite = ?1, revision = revision + 1
             WHERE archive_id = ?2 AND revision = ?3",
            params![is_favorite as i64, archive_id, expected_revision],
        )
        .map_err(|error| format!("お気に入りを更新できませんでした: {error}"))?;
    if changed != 1 {
        return Err(conflict("作品"));
    }
    crate::database::record_change(
        &transaction,
        "archive",
        archive_id,
        "favorite_changed",
        source,
        if is_favorite {
            "お気に入りに追加"
        } else {
            "お気に入りから削除"
        },
    )?;
    transaction
        .commit()
        .map_err(|error| format!("お気に入りの更新を確定できませんでした: {error}"))
}

pub fn rename_tag_group_with_revision(
    repository_root: &Path,
    group_id: i64,
    name: &str,
    expected_revision: i64,
    source: crate::database::ChangeSource,
) -> Result<(), String> {
    let name = validate_group_name(name)?;
    let mut connection = open(repository_root)?;
    let transaction = begin_immediate(&mut connection, "タググループ名の変更")?;
    let changed = transaction
        .execute(
            "UPDATE tag_groups SET name = ?1, revision = revision + 1
             WHERE id = ?2 AND revision = ?3",
            params![name, group_id, expected_revision],
        )
        .map_err(|error| format!("タググループ名を変更できませんでした: {error}"))?;
    if changed != 1 {
        return Err(conflict("タググループ"));
    }
    crate::database::record_change(
        &transaction,
        "tag_group",
        &group_id.to_string(),
        "renamed",
        source,
        &format!("グループ名を「{name}」へ変更"),
    )?;
    transaction
        .commit()
        .map_err(|error| format!("タググループ名の変更を確定できませんでした: {error}"))
}

pub fn delete_tag_group_with_revision(
    repository_root: &Path,
    group_id: i64,
    expected_revision: i64,
    source: crate::database::ChangeSource,
) -> Result<(), String> {
    let mut connection = open(repository_root)?;
    let transaction = begin_immediate(&mut connection, "タググループの削除")?;
    let occupied: bool = transaction
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
    let changed = transaction
        .execute(
            "DELETE FROM tag_groups WHERE id = ?1 AND revision = ?2",
            params![group_id, expected_revision],
        )
        .map_err(|error| format!("タググループを削除できませんでした: {error}"))?;
    if changed != 1 {
        return Err(conflict("タググループ"));
    }
    crate::database::record_change(
        &transaction,
        "tag_group",
        &group_id.to_string(),
        "deleted",
        source,
        "タググループを削除",
    )?;
    transaction
        .commit()
        .map_err(|error| format!("タググループの削除を確定できませんでした: {error}"))
}

pub fn move_tag_with_revision(
    repository_root: &Path,
    tag_id: i64,
    parent_id: Option<i64>,
    expected_revision: i64,
    source: crate::database::ChangeSource,
) -> Result<(), String> {
    let mut connection = open(repository_root)?;
    let transaction = begin_immediate(&mut connection, "タグの移動")?;
    let category = transaction
        .query_row("SELECT category FROM tags WHERE id = ?1", [tag_id], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(|error| format!("タグを確認できませんでした: {error}"))?
        .ok_or_else(|| "移動するタグが見つかりません。".to_string())?;
    validate_group_parent(&transaction, &category, parent_id)?;
    let changed = transaction
        .execute(
            "UPDATE tags SET group_id = ?1, revision = revision + 1
             WHERE id = ?2 AND revision = ?3",
            params![parent_id, tag_id, expected_revision],
        )
        .map_err(|error| format!("タグを移動できませんでした: {error}"))?;
    if changed != 1 {
        return Err(conflict("タグ"));
    }
    crate::database::record_change(
        &transaction,
        "tag",
        &tag_id.to_string(),
        "moved",
        source,
        "タググループを変更",
    )?;
    transaction
        .commit()
        .map_err(|error| format!("タグの移動を確定できませんでした: {error}"))
}

pub fn move_tag_group_with_revision(
    repository_root: &Path,
    group_id: i64,
    parent_id: Option<i64>,
    expected_revision: i64,
    source: crate::database::ChangeSource,
) -> Result<(), String> {
    let mut connection = open(repository_root)?;
    let transaction = begin_immediate(&mut connection, "タググループの移動")?;
    let category = transaction
        .query_row(
            "SELECT category FROM tag_groups WHERE id = ?1",
            [group_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("タググループを確認できませんでした: {error}"))?
        .ok_or_else(|| "移動するタググループが見つかりません。".to_string())?;
    validate_group_parent(&transaction, &category, parent_id)?;
    if let Some(parent_id) = parent_id {
        let is_descendant: bool = transaction
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
    let changed = transaction
        .execute(
            "UPDATE tag_groups SET parent_id = ?1, revision = revision + 1
             WHERE id = ?2 AND revision = ?3",
            params![parent_id, group_id, expected_revision],
        )
        .map_err(|error| format!("タググループを移動できませんでした: {error}"))?;
    if changed != 1 {
        return Err(conflict("タググループ"));
    }
    crate::database::record_change(
        &transaction,
        "tag_group",
        &group_id.to_string(),
        "moved",
        source,
        "タググループの親を変更",
    )?;
    transaction
        .commit()
        .map_err(|error| format!("タググループの移動を確定できませんでした: {error}"))
}

pub fn add_tag_to_archives_with_revisions(
    repository_root: &Path,
    archive_ids: &[String],
    category: &str,
    name: &str,
    expected_revisions: &BTreeMap<String, i64>,
    source: crate::database::ChangeSource,
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
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if archive_ids.is_empty() {
        return Err("タグを付ける作品が選択されていません。".to_string());
    }

    let mut connection = open(repository_root)?;
    let transaction = begin_immediate(&mut connection, "タグの更新")?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO tags (name, category) VALUES (?1, ?2)",
            params![name, category],
        )
        .map_err(|error| format!("タグを作成できませんでした: {error}"))?;
    let (tag_id, tag_revision): (i64, i64) = transaction
        .query_row(
            "SELECT id, revision FROM tags WHERE name = ?1 AND category = ?2",
            params![name, category],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| format!("タグを確認できませんでした: {error}"))?;
    bump_rows(
        &transaction,
        "archives",
        "archive_id",
        &archive_ids,
        expected_revisions,
        "作品",
    )?;
    for archive_id in &archive_ids {
        transaction
            .execute(
                "INSERT OR IGNORE INTO archive_tags (archive_id, tag_id) VALUES (?1, ?2)",
                params![archive_id, tag_id],
            )
            .map_err(|error| format!("タグを付けられませんでした: {error}"))?;
        crate::database::record_change(
            &transaction,
            "archive",
            archive_id,
            "tag_added",
            source,
            &format!("タグ「{name}」を追加"),
        )?;
    }
    transaction
        .execute(
            "UPDATE tags SET revision = revision + 1 WHERE id = ?1 AND revision = ?2",
            params![tag_id, tag_revision],
        )
        .map_err(|error| format!("タグのリビジョンを更新できませんでした: {error}"))?;
    crate::database::record_change(
        &transaction,
        "tag",
        &tag_id.to_string(),
        "linked",
        source,
        &format!("タグ「{name}」を作品へ追加"),
    )?;
    transaction
        .commit()
        .map_err(|error| format!("タグの更新を確定できませんでした: {error}"))?;
    Ok(TagSummary {
        id: tag_id,
        name: name.to_string(),
        category: category.to_string(),
        revision: tag_revision + 1,
    })
}

pub fn remove_tag_from_archives_with_revisions(
    repository_root: &Path,
    archive_ids: &[String],
    tag_id: i64,
    expected_revisions: &BTreeMap<String, i64>,
    expected_tag_revision: i64,
    source: crate::database::ChangeSource,
) -> Result<(), String> {
    let archive_ids = archive_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if archive_ids.is_empty() {
        return Err("タグを外す作品が選択されていません。".to_string());
    }
    let mut connection = open(repository_root)?;
    let transaction = begin_immediate(&mut connection, "タグの更新")?;
    bump_rows(
        &transaction,
        "archives",
        "archive_id",
        &archive_ids,
        expected_revisions,
        "作品",
    )?;
    for archive_id in &archive_ids {
        transaction
            .execute(
                "DELETE FROM archive_tags WHERE archive_id = ?1 AND tag_id = ?2",
                params![archive_id, tag_id],
            )
            .map_err(|error| format!("タグを外せませんでした: {error}"))?;
        crate::database::record_change(
            &transaction,
            "archive",
            archive_id,
            "tag_removed",
            source,
            "タグを削除",
        )?;
    }
    let tag_changed = transaction
        .execute(
            "UPDATE tags SET revision = revision + 1 WHERE id = ?1 AND revision = ?2",
            params![tag_id, expected_tag_revision],
        )
        .map_err(|error| format!("タグのリビジョンを更新できませんでした: {error}"))?;
    if tag_changed != 1 {
        return Err(conflict("タグ"));
    }
    crate::database::record_change(
        &transaction,
        "tag",
        &tag_id.to_string(),
        "unlinked",
        source,
        "タグを作品から削除",
    )?;
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
        rename_tag_group, set_favorite, ArchiveQuery, SortBy, SortDirection, RECENT_LIMIT,
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
            let id = format!("{index:064x}");
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
        format!("{index:064x}")
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
    fn archive_queries_return_more_than_fifty_results_except_recent() {
        let repository = repository_with_archives();
        let connection = Connection::open(repository.path().join("database/library.sqlite3"))
            .expect("open database");
        for index in 3..54 {
            let archive_id = id(index);
            let filename = format!("bulk-{index}.cbz");
            connection
                .execute(
                    "INSERT INTO archives (archive_id, original_filename, relative_path, format,
                        file_size, is_favorite, imported_at)
                     VALUES (?1, ?2, ?3, 'cbz', 1, 1, '2026-04-01T00:00:00Z')",
                    params![
                        archive_id,
                        filename,
                        format!("archives/{}/{filename}", &archive_id[..2])
                    ],
                )
                .expect("insert archive");
        }

        assert_eq!(names(repository.path(), &ArchiveQuery::default()).len(), 54);
        assert_eq!(
            names(
                repository.path(),
                &ArchiveQuery {
                    search: Some("bulk-".to_string()),
                    ..Default::default()
                }
            )
            .len(),
            51
        );
        assert_eq!(
            names(
                repository.path(),
                &ArchiveQuery {
                    favorites_only: true,
                    ..Default::default()
                }
            )
            .len(),
            51
        );
        assert_eq!(
            names(
                repository.path(),
                &ArchiveQuery {
                    recent_only: true,
                    ..Default::default()
                }
            )
            .len(),
            RECENT_LIMIT as usize
        );
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
        assert_eq!(search("lph"), ["Alpha_100%.cbz"]);
        let tag = add_tag(repository.path(), &id(2), "author", "山田太郎 100%").expect("tag");
        assert_eq!(search("山田"), ["Gamma.7z"]);
        assert_eq!(search("田太"), ["Gamma.7z"]);
        assert_eq!(search("太郎 100%"), ["Gamma.7z"]);
        assert!(search("太郎 100_").is_empty());
        assert_eq!(
            names(
                repository.path(),
                &ArchiveQuery {
                    search: Some("田太".to_string()),
                    recent_only: true,
                    ..Default::default()
                }
            ),
            ["Gamma.7z"]
        );
        remove_tag(repository.path(), &id(2), tag.id).expect("remove tag");
        assert!(search("田太").is_empty());
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
