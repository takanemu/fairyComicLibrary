use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tauri::{AppHandle, Emitter};

const SOCKET_VERSION: u64 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeNotification {
    pub protocol_version: u64,
    pub repository_path: String,
    pub revision: i64,
}

fn runtime_directory() -> Result<PathBuf, String> {
    let uid = unsafe { libc::geteuid() };
    let directory = std::env::temp_dir().join(format!("fairy-comic-library-{uid}"));
    fs::create_dir_all(&directory)
        .map_err(|error| format!("MCP通知用ディレクトリを作成できませんでした: {error}"))?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("MCP通知用ディレクトリを保護できませんでした: {error}"))?;
    Ok(directory)
}

fn socket_path(repository_root: &Path) -> Result<PathBuf, String> {
    let canonical = repository_root
        .canonicalize()
        .map_err(|error| format!("通知対象リポジトリを確定できませんでした: {error}"))?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    Ok(runtime_directory()?.join(format!("{}.sock", &digest[..24])))
}

pub fn notify(repository_root: &Path, revision: i64) -> Result<(), String> {
    let notification = ChangeNotification {
        protocol_version: SOCKET_VERSION,
        repository_path: repository_root.to_string_lossy().into_owned(),
        revision,
    };
    let mut stream = UnixStream::connect(socket_path(repository_root)?)
        .map_err(|error| format!("GUIへMCP変更通知を接続できませんでした: {error}"))?;
    serde_json::to_writer(&mut stream, &notification)
        .map_err(|error| format!("MCP変更通知を作成できませんでした: {error}"))?;
    stream
        .write_all(b"\n")
        .and_then(|_| stream.flush())
        .map_err(|error| format!("MCP変更通知を送信できませんでした: {error}"))
}

pub fn start_listener(
    app: AppHandle,
    repository_root: PathBuf,
    generation: Arc<AtomicU64>,
) -> Result<(), String> {
    let path = socket_path(&repository_root)?;
    let current_generation = generation.fetch_add(1, Ordering::SeqCst) + 1;

    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            return Ok(());
        }
        fs::remove_file(&path)
            .map_err(|error| format!("古いMCP通知ソケットを削除できませんでした: {error}"))?;
    }

    let listener = UnixListener::bind(&path)
        .map_err(|error| format!("MCP変更通知リスナーを開始できませんでした: {error}"))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("MCP通知ソケットを保護できませんでした: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("MCP変更通知リスナーを設定できませんでした: {error}"))?;

    std::thread::spawn(move || {
        while generation.load(Ordering::SeqCst) == current_generation {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                    let mut bytes = Vec::new();
                    if stream.read_to_end(&mut bytes).is_ok() {
                        if let Ok(notification) =
                            serde_json::from_slice::<ChangeNotification>(&bytes)
                        {
                            if notification.protocol_version == SOCKET_VERSION
                                && notification.repository_path == repository_root.to_string_lossy()
                            {
                                let _ = app.emit("library-changed", notification);
                            }
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
        if generation.load(Ordering::SeqCst) != current_generation && path.exists() {
            let _ = fs::remove_file(path);
        }
    });
    Ok(())
}

pub fn latest_change_revision(repository_root: &Path) -> Result<i64, String> {
    let connection = rusqlite::Connection::open(repository_root.join("database/library.sqlite3"))
        .map_err(|error| format!("変更履歴のDBを開けませんでした: {error}"))?;
    connection
        .query_row(
            "SELECT COALESCE(MAX(revision), 0) FROM library_changes",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("変更履歴の版を読み取れませんでした: {error}"))
}

#[cfg(test)]
mod tests {
    use super::ChangeNotification;
    use std::{io::Read, os::unix::net::UnixListener};

    #[test]
    fn notification_uses_a_private_repository_specific_socket_path() {
        let repository = tempfile::tempdir().expect("temporary repository");
        let path = super::socket_path(repository.path()).expect("socket path");
        assert!(path.to_string_lossy().contains("fairy-comic-library-"));

        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind listener");
        super::notify(repository.path(), 7).expect("notify");
        let (mut stream, _) = listener.accept().expect("accept");
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).expect("read notification");
        let notification: ChangeNotification =
            serde_json::from_slice(&bytes).expect("parse notification");
        assert_eq!(notification.revision, 7);
        std::fs::remove_file(path).expect("remove socket");
    }
}
