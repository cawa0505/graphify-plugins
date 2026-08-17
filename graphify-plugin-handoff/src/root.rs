//! Relay root 解析（PROTOCOL.md §2）。
//!
//! - 具名 repo 操作：走記憶體 state 的 `repos[name].path` registry，零 walk-up。
//! - 無 repo 操作：使用啟動（bind / relayInit）時快取的 root。
//! - 唯一 walk-up 路徑：`relayInit` / `bind` 從起點向上找第一個含 `relay.json` 的目錄。
//! - Fail-fast：找不到 root 即回傳 `No relay.json found. Run relayInit first.`，
//!   絕不做無界搜尋。
//!
//! ponytail: 向上走到 fs root 為止（與 legacy `discoverRoot` 一致，無 home 上限）；
//! 若日後發現誤中家目錄以上的 stray relay.json，再考慮加 XDG 邊界。

use std::path::{Path, PathBuf};

/// relay 狀態檔名。
pub const RELAY_JSON: &str = "relay.json";

/// 目錄是否為 relay root（含有 relay.json）。
pub fn is_relay_root(dir: &Path) -> bool {
    dir.join(RELAY_JSON).is_file()
}

/// 從 `start` 向上找第一個含 `relay.json` 的目錄，走到 fs root 為止。
pub fn resolve_root(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start.to_path_buf());
    while let Some(dir) = cur {
        if is_relay_root(&dir) {
            return Some(dir);
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
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn walk_up_finds_ancestor_root() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(RELAY_JSON), "{}").unwrap();
        let nested = root.join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(resolve_root(&nested), Some(root.to_path_buf()));
    }

    #[test]
    fn walk_up_finds_root_at_fs_root_level() {
        // legacy 行為：無 home 上限，一路走到 fs root
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(RELAY_JSON), "{}").unwrap();
        let nested = root.join("a/b");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(resolve_root(&nested), Some(root.to_path_buf()));
    }

    #[test]
    fn no_root_returns_none() {
        let dir = tempdir().unwrap();
        assert_eq!(resolve_root(dir.path()), None);
    }

    #[test]
    fn is_relay_root_detects_file() {
        let dir = tempdir().unwrap();
        assert!(!is_relay_root(dir.path()));
        fs::write(dir.path().join(RELAY_JSON), "{}").unwrap();
        assert!(is_relay_root(dir.path()));
    }

    #[test]
    fn repo_abs_path_joins_without_search() {
        let root = Path::new("/tmp/relay-root");
        assert_eq!(repo_abs_path(root, "api"), root.join("api"));
    }
}
