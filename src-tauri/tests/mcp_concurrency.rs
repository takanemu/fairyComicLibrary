use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{Arc, Barrier},
    thread,
};

fn mcp_binary() -> &'static str {
    env!("CARGO_BIN_EXE_fairy-comic-library")
}

fn spawn_server(repository: &std::path::Path) -> Child {
    Command::new(mcp_binary())
        .args(["--mcp", "--repository"])
        .arg(repository)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn MCP server")
}

fn send_request(child: &mut Child, request: Value) -> Value {
    let stdin = child.stdin.as_mut().expect("stdin");
    serde_json::to_writer(&mut *stdin, &request).expect("write request");
    stdin.write_all(b"\n").expect("finish request");
    stdin.flush().expect("flush request");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).expect("read response");
    child.stdout = Some(reader.into_inner());
    serde_json::from_str(&line).expect("parse response")
}

fn stop_server(mut child: Child) {
    drop(child.stdin.take());
    child.wait().expect("wait MCP server");
}

#[test]
fn two_mcp_processes_cannot_overwrite_the_same_revision() {
    let repository = tempfile::tempdir().expect("repository");
    let mut initializer = spawn_server(repository.path());
    send_request(
        &mut initializer,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "get_repository_status",
            "params": {}
        }),
    );
    stop_server(initializer);

    let archive_id = "ab".repeat(32);
    let database_path = repository.path().join("database/library.sqlite3");
    let connection = Connection::open(&database_path).expect("open database");
    connection
        .execute(
            "INSERT INTO archives (
                archive_id, original_filename, relative_path, format, file_size, imported_at
             ) VALUES (?1, 'concurrent.cbz', 'archives/ab/concurrent.cbz', 'cbz', 1,
                '2026-11-18T00:00:00Z')",
            [&archive_id],
        )
        .expect("insert archive");
    drop(connection);

    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for (id, favorite) in [(10, true), (11, false)] {
        let repository_path = repository.path().to_path_buf();
        let archive_id = archive_id.clone();
        let barrier = barrier.clone();
        handles.push(thread::spawn(move || {
            let mut server = spawn_server(&repository_path);
            barrier.wait();
            let response = send_request(
                &mut server,
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "tools/call",
                    "params": {
                        "name": "set_favorite",
                        "arguments": {
                            "archiveId": archive_id,
                            "isFavorite": favorite,
                            "expectedRevision": 0
                        }
                    }
                }),
            );
            stop_server(server);
            response["result"]["isError"].as_bool().expect("isError")
        }));
    }
    barrier.wait();
    let errors = handles
        .into_iter()
        .map(|handle| handle.join().expect("worker"))
        .collect::<Vec<_>>();
    assert_eq!(errors.iter().filter(|error| !**error).count(), 1);
    assert_eq!(errors.iter().filter(|error| **error).count(), 1);

    let connection = Connection::open(database_path).expect("reopen database");
    let revision: i64 = connection
        .query_row(
            "SELECT revision FROM archives WHERE archive_id = ?1",
            [&archive_id],
            |row| row.get(0),
        )
        .expect("read revision");
    let change_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM library_changes", [], |row| row.get(0))
        .expect("count changes");
    assert_eq!(revision, 1);
    assert_eq!(change_count, 1);
}

#[test]
fn mcp_rejects_a_stale_revision_after_a_gui_update() {
    let repository = tempfile::tempdir().expect("repository");
    let mut initializer = spawn_server(repository.path());
    send_request(
        &mut initializer,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "ping",
            "params": {}
        }),
    );
    stop_server(initializer);

    let archive_id = "cd".repeat(32);
    let database_path = repository.path().join("database/library.sqlite3");
    let connection = Connection::open(&database_path).expect("open database");
    connection
        .execute(
            "INSERT INTO archives (
                archive_id, original_filename, relative_path, format, file_size, imported_at
             ) VALUES (?1, 'gui.cbz', 'archives/cd/gui.cbz', 'cbz', 1,
                '2026-11-18T00:00:00Z')",
            [&archive_id],
        )
        .expect("insert archive");
    let transaction = connection
        .unchecked_transaction()
        .expect("start GUI transaction");
    transaction
        .execute(
            "UPDATE archives SET is_favorite = 1, revision = revision + 1
             WHERE archive_id = ?1 AND revision = 0",
            [&archive_id],
        )
        .expect("GUI update");
    transaction
        .execute(
            "INSERT INTO library_changes (
                entity_type, entity_id, operation, source, summary, changed_at
             ) VALUES ('archive', ?1, 'favorite_changed', 'gui',
                'GUIからお気に入りに追加', '2026-11-18T00:00:00Z')",
            [&archive_id],
        )
        .expect("record GUI change");
    transaction.commit().expect("commit GUI update");
    drop(connection);

    let mut server = spawn_server(repository.path());
    let stale = send_request(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": 20,
            "method": "tools/call",
            "params": {
                "name": "set_favorite",
                "arguments": {
                    "archiveId": archive_id,
                    "isFavorite": false,
                    "expectedRevision": 0
                }
            }
        }),
    );
    assert_eq!(stale["result"]["isError"], true);
    let fresh = send_request(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": 21,
            "method": "tools/call",
            "params": {
                "name": "set_favorite",
                "arguments": {
                    "archiveId": archive_id,
                    "isFavorite": false,
                    "expectedRevision": 1
                }
            }
        }),
    );
    assert_eq!(fresh["result"]["isError"], false);
    stop_server(server);

    let connection = Connection::open(database_path).expect("reopen database");
    let (favorite, revision): (i64, i64) = connection
        .query_row(
            "SELECT is_favorite, revision FROM archives WHERE archive_id = ?1",
            [&archive_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read archive");
    assert_eq!((favorite, revision), (0, 2));
}
