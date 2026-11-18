use crate::{conversion, importing, library, maintenance, repository, video};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[derive(Debug, Clone)]
pub struct LibraryService {
    repository_root: PathBuf,
}

impl LibraryService {
    pub fn new(repository_root: PathBuf) -> Self {
        Self { repository_root }
    }

    pub fn open(repository_path: &Path) -> Result<(Self, repository::RepositoryStatus), String> {
        let status = repository::open(repository_path)?;
        Ok((Self::new(PathBuf::from(&status.path)), status))
    }

    pub fn repository_status(&self) -> Result<repository::RepositoryStatus, String> {
        repository::open(&self.repository_root)
    }

    pub fn get_video_duration(&self, archive_id: &str) -> Result<u64, String> {
        let (path, format) = maintenance::archive_for_open(&self.repository_root, archive_id)?;
        video::duration_seconds(&path, &format)
    }

    pub fn list_archives(
        &self,
        query: &library::ArchiveQuery,
    ) -> Result<Vec<library::ArchiveSummary>, String> {
        library::list_archives(&self.repository_root, query)
    }

    pub fn list_tags(&self) -> Result<Vec<library::TagWithCount>, String> {
        library::list_tags(&self.repository_root)
    }

    pub fn list_tag_groups(&self) -> Result<Vec<library::TagGroup>, String> {
        library::list_tag_groups(&self.repository_root)
    }

    pub fn create_tag_group(
        &self,
        category: &str,
        name: &str,
        parent_id: Option<i64>,
    ) -> Result<library::TagGroup, String> {
        library::create_tag_group(&self.repository_root, category, name, parent_id)
    }

    pub fn rename_tag_group(
        &self,
        group_id: i64,
        name: &str,
        expected_revision: i64,
    ) -> Result<(), String> {
        library::rename_tag_group_with_revision(
            &self.repository_root,
            group_id,
            name,
            expected_revision,
        )
    }

    pub fn delete_tag_group(&self, group_id: i64, expected_revision: i64) -> Result<(), String> {
        library::delete_tag_group_with_revision(&self.repository_root, group_id, expected_revision)
    }

    pub fn move_tag(
        &self,
        tag_id: i64,
        parent_id: Option<i64>,
        expected_revision: i64,
    ) -> Result<(), String> {
        library::move_tag_with_revision(&self.repository_root, tag_id, parent_id, expected_revision)
    }

    pub fn move_tag_group(
        &self,
        group_id: i64,
        parent_id: Option<i64>,
        expected_revision: i64,
    ) -> Result<(), String> {
        library::move_tag_group_with_revision(
            &self.repository_root,
            group_id,
            parent_id,
            expected_revision,
        )
    }

    pub fn set_favorite(
        &self,
        archive_id: &str,
        is_favorite: bool,
        expected_revision: i64,
    ) -> Result<(), String> {
        library::set_favorite_with_revision(
            &self.repository_root,
            archive_id,
            is_favorite,
            expected_revision,
        )
    }

    pub fn add_tag(
        &self,
        archive_id: &str,
        category: &str,
        name: &str,
        expected_revision: i64,
    ) -> Result<library::TagSummary, String> {
        self.add_tag_to_archives(
            &[archive_id.to_string()],
            category,
            name,
            &BTreeMap::from([(archive_id.to_string(), expected_revision)]),
        )
    }

    pub fn add_tag_to_archives(
        &self,
        archive_ids: &[String],
        category: &str,
        name: &str,
        expected_revisions: &BTreeMap<String, i64>,
    ) -> Result<library::TagSummary, String> {
        library::add_tag_to_archives_with_revisions(
            &self.repository_root,
            archive_ids,
            category,
            name,
            expected_revisions,
        )
    }

    pub fn remove_tag(
        &self,
        archive_id: &str,
        tag_id: i64,
        expected_revision: i64,
        expected_tag_revision: i64,
    ) -> Result<(), String> {
        self.remove_tag_from_archives(
            &[archive_id.to_string()],
            tag_id,
            &BTreeMap::from([(archive_id.to_string(), expected_revision)]),
            expected_tag_revision,
        )
    }

    pub fn remove_tag_from_archives(
        &self,
        archive_ids: &[String],
        tag_id: i64,
        expected_revisions: &BTreeMap<String, i64>,
        expected_tag_revision: i64,
    ) -> Result<(), String> {
        library::remove_tag_from_archives_with_revisions(
            &self.repository_root,
            archive_ids,
            tag_id,
            expected_revisions,
            expected_tag_revision,
        )
    }

    pub async fn import_archive(
        &self,
        path: &Path,
        thumbnail_path: Option<&Path>,
    ) -> Result<importing::ImportOutcome, String> {
        importing::import_archive_with_thumbnail(&self.repository_root, path, thumbnail_path).await
    }

    pub fn backup_repository(
        &self,
        destination: &Path,
    ) -> Result<maintenance::BackupSummary, String> {
        maintenance::backup(&self.repository_root, destination)
    }

    pub fn unmanage_archives(
        &self,
        archive_ids: &[String],
        download_directory: &Path,
    ) -> Result<maintenance::UnmanageSummary, String> {
        maintenance::unmanage_archives(&self.repository_root, download_directory, archive_ids)
    }

    pub fn regenerate_thumbnail(
        &self,
        archive_id: &str,
        entry_name: Option<&str>,
    ) -> Result<(), String> {
        maintenance::regenerate_thumbnail(&self.repository_root, archive_id, entry_name)
    }

    pub fn set_package_image(&self, archive_id: &str, image_path: &Path) -> Result<(), String> {
        maintenance::set_package_image(&self.repository_root, archive_id, image_path)
    }

    pub fn regenerate_missing_thumbnails(&self) -> Result<maintenance::RegenerateSummary, String> {
        maintenance::regenerate_missing_thumbnails(&self.repository_root)
    }

    pub fn list_cover_candidates(&self, archive_id: &str) -> Result<Vec<String>, String> {
        maintenance::list_cover_candidates(&self.repository_root, archive_id)
    }

    pub fn open_in_viewer(&self, archive_id: &str) -> Result<(), String> {
        let (archive, format) = maintenance::archive_for_open(&self.repository_root, archive_id)?;
        if matches!(format.as_str(), "epub" | "azw3" | "mkv" | "mp4") {
            let application = if matches!(format.as_str(), "mkv" | "mp4") {
                "動画プレーヤー"
            } else {
                "電子書籍リーダー"
            };
            let output = std::process::Command::new("xdg-open")
                .arg(&archive)
                .output()
                .map_err(|error| {
                    format!("OSに関連付けられた{application}を起動できませんでした: {error}")
                })?;
            if !output.status.success() {
                let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(if detail.is_empty() {
                    format!("OSに関連付けられた{application}を起動できませんでした。対応するアプリを関連付けてください。")
                } else {
                    format!("OSに関連付けられた{application}を起動できませんでした: {detail}")
                });
            }
        } else {
            let viewer = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join("brave-comic-viewer")))
                .filter(|path| path.is_file())
                .ok_or_else(|| "ビューアー（brave-comic-viewer）が見つかりません。".to_string())?;
            let mut child = std::process::Command::new(viewer)
                .arg(&archive)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|error| format!("ビューアーを起動できませんでした: {error}"))?;
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        library::record_read(&self.repository_root, archive_id).map_err(|error| {
            format!("ビューアーは起動しましたが読書履歴を更新できませんでした: {error}")
        })
    }

    pub fn convert_archive(
        &self,
        archive_id: &str,
        avif_speed: u8,
        cancel: &Arc<AtomicBool>,
        encode: &mut impl FnMut(&Path, &Path, u8) -> Result<(), String>,
        on_progress: &mut impl FnMut(conversion::ConvertProgress),
    ) -> Result<conversion::ConvertOutcome, String> {
        cancel.store(false, Ordering::SeqCst);
        conversion::convert_archive(
            &self.repository_root,
            archive_id,
            avif_speed,
            cancel,
            encode,
            on_progress,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::LibraryService;
    use crate::{database, library::ArchiveQuery};
    use rusqlite::Connection;
    use std::collections::BTreeMap;

    #[test]
    fn gui_and_mcp_can_share_library_operations() {
        let repository = tempfile::tempdir().expect("temporary repository");
        std::fs::create_dir_all(repository.path().join("database"))
            .expect("create database directory");
        database::initialize(&repository.path().join("database/library.sqlite3"))
            .expect("initialize database");

        let archive_id = "ab".repeat(32);
        let connection =
            Connection::open(repository.path().join("database/library.sqlite3")).expect("open db");
        connection
            .execute(
                "INSERT INTO archives (
                    archive_id, original_filename, relative_path, format,
                    file_size, imported_at
                 ) VALUES (?1, 'sample.cbz', 'archives/ab/sample.cbz', 'cbz', 1,
                    '2026-11-18T00:00:00Z')",
                [&archive_id],
            )
            .expect("insert archive");
        drop(connection);

        let gui_service = LibraryService::new(repository.path().to_path_buf());
        let mcp_service = LibraryService::new(repository.path().to_path_buf());

        gui_service
            .set_favorite(&archive_id, true, 0)
            .expect("GUI update");
        let archives = mcp_service
            .list_archives(&ArchiveQuery::default())
            .expect("MCP read");
        assert!(archives[0].is_favorite);

        let tag = mcp_service
            .add_tag(&archive_id, "author", "Shared author", 1)
            .expect("MCP update");
        let archives = gui_service
            .list_archives(&ArchiveQuery::default())
            .expect("GUI read");
        assert_eq!(archives[0].tags, vec![tag]);

        let conflict = mcp_service
            .set_favorite(&archive_id, false, 1)
            .expect_err("stale revision must fail");
        assert!(conflict.contains("他のユーザーまたはAI"));
    }

    #[test]
    fn bulk_revision_conflict_rolls_back_every_archive() {
        let repository = tempfile::tempdir().expect("temporary repository");
        std::fs::create_dir_all(repository.path().join("database"))
            .expect("create database directory");
        database::initialize(&repository.path().join("database/library.sqlite3"))
            .expect("initialize database");
        let archive_ids = ["ab".repeat(32), "cd".repeat(32)];
        let connection =
            Connection::open(repository.path().join("database/library.sqlite3")).expect("open db");
        for (index, archive_id) in archive_ids.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO archives (
                        archive_id, original_filename, relative_path, format,
                        file_size, imported_at
                     ) VALUES (?1, ?2, ?3, 'cbz', 1, '2026-11-18T00:00:00Z')",
                    rusqlite::params![
                        archive_id,
                        format!("{index}.cbz"),
                        format!("archives/{}/{}.cbz", &archive_id[..2], index)
                    ],
                )
                .expect("insert archive");
        }
        drop(connection);

        let service = LibraryService::new(repository.path().to_path_buf());
        let error = service
            .add_tag_to_archives(
                &archive_ids.to_vec(),
                "author",
                "Must roll back",
                &BTreeMap::from([(archive_ids[0].clone(), 0), (archive_ids[1].clone(), 1)]),
            )
            .expect_err("second archive revision must conflict");
        assert!(error.contains("他のユーザーまたはAI"));

        let archives = service
            .list_archives(&ArchiveQuery::default())
            .expect("list archives");
        assert!(archives.iter().all(|archive| archive.tags.is_empty()));
        assert!(archives.iter().all(|archive| archive.revision == 0));
        assert!(service.list_tags().expect("list tags").is_empty());
        let connection =
            Connection::open(repository.path().join("database/library.sqlite3")).expect("open db");
        let changes: i64 = connection
            .query_row("SELECT COUNT(*) FROM library_changes", [], |row| row.get(0))
            .expect("count changes");
        assert_eq!(changes, 0);
    }
}
