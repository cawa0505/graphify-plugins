//! doctor 判定核心（openspec/changes/handoff-doctor「檢查項判定準則」）。
//!
//! 純函式：輸入狀態檔路徑與環境線索，輸出 [`Finding`]；不做任何寫入。
//! 以 `serde_json::Value` 弱型別讀取——髒檔正是 schema 不合檔，強型別會解析失敗。

use std::path::{Path, PathBuf};

use serde_json::Value;

/// 判定等級。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Clean,
    Info,
    Dirty,
}

/// 單一 relay 狀態檔的檢查結果：判定等級 + 全部原因（多髒全列不短路）。
#[derive(Debug, Clone)]
pub struct Finding {
    pub path: PathBuf,
    pub verdict: Verdict,
    pub reasons: Vec<String>,
}

enum Reason {
    Dirty(String),
    Info(String),
}

/// 檢查單一 relay.json。`home` 為 $HOME（stray 判定）；`scan_mode` 啟用
/// 「非 workspace root 位置」檢查（--scan 限定；relay root = 檔案所在目錄）。
pub fn check_relay_file(path: &Path, home: Option<&Path>, scan_mode: bool) -> Finding {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            return dirty_now(path, format!("unreadable: {e}"));
        }
    };
    let v: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return dirty_now(path, format!("unparseable JSON: {e}"));
        }
    };
    let mut reasons = foreign_reasons(path.parent().unwrap_or(Path::new("/")), &v);
    reasons.extend(stray_reasons(path, home, scan_mode));
    reasons.extend(legacy_reasons(&v));
    let verdict = if reasons.iter().any(|r| matches!(r, Reason::Dirty(_))) {
        Verdict::Dirty
    } else if reasons.iter().any(|r| matches!(r, Reason::Info(_))) {
        Verdict::Info
    } else {
        Verdict::Clean
    };
    Finding {
        path: path.to_path_buf(),
        verdict,
        reasons: reasons
            .into_iter()
            .map(|r| match r {
                Reason::Dirty(s) | Reason::Info(s) => s,
            })
            .collect(),
    }
}

fn dirty_now(path: &Path, reason: String) -> Finding {
    Finding {
        path: path.to_path_buf(),
        verdict: Verdict::Dirty,
        reasons: vec![reason],
    }
}

/// foreign 三條件（spec 檢查項 2）：(a) bare-name key 不等於 root basename 且
/// root 下無同名子目錄；(b) path 絕對路徑且 canonical 位於 root 外（root 本身
/// 除外）；(c) path 指向不存在的目錄。monorepo 同名子目錄存在 → 不誤殺。
fn foreign_reasons(root: &Path, v: &Value) -> Vec<Reason> {
    let Some(repos) = v.get("repos").and_then(Value::as_object) else {
        return vec![Reason::Dirty(
            "repos field malformed (not an object)".into(),
        )];
    };
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let root_name = root
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut out = Vec::new();
    for (key, entry) in repos {
        let path_str = entry.get("path").and_then(Value::as_str).unwrap_or(key);
        let p = Path::new(path_str);
        let candidate = if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        };
        if !candidate.is_dir() {
            // spec 豁免：legacy 自身條目（key == root basename 且 relative path
            // 為同名裸名）代表 root 本身——舊碼記法，非污染。
            let is_self = key.as_str() == root_name && !p.is_absolute() && path_str == key;
            if !is_self {
                out.push(Reason::Dirty(format!(
                    r#"foreign repo "{key}" (path={path_str}, no such dir)"#
                )));
            }
            continue;
        }
        if p.is_absolute() {
            let canon = candidate
                .canonicalize()
                .unwrap_or_else(|_| candidate.clone());
            if canon != root_canon && !canon.starts_with(&root_canon) {
                out.push(Reason::Dirty(format!(
                    r#"foreign repo "{key}" (path={path_str}, outside relay root)"#
                )));
                continue;
            }
        }
        if key.as_str() != root_name && !root.join(key).is_dir() {
            out.push(Reason::Dirty(format!(
                r#"foreign repo "{key}" (path={path_str}, no such dir)"#
            )));
        }
    }
    out
}

/// stray 位置（spec 檢查項 3）：$HOME 正下方（恆為跨 workspace 共用檔）；
/// scan 模式下位於 git repo 子目錄（bind 永遠看不到的孤兒狀態檔）。
fn stray_reasons(path: &Path, home: Option<&Path>, scan_mode: bool) -> Vec<Reason> {
    let mut out = Vec::new();
    let Some(dir) = path.parent() else {
        return out;
    };
    if home.is_some_and(|h| h == dir) {
        out.push(Reason::Dirty(
            "stray location: directly under $HOME (shared by every session started there)".into(),
        ));
    }
    if scan_mode {
        match crate::root::git_toplevel(dir) {
            Some(tl) if tl != dir => out.push(Reason::Dirty(format!(
                "stray location: inside git repo {} but not its toplevel (orphan state file)",
                tl.display()
            ))),
            _ => {}
        }
    }
    out
}

/// legacy schema（spec 檢查項 4，INFO）：以「非空內容」為訊號——新碼 `fresh()`
/// 也序列化空 `project_context`/空 snapshot，欄位存在與否不構成 legacy 證據。
fn legacy_reasons(v: &Value) -> Vec<Reason> {
    let mut out = Vec::new();
    if !v
        .get("project_context")
        .and_then(Value::as_str)
        .unwrap_or("")
        .is_empty()
    {
        out.push(Reason::Info(
            "legacy global schema (project_context present) — consider relayInit to upgrade".into(),
        ));
    }
    if let Some(ss) = v.get("state_snapshot") {
        let threads = ss
            .get("open_threads")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty());
        let last = ss
            .get("last_session")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty());
        if threads || last {
            out.push(Reason::Info(
                "legacy global schema (state_snapshot present) — consider relayInit to upgrade"
                    .into(),
            ));
        }
    }
    out
}
