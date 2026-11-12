use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

pub const CURRENT_SCHEMA_VERSION: u32 = 5;

pub fn initialize(path: &Path) -> Result<u64, String> {
    let connection = Connection::open(path)
        .map_err(|error| format!("SQLiteデータベースを開けませんでした: {error}"))?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|error| format!("SQLite外部キー制約を有効にできませんでした: {error}"))?;

    let version = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
        .map_err(|error| format!("SQLiteスキーマバージョンを読み取れませんでした: {error}"))?;

    if version > CURRENT_SCHEMA_VERSION {
        return Err(format!(
            "リポジトリのDBスキーマ（{version}）は、このアプリが対応するバージョン（{CURRENT_SCHEMA_VERSION}）より新しいため開けません。アプリを更新してください。"
        ));
    }

    if version == 0 {
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| format!("SQLiteスキーマの更新を開始できませんでした: {error}"))?;

        transaction
            .execute_batch(
                "
                CREATE TABLE archives (
                    archive_id TEXT PRIMARY KEY CHECK(length(archive_id) = 64),
                    original_filename TEXT NOT NULL,
                    original_path TEXT,
                    relative_path TEXT NOT NULL UNIQUE,
                    format TEXT NOT NULL,
                    file_size INTEGER NOT NULL CHECK(file_size >= 0),
                    thumbnail_relative_path TEXT,
                    is_favorite INTEGER NOT NULL DEFAULT 0 CHECK(is_favorite IN (0, 1)),
                    imported_at TEXT NOT NULL
                );

                CREATE TABLE tags (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    category TEXT NOT NULL CHECK(category IN ('author', 'series', 'classification')),
                    UNIQUE(category, name)
                );

                CREATE TABLE archive_tags (
                    archive_id TEXT NOT NULL REFERENCES archives(archive_id) ON DELETE CASCADE,
                    tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                    PRIMARY KEY (archive_id, tag_id)
                );

                CREATE INDEX idx_archives_imported_at ON archives(imported_at);
                CREATE INDEX idx_tags_category_name ON tags(category, name);
                PRAGMA user_version = 1;
                ",
            )
            .map_err(|error| format!("SQLiteスキーマを作成できませんでした: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("SQLiteスキーマの更新を確定できませんでした: {error}"))?;
    }

    if version < 2 {
        // v1のDBは、後続の初期化（version == 0）で作られた直後も含めここで v2 へ上げる。
        let current = connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .map_err(|error| format!("SQLiteスキーマバージョンを読み取れませんでした: {error}"))?;
        if current < 2 {
            let transaction = connection
                .unchecked_transaction()
                .map_err(|error| format!("SQLiteスキーマの更新を開始できませんでした: {error}"))?;
            transaction
                .execute_batch(
                    "
                    ALTER TABLE archives ADD COLUMN converted_from TEXT;
                    CREATE INDEX idx_archives_converted_from ON archives(converted_from);
                    PRAGMA user_version = 2;
                    ",
                )
                .map_err(|error| format!("SQLiteスキーマをv2へ更新できませんでした: {error}"))?;
            transaction
                .commit()
                .map_err(|error| format!("SQLiteスキーマの更新を確定できませんでした: {error}"))?;
        }
    }

    let current = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
        .map_err(|error| format!("SQLiteスキーマバージョンを読み取れませんでした: {error}"))?;
    if current < 3 {
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| format!("SQLiteスキーマの更新を開始できませんでした: {error}"))?;
        transaction
            .execute_batch(
                "
                ALTER TABLE archives ADD COLUMN page_count INTEGER CHECK(page_count >= 0);
                PRAGMA user_version = 3;
                ",
            )
            .map_err(|error| format!("SQLiteスキーマをv3へ更新できませんでした: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("SQLiteスキーマの更新を確定できませんでした: {error}"))?;
    }

    for table in ["archives", "tags", "archive_tags"] {
        let exists = connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("SQLiteテーブルを検証できませんでした: {error}"))?
            .is_some();
        if !exists {
            return Err(format!(
                "リポジトリのSQLiteデータベースに必要なテーブル '{table}' がありません。DBが破損している可能性があります。"
            ));
        }
    }

    let current = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
        .map_err(|error| format!("SQLiteスキーマバージョンを読み取れませんでした: {error}"))?;
    if current < 4 {
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| format!("SQLiteスキーマの更新を開始できませんでした: {error}"))?;
        transaction
            .execute_batch(
                "
                ALTER TABLE archives ADD COLUMN last_read_at TEXT;
                ALTER TABLE archives ADD COLUMN read_count INTEGER NOT NULL DEFAULT 0
                    CHECK(read_count >= 0);
                CREATE INDEX idx_archives_last_read_at ON archives(last_read_at);
                PRAGMA user_version = 4;
                ",
            )
            .map_err(|error| format!("SQLiteスキーマをv4へ更新できませんでした: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("SQLiteスキーマの更新を確定できませんでした: {error}"))?;
    }

    let current = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
        .map_err(|error| format!("SQLiteスキーマバージョンを読み取れませんでした: {error}"))?;
    if current < 5 {
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| format!("SQLiteスキーマの更新を開始できませんでした: {error}"))?;
        transaction
            .execute_batch(
                "
                CREATE TABLE tag_groups (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    category TEXT NOT NULL CHECK(category IN ('author', 'series', 'classification')),
                    parent_id INTEGER REFERENCES tag_groups(id) ON DELETE RESTRICT
                );
                ALTER TABLE tags ADD COLUMN group_id INTEGER REFERENCES tag_groups(id) ON DELETE SET NULL;
                CREATE INDEX idx_tag_groups_parent ON tag_groups(category, parent_id, name);
                CREATE INDEX idx_tags_group ON tags(group_id, name);
                PRAGMA user_version = 5;
                ",
            )
            .map_err(|error| format!("SQLiteスキーマをv5へ更新できませんでした: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("SQLiteスキーマの更新を確定できませんでした: {error}"))?;
    }

    let tag_groups_exists = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'tag_groups'",
            [],
            |_| Ok(()),
        )
        .optional()
        .map_err(|error| format!("SQLiteテーブルを検証できませんでした: {error}"))?
        .is_some();
    if !tag_groups_exists {
        return Err(
            "リポジトリのSQLiteデータベースに必要なテーブル 'tag_groups' がありません。DBが破損している可能性があります。"
                .to_string(),
        );
    }

    connection
        .query_row("SELECT COUNT(*) FROM archives", [], |row| {
            row.get::<_, u64>(0)
        })
        .map_err(|error| format!("SQLiteデータベースを読み取れませんでした: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{initialize, CURRENT_SCHEMA_VERSION};
    use rusqlite::Connection;

    #[test]
    fn initializes_schema_and_reopens_existing_database() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let database_path = repository.path().join("database/library.sqlite3");
        std::fs::create_dir_all(database_path.parent().expect("database parent"))
            .expect("create database directory");

        assert_eq!(initialize(&database_path).expect("initialize database"), 0);

        let connection = Connection::open(&database_path).expect("open database");
        connection
            .execute(
                "INSERT INTO archives (
                    archive_id, original_filename, relative_path, format,
                    file_size, thumbnail_relative_path, imported_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    "a".repeat(64),
                    "sample.cbz",
                    "archives/aa/sample.cbz",
                    "cbz",
                    42_i64,
                    "thumbnails/aa/".to_string() + &"a".repeat(64) + ".webp",
                    "2026-10-24T00:00:00Z",
                ],
            )
            .expect("insert sample archive");
        drop(connection);

        assert_eq!(initialize(&database_path).expect("reopen database"), 1);

        let connection = Connection::open(database_path).expect("open database");
        let version = connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .expect("read schema version");
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        let relative_path = connection
            .query_row(
                "SELECT relative_path FROM archives WHERE archive_id = ?1",
                ["a".repeat(64)],
                |row| row.get::<_, String>(0),
            )
            .expect("read persisted archive path");
        assert_eq!(relative_path, "archives/aa/sample.cbz");
    }

    #[test]
    fn migrates_schema_v3_read_history_fields() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let database_path = repository.path().join("library.sqlite3");
        let connection = Connection::open(&database_path).expect("create v3 database");
        connection
            .execute_batch(
                "
                CREATE TABLE archives (
                    archive_id TEXT PRIMARY KEY CHECK(length(archive_id) = 64),
                    original_filename TEXT NOT NULL,
                    original_path TEXT,
                    relative_path TEXT NOT NULL UNIQUE,
                    format TEXT NOT NULL,
                    file_size INTEGER NOT NULL CHECK(file_size >= 0),
                    thumbnail_relative_path TEXT,
                    is_favorite INTEGER NOT NULL DEFAULT 0 CHECK(is_favorite IN (0, 1)),
                    imported_at TEXT NOT NULL,
                    converted_from TEXT,
                    page_count INTEGER CHECK(page_count >= 0)
                );
                CREATE TABLE tags (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    category TEXT NOT NULL CHECK(category IN ('author', 'series', 'classification')),
                    UNIQUE(category, name)
                );
                CREATE TABLE archive_tags (
                    archive_id TEXT NOT NULL REFERENCES archives(archive_id) ON DELETE CASCADE,
                    tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                    PRIMARY KEY (archive_id, tag_id)
                );
                INSERT INTO archives (
                    archive_id, original_filename, relative_path, format, file_size, imported_at
                ) VALUES (
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    'legacy.cbz', 'archives/aa/legacy.cbz', 'cbz', 42, '2026-10-24T00:00:00Z'
                );
                PRAGMA user_version = 3;
                ",
            )
            .expect("create v3 schema");
        drop(connection);

        assert_eq!(initialize(&database_path).expect("migrate v3 database"), 1);
        let connection = Connection::open(database_path).expect("open migrated database");
        let (last_read_at, read_count): (Option<String>, i64) = connection
            .query_row(
                "SELECT last_read_at, read_count FROM archives WHERE original_filename = 'legacy.cbz'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read new fields");
        assert_eq!(last_read_at, None);
        assert_eq!(read_count, 0);
    }

    #[test]
    fn migrates_v4_tags_without_losing_archive_links() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let database_path = repository.path().join("library.sqlite3");
        let connection = Connection::open(&database_path).expect("create v4 database");
        connection
            .execute_batch(
                "
                CREATE TABLE archives (
                    archive_id TEXT PRIMARY KEY,
                    original_filename TEXT NOT NULL,
                    relative_path TEXT NOT NULL,
                    format TEXT NOT NULL,
                    file_size INTEGER NOT NULL,
                    imported_at TEXT NOT NULL
                );
                CREATE TABLE tags (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    category TEXT NOT NULL,
                    UNIQUE(category, name)
                );
                CREATE TABLE archive_tags (
                    archive_id TEXT NOT NULL REFERENCES archives(archive_id) ON DELETE CASCADE,
                    tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                    PRIMARY KEY (archive_id, tag_id)
                );
                INSERT INTO archives VALUES (
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    'legacy.cbz', 'archives/aa/legacy.cbz', 'cbz', 42, '2026-10-24T00:00:00Z'
                );
                INSERT INTO tags (id, name, category) VALUES (7, 'Existing author', 'author');
                INSERT INTO archive_tags VALUES (
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 7
                );
                PRAGMA user_version = 4;
                ",
            )
            .expect("create v4 schema");
        drop(connection);

        assert_eq!(initialize(&database_path).expect("migrate v4 database"), 1);
        let connection = Connection::open(database_path).expect("open migrated database");
        let version = connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .expect("read migrated schema version");
        let (group_id, linked_count): (Option<i64>, i64) = connection
            .query_row(
                "SELECT
                    (SELECT group_id FROM tags WHERE id = 7),
                    (SELECT COUNT(*) FROM archive_tags WHERE tag_id = 7)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read migrated tag data");
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        assert_eq!(group_id, None);
        assert_eq!(linked_count, 1);
    }

    #[test]
    fn rejects_newer_schema_versions() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let database_path = repository.path().join("library.sqlite3");
        let connection = Connection::open(&database_path).expect("create database");
        connection
            .pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION + 1)
            .expect("set future schema version");
        drop(connection);

        let error = initialize(&database_path).expect_err("reject newer database");
        assert!(error.contains("アプリを更新してください"));
    }
}
