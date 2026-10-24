use rusqlite::{named_params, params, Connection};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};

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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagWithCount {
    pub id: i64,
    pub name: String,
    pub category: String,
    pub archive_count: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveSummary {
    pub archive_id: String,
    pub filename: String,
    pub format: String,
    pub file_size: u64,
    pub imported_at: String,
    pub is_favorite: bool,
    pub has_thumbnail: bool,
    pub tags: Vec<TagSummary>,
}

#[derive(Debug, Default, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SortOrder {
    #[default]
    Name,
    Recent,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ArchiveQuery {
    pub search: Option<String>,
    pub favorites_only: bool,
    pub tag_ids: Vec<i64>,
    pub sort: SortOrder,
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
    let (order_by, limit) = match query.sort {
        SortOrder::Name => (
            "a.original_filename COLLATE NOCASE ASC, a.archive_id ASC",
            -1,
        ),
        SortOrder::Recent => ("a.imported_at DESC, a.archive_id ASC", RECENT_LIMIT),
    };

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
        "SELECT a.archive_id, a.original_filename, a.format, a.file_size,
                a.imported_at, a.is_favorite, a.thumbnail_relative_path IS NOT NULL
         FROM archives a
         WHERE (:favorites = 0 OR a.is_favorite = 1)
           {tag_filter}
           AND (:search IS NULL
                OR a.original_filename LIKE :search ESCAPE '\\'
                OR EXISTS (
                    SELECT 1 FROM archive_tags x JOIN tags t ON t.id = x.tag_id
                    WHERE x.archive_id = a.archive_id AND t.name LIKE :search ESCAPE '\\'))
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
                ":search": search,
                ":limit": limit,
            },
            |row| {
                Ok(ArchiveSummary {
                    archive_id: row.get(0)?,
                    filename: row.get(1)?,
                    format: row.get(2)?,
                    file_size: row.get::<_, i64>(3)?.max(0) as u64,
                    imported_at: row.get(4)?,
                    is_favorite: row.get::<_, i64>(5)? != 0,
                    has_thumbnail: row.get::<_, i64>(6)? != 0,
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

pub fn list_tags(repository_root: &Path) -> Result<Vec<TagWithCount>, String> {
    let connection = open(repository_root)?;
    let mut statement = connection
        .prepare(
            "SELECT t.id, t.name, t.category, COUNT(x.archive_id)
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
            })
        })
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .map_err(|error| format!("タグ一覧を読み取れませんでした: {error}"))?;
    Ok(tags)
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

    let mut connection = open(repository_root)?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("タグの更新を開始できませんでした: {error}"))?;
    let archive_exists = transaction
        .query_row(
            "SELECT 1 FROM archives WHERE archive_id = ?1",
            [archive_id],
            |_| Ok(()),
        )
        .is_ok();
    if !archive_exists {
        return Err("対象のアーカイブが見つかりません。".to_string());
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
    transaction
        .execute(
            "INSERT OR IGNORE INTO archive_tags (archive_id, tag_id) VALUES (?1, ?2)",
            params![archive_id, id],
        )
        .map_err(|error| format!("タグを付けられませんでした: {error}"))?;
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
    let mut connection = open(repository_root)?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("タグの更新を開始できませんでした: {error}"))?;
    transaction
        .execute(
            "DELETE FROM archive_tags WHERE archive_id = ?1 AND tag_id = ?2",
            params![archive_id, tag_id],
        )
        .map_err(|error| format!("タグを外せませんでした: {error}"))?;
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
        add_tag, list_archives, list_tags, remove_tag, set_favorite, ArchiveQuery, SortOrder,
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
        for (index, (name, imported_at)) in [
            ("Alpha_100%.cbz", "2026-01-01T00:00:00Z"),
            ("beta.zip", "2026-03-01T00:00:00Z"),
            ("Gamma.7z", "2026-02-01T00:00:00Z"),
        ]
        .into_iter()
        .enumerate()
        {
            let id = format!("{index:x}").repeat(64);
            connection
                .execute(
                    "INSERT INTO archives (archive_id, original_filename, relative_path, format,
                        file_size, thumbnail_relative_path, imported_at)
                     VALUES (?1, ?2, ?3, 'cbz', 10, ?4, ?5)",
                    params![
                        id,
                        name,
                        format!("archives/{}/{name}", &id[..2]),
                        format!("thumbnails/{}/{id}.webp", &id[..2]),
                        imported_at
                    ],
                )
                .expect("insert archive");
        }
        repository
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
    fn lists_by_name_and_by_recently_imported() {
        let repository = repository_with_archives();
        assert_eq!(
            names(repository.path(), &ArchiveQuery::default()),
            ["Alpha_100%.cbz", "beta.zip", "Gamma.7z"]
        );
        assert_eq!(
            names(
                repository.path(),
                &ArchiveQuery {
                    sort: SortOrder::Recent,
                    ..Default::default()
                }
            ),
            ["beta.zip", "Gamma.7z", "Alpha_100%.cbz"]
        );
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
            ["Alpha_100%.cbz", "beta.zip"]
        );

        remove_tag(repository.path(), &id(0), tag.id).expect("remove from first");
        assert_eq!(list_tags(repository.path()).expect("tags").len(), 1);
        remove_tag(repository.path(), &id(1), tag.id).expect("remove from second");
        assert!(list_tags(repository.path()).expect("tags").is_empty());
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
