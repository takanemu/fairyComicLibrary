use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

pub const CURRENT_SCHEMA_VERSION: u32 = 2;

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
