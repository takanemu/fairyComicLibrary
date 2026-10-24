mod commands;
mod database;
mod importing;
mod library;
mod maintenance;
mod repository;
mod thumbnails;

use std::{borrow::Cow, sync::Mutex};
use tauri::{http, Manager};
use tokio::sync::Mutex as AsyncMutex;

pub struct AppState {
    repository_path: Mutex<Option<std::path::PathBuf>>,
    import_lock: AsyncMutex<()>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(AppState {
            repository_path: Mutex::new(None),
            import_lock: AsyncMutex::new(()),
        })
        .register_asynchronous_uri_scheme_protocol("thumbnail", |context, request, responder| {
            let state = context.app_handle().state::<AppState>();
            let repository = state
                .repository_path
                .lock()
                .ok()
                .and_then(|path| path.clone());
            let uri_path = request.uri().path().to_string();
            std::thread::spawn(move || {
                responder.respond(thumbnail_response(repository, &uri_path));
            });
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_repository_status,
            commands::open_repository,
            commands::import_archive,
            commands::list_archives,
            commands::list_tags,
            commands::set_favorite,
            commands::add_tag,
            commands::remove_tag,
            commands::backup_repository,
            commands::regenerate_thumbnail,
            commands::regenerate_missing_thumbnails,
            commands::list_cover_candidates,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Fairy Comic Library");
}

fn thumbnail_response(
    repository: Option<std::path::PathBuf>,
    uri_path: &str,
) -> http::Response<Cow<'static, [u8]>> {
    let respond = |status: u16, body: Vec<u8>| {
        // 404を長期キャッシュすると、後から作成したサムネイルが表示されなくなる。
        let cache = if status == 200 {
            "max-age=31536000, immutable"
        } else {
            "no-store"
        };
        http::Response::builder()
            .status(status)
            .header("Content-Type", "image/webp")
            .header("Cache-Control", cache)
            .body(Cow::Owned(body))
            .expect("build thumbnail response")
    };

    let archive_id = uri_path.trim_start_matches('/');
    let is_valid_id =
        archive_id.len() == 64 && archive_id.bytes().all(|byte| byte.is_ascii_hexdigit());
    let Some(repository) = repository.filter(|_| is_valid_id) else {
        return respond(404, Vec::new());
    };
    let path = repository
        .join("thumbnails")
        .join(&archive_id[..2])
        .join(format!("{archive_id}.webp"));
    match std::fs::read(path) {
        Ok(bytes) => respond(200, bytes),
        Err(_) => respond(404, Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::thumbnail_response;

    #[test]
    fn serves_stored_webp_thumbnails_and_rejects_bad_ids() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let id = "ab".repeat(32);
        let directory = repository.path().join("thumbnails/ab");
        std::fs::create_dir_all(&directory).expect("create thumbnail directory");
        std::fs::write(directory.join(format!("{id}.webp")), b"RIFFwebp").expect("write");

        let ok = thumbnail_response(Some(repository.path().to_path_buf()), &format!("/{id}"));
        assert_eq!(ok.status(), 200);
        assert_eq!(ok.headers()["Content-Type"], "image/webp");
        assert_eq!(ok.body().as_ref(), b"RIFFwebp");

        for bad in [
            "/../../etc/passwd",
            "/short",
            &format!("/{}", "g".repeat(64)),
        ] {
            let response = thumbnail_response(Some(repository.path().to_path_buf()), bad);
            assert_eq!(response.status(), 404, "{bad}");
        }
        assert_eq!(thumbnail_response(None, &format!("/{id}")).status(), 404);
    }
}
