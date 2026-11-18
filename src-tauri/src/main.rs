fn main() {
    let mut args = std::env::args_os().skip(1);
    let mut mcp = false;
    let mut repository = None;
    while let Some(argument) = args.next() {
        if argument == "--mcp" {
            mcp = true;
        } else if argument == "--repository" {
            repository = args.next().map(std::path::PathBuf::from);
        } else if let Some(path) = argument
            .to_str()
            .and_then(|argument| argument.strip_prefix("--repository="))
        {
            repository = Some(std::path::PathBuf::from(path));
        } else if mcp {
            eprintln!("不明なMCP引数です: {}", argument.to_string_lossy());
            std::process::exit(2);
        }
    }

    if mcp {
        let Some(repository) = repository else {
            eprintln!("--mcpには--repository <ディレクトリ>が必要です。");
            std::process::exit(2);
        };
        if let Err(error) = fairy_comic_library_lib::run_mcp(&repository) {
            eprintln!("MCPサーバーを開始できませんでした: {error}");
            std::process::exit(1);
        }
        return;
    }
    fairy_comic_library_lib::run();
}
