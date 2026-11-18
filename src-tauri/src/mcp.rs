use crate::{library, services::LibraryService};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{BufRead, Write},
    path::Path,
};

const PROTOCOL_VERSION: &str = "2024-11-05";

pub fn run(repository_path: &Path) -> Result<(), String> {
    let (_, status) = LibraryService::open(repository_path)?;
    let service = LibraryService::for_mcp(status.path.into());
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve(service, stdin.lock(), stdout.lock())
}

fn serve(
    service: LibraryService,
    mut input: impl BufRead,
    mut output: impl Write,
) -> Result<(), String> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = input
            .read_line(&mut line)
            .map_err(|error| format!("MCP入力を読み取れませんでした: {error}"))?;
        if read == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle_request(&service, request),
            Err(error) => Some(error_response(
                Value::Null,
                -32700,
                &format!("JSONを解析できませんでした: {error}"),
            )),
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut output, &response)
                .map_err(|error| format!("MCP応答を書き込めませんでした: {error}"))?;
            output
                .write_all(b"\n")
                .and_then(|_| output.flush())
                .map_err(|error| format!("MCP応答を送信できませんでした: {error}"))?;
        }
    }
    Ok(())
}

fn handle_request(service: &LibraryService, request: Value) -> Option<Value> {
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(error_response(
            request.get("id").cloned().unwrap_or(Value::Null),
            -32600,
            "JSON-RPC 2.0のメッセージが必要です。",
        ));
    }
    let method = request.get("method").and_then(Value::as_str);
    let id = request.get("id").cloned();
    if method == Some("notifications/initialized") {
        return None;
    }
    let Some(id) = id else {
        return None;
    };
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    let result = match method {
        Some("initialize") => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": { "listChanged": false }
            },
            "serverInfo": {
                "name": "fairy-comic-library",
                "version": env!("CARGO_PKG_VERSION")
            }
        })),
        Some("ping") => Ok(json!({})),
        Some("tools/list") => Ok(json!({ "tools": tool_definitions() })),
        Some("tools/call") => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(error_response(id, -32602, "tools/callにはnameが必要です。"));
            };
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            Ok(match call_tool(service, name, arguments) {
                Ok(result) => {
                    if is_mutating_tool(name) {
                        if let Ok(revision) = service.latest_change_revision() {
                            if let Err(error) =
                                crate::sync::notify(service.repository_root(), revision)
                            {
                                eprintln!("MCP変更通知を送れませんでした: {error}");
                            }
                        }
                    }
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string(&result)
                                .unwrap_or_else(|_| "null".to_string())
                        }],
                        "isError": false
                    })
                }
                Err(error) => json!({
                    "content": [{ "type": "text", "text": error }],
                    "isError": true
                }),
            })
        }
        Some(_) => Err((-32601, "指定されたMCPメソッドは存在しません。".to_string())),
        None => Err((-32600, "methodが必要です。".to_string())),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => error_response(id, code, &message),
    })
}

fn is_mutating_tool(name: &str) -> bool {
    matches!(
        name,
        "set_favorite"
            | "add_tag_to_archives"
            | "remove_tag_from_archives"
            | "create_tag_group"
            | "rename_tag_group"
            | "delete_tag_group"
            | "move_tag"
            | "move_tag_group"
    )
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

fn parse<T: for<'de> Deserialize<'de>>(value: Value, tool: &str) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| format!("{tool}の引数が不正です: {error}"))
}

fn call_tool(service: &LibraryService, name: &str, arguments: Value) -> Result<Value, String> {
    match name {
        "get_repository_status" => service.repository_status().map(|status| json!(status)),
        "list_archives" => {
            let query: library::ArchiveQuery = parse(arguments, name)?;
            service
                .list_archives(&query)
                .map(|archives| json!(archives))
        }
        "list_tags" => service.list_tags().map(|tags| json!(tags)),
        "list_tag_groups" => service.list_tag_groups().map(|groups| json!(groups)),
        "set_favorite" => {
            let args: SetFavoriteArgs = parse(arguments, name)?;
            service
                .set_favorite(&args.archive_id, args.is_favorite, args.expected_revision)
                .map(|_| Value::Null)
        }
        "add_tag_to_archives" => {
            let args: AddTagArgs = parse(arguments, name)?;
            service
                .add_tag_to_archives(
                    &args.archive_ids,
                    &args.category,
                    &args.name,
                    &args.expected_revisions,
                )
                .map(|tag| json!(tag))
        }
        "remove_tag_from_archives" => {
            let args: RemoveTagArgs = parse(arguments, name)?;
            service
                .remove_tag_from_archives(
                    &args.archive_ids,
                    args.tag_id,
                    &args.expected_revisions,
                    args.expected_tag_revision,
                )
                .map(|_| Value::Null)
        }
        "create_tag_group" => {
            let args: CreateGroupArgs = parse(arguments, name)?;
            service
                .create_tag_group(&args.category, &args.name, args.parent_id)
                .map(|group| json!(group))
        }
        "rename_tag_group" => {
            let args: RenameGroupArgs = parse(arguments, name)?;
            service
                .rename_tag_group(args.group_id, &args.name, args.expected_revision)
                .map(|_| Value::Null)
        }
        "delete_tag_group" => {
            let args: DeleteGroupArgs = parse(arguments, name)?;
            service
                .delete_tag_group(args.group_id, args.expected_revision)
                .map(|_| Value::Null)
        }
        "move_tag" => {
            let args: MoveTagArgs = parse(arguments, name)?;
            service
                .move_tag(args.tag_id, args.parent_id, args.expected_revision)
                .map(|_| Value::Null)
        }
        "move_tag_group" => {
            let args: MoveGroupArgs = parse(arguments, name)?;
            service
                .move_tag_group(args.group_id, args.parent_id, args.expected_revision)
                .map(|_| Value::Null)
        }
        _ => Err(format!("未知のMCPツールです: {name}")),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetFavoriteArgs {
    archive_id: String,
    is_favorite: bool,
    expected_revision: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddTagArgs {
    archive_ids: Vec<String>,
    category: String,
    name: String,
    expected_revisions: BTreeMap<String, i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoveTagArgs {
    archive_ids: Vec<String>,
    tag_id: i64,
    expected_revisions: BTreeMap<String, i64>,
    expected_tag_revision: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateGroupArgs {
    category: String,
    name: String,
    #[serde(default)]
    parent_id: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameGroupArgs {
    group_id: i64,
    name: String,
    expected_revision: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteGroupArgs {
    group_id: i64,
    expected_revision: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveTagArgs {
    tag_id: i64,
    parent_id: Option<i64>,
    expected_revision: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveGroupArgs {
    group_id: i64,
    parent_id: Option<i64>,
    expected_revision: i64,
}

fn tool_definitions() -> Value {
    json!([
        tool(
            "get_repository_status",
            "リポジトリの状態と警告を取得します。",
            json!({"type":"object","properties":{},"additionalProperties":false})
        ),
        tool(
            "list_archives",
            "検索・絞り込み・並び替え条件で作品を取得します。",
            archive_query_schema()
        ),
        tool(
            "list_tags",
            "タグと作品数を取得します。",
            json!({"type":"object","properties":{},"additionalProperties":false})
        ),
        tool(
            "list_tag_groups",
            "タググループの階層を取得します。",
            json!({"type":"object","properties":{},"additionalProperties":false})
        ),
        tool(
            "set_favorite",
            "期待リビジョンが一致するときだけ作品のお気に入りを変更します。",
            json!({
                "type":"object","properties":{
                    "archiveId":{"type":"string"},"isFavorite":{"type":"boolean"},"expectedRevision":{"type":"integer"}
                },"required":["archiveId","isFavorite","expectedRevision"],"additionalProperties":false
            })
        ),
        tool(
            "add_tag_to_archives",
            "複数作品へ同じタグを原子的に追加します。",
            json!({
                "type":"object","properties":{
                    "archiveIds":{"type":"array","items":{"type":"string"},"minItems":1},
                    "category":{"type":"string","enum":["author","series","classification"]},
                    "name":{"type":"string","minLength":1,"maxLength":100},
                    "expectedRevisions":{"type":"object","additionalProperties":{"type":"integer"}}
                },"required":["archiveIds","category","name","expectedRevisions"],"additionalProperties":false
            })
        ),
        tool(
            "remove_tag_from_archives",
            "複数作品からタグを原子的に外します。",
            json!({
                "type":"object","properties":{
                    "archiveIds":{"type":"array","items":{"type":"string"},"minItems":1},
                    "tagId":{"type":"integer"},
                    "expectedRevisions":{"type":"object","additionalProperties":{"type":"integer"}},
                    "expectedTagRevision":{"type":"integer"}
                },"required":["archiveIds","tagId","expectedRevisions","expectedTagRevision"],"additionalProperties":false
            })
        ),
        tool(
            "create_tag_group",
            "タググループを作成します。",
            json!({
                "type":"object","properties":{
                    "category":{"type":"string","enum":["author","series","classification"]},
                    "name":{"type":"string","minLength":1,"maxLength":100},
                    "parentId":{"type":["integer","null"]}
                },"required":["category","name"],"additionalProperties":false
            })
        ),
        tool(
            "rename_tag_group",
            "期待リビジョンが一致するときだけタググループ名を変更します。",
            revision_group_name_schema()
        ),
        tool(
            "delete_tag_group",
            "空のタググループを削除します。",
            group_revision_schema()
        ),
        tool(
            "move_tag",
            "タグを別グループへ移動します。",
            move_schema("tagId")
        ),
        tool(
            "move_tag_group",
            "タググループを別グループへ移動します。",
            move_schema("groupId")
        )
    ])
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": input_schema })
}

fn archive_query_schema() -> Value {
    json!({
        "type":"object","properties":{
            "search":{"type":["string","null"]},"favoritesOnly":{"type":"boolean"},
            "tagIds":{"type":"array","items":{"type":"integer"}},
            "recentOnly":{"type":"boolean"},
            "sortBy":{"type":"string","enum":["fileSize","importedAt","lastReadAt","readCount","name"]},
            "sortDirection":{"type":"string","enum":["asc","desc"]}
        },"additionalProperties":false
    })
}

fn revision_group_name_schema() -> Value {
    json!({
        "type":"object","properties":{
            "groupId":{"type":"integer"},"name":{"type":"string","minLength":1,"maxLength":100},
            "expectedRevision":{"type":"integer"}
        },"required":["groupId","name","expectedRevision"],"additionalProperties":false
    })
}

fn group_revision_schema() -> Value {
    json!({
        "type":"object","properties":{
            "groupId":{"type":"integer"},"expectedRevision":{"type":"integer"}
        },"required":["groupId","expectedRevision"],"additionalProperties":false
    })
}

fn move_schema(id_field: &str) -> Value {
    json!({
        "type":"object","properties":{
            id_field: {"type":"integer"},"parentId":{"type":["integer","null"]},
            "expectedRevision":{"type":"integer"}
        },"required":[id_field,"expectedRevision"],"additionalProperties":false
    })
}

#[cfg(test)]
mod tests {
    use super::{call_tool, handle_request, serve, tool_definitions};
    use crate::{database, services::LibraryService};
    use rusqlite::Connection;
    use serde_json::json;
    use std::io::Cursor;

    #[test]
    fn initializes_and_lists_tools_over_stdio() {
        let repository = tempfile::tempdir().expect("temporary repository");
        std::fs::create_dir_all(repository.path().join("database"))
            .expect("create database directory");
        database::initialize(&repository.path().join("database/library.sqlite3"))
            .expect("initialize database");
        let service = LibraryService::for_mcp(repository.path().to_path_buf());
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n\
{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n\
{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n";
        let mut output = Vec::new();

        serve(service, Cursor::new(input), &mut output).expect("serve MCP requests");

        let responses = String::from_utf8(output)
            .expect("UTF-8 response")
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON response"))
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(
            responses[0]["result"]["serverInfo"]["name"],
            "fairy-comic-library"
        );
        assert_eq!(responses[1]["id"], 2);
        assert!(responses[1]["result"]["tools"]
            .as_array()
            .is_some_and(|tools| { tools.iter().any(|tool| tool["name"] == "set_favorite") }));
    }

    #[test]
    fn mcp_updates_use_mcp_source_and_optimistic_lock() {
        let repository = tempfile::tempdir().expect("temporary repository");
        std::fs::create_dir_all(repository.path().join("database"))
            .expect("create database directory");
        let database_path = repository.path().join("database/library.sqlite3");
        database::initialize(&database_path).expect("initialize database");
        let archive_id = "ab".repeat(32);
        let connection = Connection::open(&database_path).expect("open database");
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

        let service = LibraryService::for_mcp(repository.path().to_path_buf());
        call_tool(
            &service,
            "set_favorite",
            json!({
                "archiveId": archive_id,
                "isFavorite": true,
                "expectedRevision": 0
            }),
        )
        .expect("MCP favorite update");
        let conflict = call_tool(
            &service,
            "set_favorite",
            json!({
                "archiveId": archive_id,
                "isFavorite": false,
                "expectedRevision": 0
            }),
        )
        .expect_err("stale update must conflict");
        assert!(conflict.contains("他のユーザーまたはAI"));

        let connection = Connection::open(database_path).expect("open database");
        let source: String = connection
            .query_row("SELECT source FROM library_changes", [], |row| row.get(0))
            .expect("read change source");
        assert_eq!(source, "mcp");
    }

    #[test]
    fn tool_schemas_include_dynamic_move_ids() {
        let move_tag = tool_definitions()
            .as_array()
            .expect("tool array")
            .iter()
            .find(|tool| tool["name"] == "move_tag")
            .expect("move_tag tool")
            .clone();
        assert!(move_tag["inputSchema"]["properties"].get("tagId").is_some());
        assert!(move_tag["inputSchema"]["required"]
            .as_array()
            .is_some_and(|required| required.contains(&json!("tagId"))));
    }

    #[test]
    fn tool_errors_are_mcp_results() {
        let repository = tempfile::tempdir().expect("temporary repository");
        std::fs::create_dir_all(repository.path().join("database"))
            .expect("create database directory");
        database::initialize(&repository.path().join("database/library.sqlite3"))
            .expect("initialize database");
        let service = LibraryService::for_mcp(repository.path().to_path_buf());
        let response = handle_request(
            &service,
            json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "set_favorite",
                    "arguments": {
                        "archiveId": "bad"
                    }
                }
            }),
        )
        .expect("tool response");
        assert_eq!(response["id"], 4);
        assert_eq!(response["result"]["isError"], true);
    }
}
