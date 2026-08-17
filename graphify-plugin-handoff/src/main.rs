//! handoff-cli — 本地手動測試 CLI（`cargo run --features cli -- <tool> [args]`）。
//!
//! 正式使用路徑是 GraphifyMCP 自動註冊的 relay* MCP tools；此 bin 僅供
//! 開發/除錯，與 legacy `@jimmyyen/opencode-code-relay-plugin` 的
//! `/relay-*` slash commands 同級（避免測試時誤用 legacy 版本）。

#![cfg(feature = "cli")]

use std::path::PathBuf;
use std::process::ExitCode;

use graphify_plugin_handoff::relay::SaveArgs;
use graphify_plugin_handoff::RelayPlugin;

const USAGE: &str = "\
usage: handoff-cli <tool> [args]

tools:
  init [project_context]                    建立 relay root（已存在則拒絕）
  save [--repo R] [--role R] [--phase P] [--volatile V] [--conf N] \\
       [--next N] [--debt a,b,c] [--kind backend|frontend|infra]
  close [--repo R] [--next N]               consistency + spec diff + commit
  switch <repo> [kind]                      交出 baton
  resume [repo] [kind]                      渲染 RESUME（預設 baton repo）
  status                                    全部 repo 摘要
  add <file> [repo]                         收 TODO/handoff 文件進 open_threads
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let tool = args[0].as_str();
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cannot resolve cwd: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut plugin = RelayPlugin::new();
    plugin.bind_for_cli(&cwd);

    let result = match tool {
        "init" => plugin.relay_init(args.get(1).map(String::as_str).unwrap_or(""), None),
        "save" => plugin.relay_save(save_args(&args[1..])),
        "close" => {
            let repo = flag(&args, "--repo");
            let next = flag(&args, "--next");
            plugin.relay_close(repo, next)
        }
        "switch" => {
            let Some(repo) = args.get(1) else {
                eprintln!("switch 需要 <repo>");
                return ExitCode::FAILURE;
            };
            plugin.relay_switch(repo, args.get(2).map(String::as_str))
        }
        "resume" => plugin.relay_resume(args.get(1).map(String::as_str), args.get(2).map(String::as_str)),
        "status" => plugin.relay_status(),
        "add" => {
            let Some(file) = args.get(1) else {
                eprintln!("add 需要 <file>");
                return ExitCode::FAILURE;
            };
            plugin.relay_add(&PathBuf::from(file), args.get(2).map(String::as_str))
        }
        other => {
            eprintln!("unknown tool: {other}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w[0] == name)
        .map(|w| w[1].as_str())
}

fn save_args(args: &[String]) -> SaveArgs<'_> {
    SaveArgs {
        repo: flag(args, "--repo"),
        role: flag(args, "--role"),
        active_phase: flag(args, "--phase"),
        volatile_state: flag(args, "--volatile"),
        confidence: flag(args, "--conf").and_then(|v| v.parse().ok()),
        next_session_starter: flag(args, "--next"),
        debt_tag: flag(args, "--debt"),
        kind: flag(args, "--kind"),
    }
}
