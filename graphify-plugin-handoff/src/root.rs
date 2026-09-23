//! Relay root 解析（workspace 主權模型；PROTOCOL.md §2，2026-09-24 修訂）。
//!
//! relay root = workspace root，**不做任何向上（walk-up）搜尋**：
//!
//! 1. `GRAPHIFY_RELAY_ROOT` env override：設定時跳過解析，直接綁定該路徑
//!    （刻意共用 relay root 的明確出口，例如跨 repo 的單一 baton 工作流）。
//! 2. cwd 位於 git repo 內：`git rev-parse --show-toplevel`（git 邊界唯一決定 root）。
//! 3. 非 git 目錄：cwd 本身。
//!
//! relay 狀態檔只認 workspace root 這一個位置。workspace root 沒有 relay.json 時，
//! relay 工具回明確錯誤，指引 `relay_init` 或 `GRAPHIFY_RELAY_ROOT`。
//!
//! 為什麼完全向上搜尋：walk-up 讓「上層任何 relay.json」決定你的 root——污染是
//! 結構性的（`$HOME/relay.json` 曾讓 20 個專案共用一個 root、active_baton 互相
//! 覆蓋）。workspace 主權模型下 root 由 git 邊界唯一決定，無法被上層檔案劫持；
//! 共用 relay 必須是**顯式表態**（`GRAPHIFY_RELAY_ROOT`）。

use std::path::{Path, PathBuf};
use std::process::Command;

/// relay 狀態檔名。
pub const RELAY_JSON: &str = "relay.json";

/// env override 名稱：跳過 workspace root 解析，直接綁定此路徑。
pub const ENV_RELAY_ROOT: &str = "GRAPHIFY_RELAY_ROOT";

/// 目錄是否為 relay root（含有 relay.json）。
pub fn is_relay_root(dir: &Path) -> bool {
    dir.join(RELAY_JSON).is_file()
}

/// workspace root（零 walk-up）。
///
/// 優先序：`GRAPHIFY_RELAY_ROOT`（指向 relay.json 檔時取其 parent 目錄；
/// 指向目錄時直接用該目錄）→ git toplevel → cwd 本身。
pub fn workspace_root(start: &Path) -> PathBuf {
    if let Some(v) = std::env::var_os(ENV_RELAY_ROOT) {
        let p = PathBuf::from(v);
        if p.is_file() {
            return p.parent().unwrap_or(&p).to_path_buf();
        }
        return p;
    }
    git_toplevel(start).unwrap_or_else(|| start.to_path_buf())
}

/// cwd 位於 git repo 內時回傳該 repo 的 toplevel（`git rev-parse --show-toplevel`），
/// 否則 `None`。git 指令本身失敗（例如須先 `git safe.directory`）視同非 git 目錄。
fn git_toplevel(start: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(start)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(PathBuf::from(text))
    }
}

/// 綁定解析：workspace root 且含 relay.json → 該目錄；否則 `None`（工具回 NoRoot）。
pub fn resolve_root(start: &Path) -> Option<PathBuf> {
    let ws = workspace_root(start);
    if is_relay_root(&ws) {
        Some(ws)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use tempfile::tempdir;

    /// 在 `dir` 放一顆 relay.json。
    fn put_relay(dir: &Path) {
        fs::write(dir.join(RELAY_JSON), "{}").unwrap();
    }

    /// 在 `dir` 真正 `git init`（workspace 主權模型以 git 邊界為準）。
    fn git_init(dir: &Path) {
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git init failed in {dir:?}");
    }

    /// 暫時覆寫 env，回傳還原用的 guard（呼叫端須標 `#[serial]`）。
    struct EnvGuard(Option<std::ffi::OsString>);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(v) => unsafe { std::env::set_var(ENV_RELAY_ROOT, &v) },
                None => unsafe { std::env::remove_var(ENV_RELAY_ROOT) },
            }
        }
    }
    /// SAFETY: 呼叫端須標 `#[serial]`；還原由 `EnvGuard::drop` 保證。
    fn set_env(value: &str) -> EnvGuard {
        let prev = std::env::var_os(ENV_RELAY_ROOT);
        unsafe { std::env::set_var(ENV_RELAY_ROOT, value) };
        EnvGuard(prev)
    }

    /// 非 git 目錄 → workspace root = cwd 本身（不向上找 stray relay）。
    #[test]
    #[serial]
    fn non_git_dir_root_is_cwd_itself() {
        let dir = tempdir().unwrap();
        assert_eq!(workspace_root(dir.path()), dir.path().to_path_buf());
    }

    /// git repo 的深層子目錄 → workspace root = git toplevel（不是 cwd 也不是上層）。
    #[test]
    #[serial]
    fn git_subdir_resolves_to_toplevel() {
        let repo = tempdir().unwrap();
        git_init(repo.path());
        let deep = repo.path().join("src/deep");
        fs::create_dir_all(&deep).unwrap();
        assert_eq!(workspace_root(&deep), repo.path().canonicalize().unwrap());
    }

    /// 核心回歸（曾有的事故）：cwd 在 git repo 深層、上層有 stray relay.json →
    /// 只認 git toplevel，不綁定上層 stray。
    #[test]
    #[serial]
    fn git_subdir_ignores_parent_stray_relay() {
        let outer = tempdir().unwrap();
        put_relay(outer.path()); // 上層 stray relay.json
        let repo = outer.path().join("proj");
        fs::create_dir_all(repo.join("src/deep")).unwrap();
        git_init(&repo);

        let ws = workspace_root(&repo.join("src/deep"));
        assert_eq!(ws, repo.canonicalize().unwrap(), "root 由 git 邊界唯一決定");
        assert!(
            !is_relay_root(&ws),
            "workspace root 沒有 relay.json → 不綁定"
        );
        assert_eq!(resolve_root(&repo.join("src/deep")), None);
    }

    /// non-git 目錄位於上層有 stray relay 之下 → 不向上綁定（零 walk-up）。
    #[test]
    #[serial]
    fn non_git_dir_ignores_parent_stray_relay() {
        let outer = tempdir().unwrap();
        put_relay(outer.path());
        let nested = outer.path().join("proj/src/deep");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(resolve_root(&nested), None);
    }

    /// env override 指向目錄 → 直接綁定該目錄（不解析、不 walk-up）。
    #[test]
    #[serial]
    fn env_override_binds_directory() {
        let dir = tempdir().unwrap();
        put_relay(dir.path());
        let _env = set_env(dir.path().to_str().unwrap());
        assert_eq!(
            resolve_root(Path::new("/tmp/elsewhere")),
            Some(dir.path().to_path_buf())
        );
    }

    /// env override 指向 relay.json 檔 → 取其 parent 作為 root。
    #[test]
    #[serial]
    fn env_override_file_uses_parent_dir() {
        let dir = tempdir().unwrap();
        put_relay(dir.path());
        let _env = set_env(dir.path().join(RELAY_JSON).to_str().unwrap());
        assert_eq!(
            workspace_root(Path::new("/tmp/elsewhere")),
            dir.path().to_path_buf()
        );
    }

    /// workspace root 沒 relay.json → 工具層的 NoRoot 由 resolve_root 的 None 觸發。
    #[test]
    #[serial]
    fn no_relay_means_no_root() {
        let dir = tempdir().unwrap();
        assert_eq!(resolve_root(dir.path()), None);
    }

    #[test]
    #[serial]
    fn is_relay_root_detects_file() {
        let dir = tempdir().unwrap();
        assert!(!is_relay_root(dir.path()));
        put_relay(dir.path());
        assert!(is_relay_root(dir.path()));
    }
}
