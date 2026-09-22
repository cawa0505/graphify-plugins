//! Relay root 解析（PROTOCOL.md §2）。
//!
//! - 具名 repo 操作：走記憶體 state 的 `repos[name].path` registry，零 walk-up。
//! - 無 repo 操作：使用啟動（bind / relayInit）時快取的 root。
//! - 唯一 walk-up 路徑：`relayInit` / `bind` 從起點向上找第一個含 `relay.json` 的目錄。
//! - Fail-fast：找不到 root 即回傳 `No relay.json found. Run relayInit first.`，
//!   絕不做無界搜尋。
//!
//! ## 邊界（bounded walk）
//!
//! walk-up 只在同一個專案範圍內進行，命中任一條件即停：
//!
//! 1. **`.git` 目錄** — 不越過 git repo 邊界。relay root 屬於某個 repo，
//!    子目錄（`myrepo/src/deep`）仍可往上命中 `myrepo/relay.json`，
//!    但 `myrepo` 自己沒有 relay 時就停在 `myrepo/.git`，不會去撿上層的。
//! 2. **`$HOME`** — 絕對上界（防無 `.git` 的散落目錄一路走到 `/`）。
//! 3. **fs root（無 parent）** — 走到頂即停。
//!
//! 為什麼要邊界：無界 walk-up 會讓沒有自己 relay 的深層目錄誤中上層任何
//! stray `relay.json`（實際事故：`$HOME/relay.json` 讓 20 個專案共用一個 root、
//! active_baton 互相覆蓋）。共用 relay 必須是**顯式表態**（在該層 `git init`
//! 或放 relay），而不是靠「剛好往上撞到」的隱式行為。

use std::path::{Path, PathBuf};

/// relay 狀態檔名。
pub const RELAY_JSON: &str = "relay.json";

/// 目錄是否為 relay root（含有 relay.json）。
pub fn is_relay_root(dir: &Path) -> bool {
    dir.join(RELAY_JSON).is_file()
}

/// 目錄是否為 git repo 根（含 `.git`，檔案或目錄皆算 — worktree 用檔案）。
fn is_git_root(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// 回傳 walk-up 的硬邊界：`$HOME`（若可解析），否則 `None`（不設限）。
fn walk_boundary() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// 從 `start` 向上找第一個含 `relay.json` 的目錄，並止於專案邊界。
///
/// 停止條件（先檢查 relay，再判邊界）：
/// - 命中 `relay.json` → 回傳該目錄（最近者優先）。
/// - 該目錄有 `.git` → 停在這裡（本 repo 無 relay，不回傳上層的）。
/// - 該目錄為 `$HOME` 或 fs root → 停手。
pub fn resolve_root(start: &Path) -> Option<PathBuf> {
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let home = walk_boundary();
    let mut cur = Some(start);
    while let Some(dir) = cur {
        // 最近者優先：先看自己這一層有沒有 relay。
        if is_relay_root(&dir) {
            return Some(dir);
        }
        // git repo 邊界：本 repo 沒有自己的 relay 就不再往上。
        if is_git_root(&dir) {
            break;
        }
        // `$HOME` 為絕對邊界（含）。
        if home.as_deref() == Some(dir.as_path()) {
            break;
        }
        let parent = dir.parent()?;
        if parent == dir {
            break;
        }
        cur = Some(parent.to_path_buf());
    }
    None
}

/// 以給定 `root` 解析具名 repo 的實際路徑（`root/<repos[name].path>`）。
/// 不做任何磁碟搜尋 — 純查表。
pub fn repo_abs_path(root: &Path, repo_path: &str) -> PathBuf {
    root.join(repo_path)
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

    /// 在 `dir` 造一個 `.git`（目錄）標記 repo 邊界。
    fn put_git(dir: &Path) {
        fs::create_dir_all(dir.join(".git")).unwrap();
    }

    /// 暫時覆寫 `$HOME`，回傳還原用的 guard。
    ///
    /// 這會改動 process-global 環境變數，故所有用到它的測試都必須標 `#[serial]`
    /// （`serial_test`），否則並行執行時會互相污染（實際發生過：殘留的 HOME
    /// 讓 `no_root_returns_none` 一路走到 `/tmp` 撿到 stray relay）。
    struct HomeGuard(Option<std::ffi::OsString>);
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(v) => unsafe { std::env::set_var("HOME", v) },
                None => unsafe { std::env::remove_var("HOME") },
            }
        }
    }
    /// SAFETY: 呼叫端須標 `#[serial]`；還原由 `HomeGuard::drop` 保證。
    fn set_home(home: &Path) -> HomeGuard {
        let prev = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", home) };
        HomeGuard(prev)
    }

    #[test]
    fn walk_up_finds_ancestor_root() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        put_git(root);
        put_relay(root);
        let nested = root.join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(resolve_root(&nested), Some(root.to_path_buf()));
    }

    /// 起點本身就是 fs root（無 parent）且沒有 relay.json → None，不得 panic。
    #[test]
    fn start_at_fs_root_without_relay_returns_none() {
        assert_eq!(resolve_root(Path::new("/")), None);
    }

    /// 核心回歸：`.git` 邊界。`proj/` 是 repo 但沒有 relay，上層才有 stray relay
    /// → 必須回 None，不得越過 repo 邊界撿上層的。
    #[test]
    #[serial]
    fn never_walks_above_git_boundary() {
        let outer = tempdir().unwrap();
        put_relay(outer.path()); // 上層 stray relay
        let repo = outer.path().join("proj");
        fs::create_dir_all(repo.join("src/deep")).unwrap();
        put_git(&repo); // repo 邊界，但 repo 自己沒有 relay
        let _home = set_home(outer.path());

        assert_eq!(
            resolve_root(&repo.join("src/deep")),
            None,
            "不得越過 .git 撿到上層 stray relay.json"
        );
    }

    /// repo 邊界與 relay 同層時可命中（子目錄仍找得到專案 root）。
    #[test]
    fn git_root_with_relay_is_found_from_subdir() {
        let proj = tempdir().unwrap();
        put_git(proj.path());
        put_relay(proj.path());
        let nested = proj.path().join("src/deep");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(resolve_root(&nested), Some(proj.path().to_path_buf()));
    }

    /// 最近者優先：內層 repo 有 relay 時，不會被外層的 relay 蓋掉。
    #[test]
    fn nearest_root_wins() {
        let outer = tempdir().unwrap();
        put_git(outer.path());
        put_relay(outer.path());
        let inner = outer.path().join("inner");
        fs::create_dir_all(&inner).unwrap();
        put_git(&inner);
        put_relay(&inner);
        assert_eq!(resolve_root(&inner), Some(inner.canonicalize().unwrap()));
    }

    /// 硬邊界：`$HOME` 內沒有 relay.json 時，即使 home 之上有 stray relay.json，
    /// 也絕不越過 home 去撿（回歸測試：曾因 /home/zeng/relay.json 造成多專案共 root）。
    #[test]
    #[serial]
    fn never_walks_above_home() {
        let home = tempdir().unwrap();
        let parent = home.path().parent().unwrap();
        // 在 home 之上（parent）放一顆 stray relay.json
        let stray = parent.join(RELAY_JSON);
        let created = !stray.exists();
        if created {
            fs::write(&stray, "{}").unwrap();
        }
        let nested = home.path().join("proj/src/deep");
        fs::create_dir_all(&nested).unwrap();
        let _home = set_home(home.path());

        let got = resolve_root(&nested);
        if created {
            let _ = fs::remove_file(&stray);
        }
        assert_eq!(got, None, "不得越過 $HOME 撿到 parent 的 stray relay.json");
    }

    /// `$HOME` 本身含 relay.json 時可命中（邊界為「含」）。
    #[test]
    #[serial]
    fn home_itself_is_inclusive_boundary() {
        let home = tempdir().unwrap();
        put_relay(home.path());
        let nested = home.path().join("a/b");
        fs::create_dir_all(&nested).unwrap();
        let _home = set_home(home.path());

        assert_eq!(
            resolve_root(&nested),
            Some(home.path().canonicalize().unwrap())
        );
    }

    #[test]
    fn no_root_returns_none() {
        let dir = tempdir().unwrap();
        // 起點 = tempdir（無 .git、$HOME 內無 relay.json）→ None
        assert_eq!(resolve_root(dir.path()), None);
    }

    #[test]
    fn is_relay_root_detects_file() {
        let dir = tempdir().unwrap();
        assert!(!is_relay_root(dir.path()));
        put_relay(dir.path());
        assert!(is_relay_root(dir.path()));
    }

    #[test]
    fn repo_abs_path_joins_without_search() {
        let root = Path::new("/tmp/relay-root");
        assert_eq!(repo_abs_path(root, "api"), root.join("api"));
    }
}
