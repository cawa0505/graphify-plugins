//! `graphify handoff doctor` 編排層：報告組裝、`--scan` 掃描、`--fix` 刪除路徑。
//!
//! 判定核心在 [`super::doctor::checks`]；本檔只做檔案系統編排與輸出。
//! 唯讀為預設；刪除僅 `--fix` opt-in（spec「唯讀報告為預設行為」）。

pub mod checks;

#[cfg(test)]
mod tests;

use crate::root;
use std::path::{Path, PathBuf};

pub use checks::{Finding, Verdict};

/// 單一 relay root（或掃描命中點）的完整檢查：relay.json + `.relay/` 鏡像伴隨。
#[derive(Debug, Clone)]
pub struct Report {
    pub finding: Finding,
    /// 同層 `.relay/` 目錄（存在時記錄；`--fix` 時隨 DIRTY 檔一併刪除）。
    pub mirror_dir: Option<PathBuf>,
}

/// 掃描一個目錄：該目錄若含 relay.json 即檢查（掃描根本身也納入）。
pub fn check_dir(dir: &Path, home: Option<&Path>, scan_mode: bool) -> Option<Report> {
    let relay_json = dir.join("relay.json");
    if !relay_json.is_file() {
        return None;
    }
    let mirror = dir.join(".relay");
    Some(Report {
        finding: checks::check_relay_file(&relay_json, home, scan_mode),
        mirror_dir: mirror.is_dir().then_some(mirror),
    })
}

/// `--scan <root>`：有限深度（≤3 層）找出所有 relay.json 位置並逐檔檢查。
/// 排除 `target/`、`node_modules/`、隱藏目錄（stdlib 手寫，不引 walkdir）。
pub fn scan(root: &Path, home: Option<&Path>) -> Vec<Report> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0u8)];
    while let Some((dir, depth)) = stack.pop() {
        if let Some(r) = check_dir(&dir, home, true) {
            out.push(r);
        }
        if depth >= 3 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            stack.push((p, depth + 1));
        }
    }
    out.sort_by(|a, b| a.finding.path.cmp(&b.finding.path));
    out
}

/// 將刪清單（DIRTY 檔 + 同層 `.relay/`）。INFO/CLEAN 永不入清單。
pub fn deletion_targets(reports: &[Report]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for r in reports {
        if r.finding.verdict == Verdict::Dirty {
            out.push(r.finding.path.clone());
            if let Some(m) = &r.mirror_dir {
                out.push(m.clone());
            }
        }
    }
    out
}

/// 執行刪除：逐一刪、單檔失敗不中斷，回傳失敗清單（spec：彙總後回退出碼 1）。
pub fn apply_fix(targets: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut failures = Vec::new();
    for t in targets {
        let result = if t.is_dir() {
            std::fs::remove_dir_all(t)
        } else {
            std::fs::remove_file(t)
        };
        if let Err(e) = result {
            failures.push((t.clone(), e.to_string()));
        }
    }
    failures
}

/// D5 輸出格式：人類可讀、逐項列判定與證據。
pub fn render(reports: &[Report]) -> String {
    if reports.is_empty() {
        return "no relay state file".to_string();
    }
    let mut lines = Vec::new();
    for r in reports {
        let tag = match r.finding.verdict {
            Verdict::Dirty => "[DIRTY]",
            Verdict::Info => "[INFO] ",
            Verdict::Clean => "[CLEAN]",
        };
        lines.push(format!("{} {}", tag, r.finding.path.display()));
        for reason in &r.finding.reasons {
            lines.push(format!("  - {reason}"));
        }
    }
    lines.join("\n")
}

/// 退出碼契約：0 = 無 DIRTY；1 = 有 DIRTY（或 fix 後殘留/失敗）。
pub fn exit_code(reports: &[Report], fix_failures: Option<&[(PathBuf, String)]>) -> i32 {
    if let Some(f) = fix_failures {
        return if f.is_empty() { 0 } else { 1 };
    }
    if reports.iter().any(|r| r.finding.verdict == Verdict::Dirty) {
        1
    } else {
        0
    }
}

/// relay-workspace-context D4：registry workspace 紀錄的 gateway-cwd 汙染檢查。
///
/// WARN 條件（唯讀，實測真實汙染紀錄 `/home/zeng` 的 key 正由 cwd 自身 derive，
/// key 相符不能作為免查條件）：`root_path` 為存在目錄但取不到 git toplevel（非
/// git），且 key 不符 **或** `root_path == $HOME`。回傳 (workspace_key, 原因) 清單。
pub fn check_registry(
    db_path: &Path,
    home: Option<&Path>,
) -> Result<Vec<(String, String)>, String> {
    use graphify_core::plugin::derive_workspace_key;
    let db = graphify_registry::RegistryDb::open(db_path).map_err(|e| e.to_string())?;
    let mut warns = Vec::new();
    for row in db.list_workspaces().map_err(|e| e.to_string())? {
        let path = PathBuf::from(&row.root_path);
        if !path.is_dir() {
            continue; // 不存在的路徑不在此面管轄（可能是已刪 repo）
        }
        if root::git_toplevel(&path).is_some() {
            continue; // git repo → 合法 workspace
        }
        let key_match = derive_workspace_key(&path) == row.workspace_key;
        let is_home = home.is_some_and(|h| h == path);
        if !key_match || is_home {
            let why = if is_home {
                "registry workspace record likely polluted by gateway cwd ($HOME); verify manually"
            } else {
                "registry workspace record likely polluted by gateway cwd (key mismatch); verify manually"
            };
            warns.push((
                row.workspace_key,
                format!("{} (path: {})", why, row.root_path),
            ));
        }
    }
    Ok(warns)
}
