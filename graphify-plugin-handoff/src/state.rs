//! Relay 狀態模型與持久化（relay.json schema 1.0.0 + .relay/relay.toon TOON 鏡像）。
//!
//! - `relay.json`：protocol 語意權威（PROTOCOL.md §1），向後相容既有工具。
//! - `.relay/relay.toon`：TOON 序列化鏡像，複用 `graphify-core::to_toon/from_toon`
//!   （relay 狀態放 `metadata.plugin_data["handoff"]`），零自訂 parser。
//! - 寫入安全（PROTOCOL.md §7）：fs2 獨佔鎖 + temp-write + rename。
//! - `.relay/` 內建 `.gitignore`（`*` + `!.gitignore`），避免本地狀態污染 git repo。

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use graphify_core::to_toon;
use serde::{Deserialize, Serialize};

use crate::Error;

/// relay.json 的 schema 版本字串。
pub const SCHEMA_VERSION: &str = "1.0.0";

/// `.relay/` 目錄內的忽略檔內容：忽略全部，僅保留 ignore 檔本身。
const RELAY_DIR_GITIGNORE: &str = "*\n!.gitignore\n";

/// relay.json schema 1.0.0（PROTOCOL.md §1）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RelayState {
    pub schema_version: String,
    #[serde(default)]
    pub project_context: String,
    #[serde(default)]
    pub active_baton: String,
    #[serde(default)]
    pub repos: BTreeMap<String, RepoState>,
    #[serde(default)]
    pub state_snapshot: StateSnapshot,
    #[serde(default)]
    pub spec_sync: SpecSync,
    #[serde(default)]
    pub updated_at: String,
}

impl RelayState {
    /// 建立一份全新的 schema 1.0.0 狀態。
    pub fn fresh() -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            project_context: String::new(),
            active_baton: String::new(),
            repos: BTreeMap::new(),
            state_snapshot: StateSnapshot::default(),
            spec_sync: SpecSync::default(),
            updated_at: now_iso(),
        }
    }
}

impl Default for RelayState {
    fn default() -> Self {
        Self::fresh()
    }
}

/// 單一 repo 的交接狀態。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RepoState {
    pub name: String,
    /// root-relative 子目錄，預設為 repo 名。
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub active_phase: String,
    #[serde(default)]
    pub volatile_state: String,
    #[serde(default = "default_confidence")]
    pub confidence_score: u8,
    #[serde(default)]
    pub debt_tag: Vec<String>,
    #[serde(default)]
    pub next_session_starter: String,
    #[serde(default)]
    pub handoffs: Vec<Handoff>,
    #[serde(default)]
    pub last_updated: String,
}

fn default_confidence() -> u8 {
    3
}

impl RepoState {
    /// 以 repo 名建立預設狀態（path 預設為 repo 名）。
    pub fn for_repo(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            path: name.clone(),
            ..Self::default()
        }
    }
}

impl Default for RepoState {
    fn default() -> Self {
        Self {
            name: String::new(),
            path: String::new(),
            role: String::new(),
            active_phase: String::new(),
            volatile_state: String::new(),
            confidence_score: default_confidence(),
            debt_tag: Vec::new(),
            next_session_starter: String::new(),
            handoffs: Vec::new(),
            last_updated: String::new(),
        }
    }
}

/// 單筆交接紀錄（relayAdd / 歷史）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    pub source: String,
    #[serde(default)]
    pub captured_at: String,
    #[serde(default)]
    pub raw: String,
}

/// 跨 repo 的 session 級快照。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StateSnapshot {
    #[serde(default)]
    pub last_session: String,
    #[serde(default)]
    pub open_threads: Vec<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
}

/// spec 同步快照（sha1-12 哈希對照表）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SpecSync {
    #[serde(default)]
    pub last_sync: String,
    #[serde(default)]
    pub drift: Vec<String>,
    #[serde(default)]
    pub specs: BTreeMap<String, String>,
}

/// 目前 UTC 時間（ISO-8601，毫秒，對應 JS `Date.prototype.toISOString()`）。
pub fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// 讀取 relay.json；檔案不存在回傳 `Ok(None)`。
pub fn load(path: &Path) -> Result<Option<RelayState>, Error> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)?;
    let state = serde_json::from_str(&text)?;
    Ok(Some(state))
}

/// 原子寫入 relay.json：temp-write + rename。
/// 鎖由上層（`relay::locked` 的 read-modify-write）持有，此處只保證單檔原子性。
/// 尾綴 `\n` 與 legacy `writeRelay`（`JSON.stringify(state, null, 2) + "\n"`）位元相容。
pub fn save_atomic(path: &Path, state: &RelayState) -> Result<(), Error> {
    let dir = path.parent().ok_or_else(|| {
        Error::Io(io::Error::new(io::ErrorKind::InvalidInput, "relay.json 無父目錄"))
    })?;
    let json = serde_json::to_string_pretty(state)?;
    let tmp_path = dir.join(format!(".relay.json.tmp-{}", std::process::id()));
    std::fs::write(&tmp_path, format!("{json}\n"))?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// 完整持久化一次 relay 寫入（對應 legacy `writeRelay`）：
/// 更新 `updated_at` → 原子寫 `relay.json` → 寫 TOON 鏡像 `.relay/relay.toon`。
pub fn persist(root: &Path, state: &mut RelayState) -> Result<(), Error> {
    state.updated_at = now_iso();
    save_atomic(&root.join(crate::root::RELAY_JSON), state)?;
    write_toon_mirror(root, state)?;
    Ok(())
}

/// 確保 `.relay/` 存在且內含 `.gitignore`（`*` + `!.gitignore`）。
pub fn ensure_relay_dir(root: &Path) -> Result<(), Error> {
    let dir = root.join(".relay");
    std::fs::create_dir_all(&dir)?;
    let ignore_path = dir.join(".gitignore");
    if !ignore_path.is_file() {
        let mut f = File::create(&ignore_path)?;
        f.write_all(RELAY_DIR_GITIGNORE.as_bytes())?;
    }
    Ok(())
}

/// 將 relay 狀態寫成 TOON 鏡像 `.relay/relay.toon`（複用 core 的 to_toon）。
pub fn write_toon_mirror(root: &Path, state: &RelayState) -> Result<(), Error> {
    ensure_relay_dir(root)?;
    let mut plugin_data = BTreeMap::new();
    plugin_data.insert("handoff".to_string(), serde_json::to_value(state)?);
    let graph = graphify_core::GraphOutput {
        nodes: vec![],
        edges: vec![],
        metadata: graphify_core::GraphMetadata {
            version: SCHEMA_VERSION.to_string(),
            generated_at: now_iso(),
            total_nodes: 0,
            total_edges: 0,
            languages: vec![],
            input_tokens: 0,
            output_tokens: 0,
            plugin_data,
        },
    };
    let toon = to_toon(&graph);
    let tmp = root.join(".relay/relay.toon.tmp");
    std::fs::write(&tmp, toon)?;
    std::fs::rename(&tmp, root.join(".relay/relay.toon"))?;
    Ok(())
}

/// 讀取 TOON 鏡像；不存在回傳 `Ok(None)`。
pub fn read_toon_mirror(root: &Path) -> Result<Option<RelayState>, Error> {
    let path = root.join(".relay/relay.toon");
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)?;
    let graph = graphify_core::from_toon(&text).map_err(|e| Error::Toon(e.to_string()))?;
    match graph.metadata.plugin_data.get("handoff") {
        Some(value) => Ok(Some(serde_json::from_value(value.clone())?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn fresh_state_has_schema_1_0_0() {
        let s = RelayState::fresh();
        assert_eq!(s.schema_version, "1.0.0");
        assert!(s.repos.is_empty());
    }

    #[test]
    fn repo_defaults_confidence_3_and_path_to_name() {
        let r = RepoState::for_repo("foo");
        assert_eq!(r.confidence_score, 3);
        assert_eq!(r.path, "foo");
        assert!(r.debt_tag.is_empty());
    }

    #[test]
    fn save_atomic_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("relay.json");
        let mut state = RelayState::fresh();
        state.project_context = "測試專案".into();
        state.repos.insert(
            "foo".into(),
            RepoState {
                name: "foo".into(),
                path: "foo".into(),
                confidence_score: 5,
                debt_tag: vec!["a".into(), "b".into()],
                ..Default::default()
            },
        );
        save_atomic(&path, &state).unwrap();
        let loaded = load(&path).unwrap().expect("檔案應存在");
        assert_eq!(loaded, state);
    }

    #[test]
    fn load_missing_returns_none() {
        let dir = tempdir().unwrap();
        assert!(load(&dir.path().join("relay.json")).unwrap().is_none());
    }

    #[test]
    fn toon_mirror_roundtrip() {
        let dir = tempdir().unwrap();
        let mut state = RelayState::fresh();
        state.state_snapshot.open_threads = vec!["thread-1".into()];
        state.repos.insert(
            "api".into(),
            RepoState {
                name: "api".into(),
                path: "api".into(),
                volatile_state: "進行中".into(),
                ..Default::default()
            },
        );
        write_toon_mirror(dir.path(), &state).unwrap();
        let read_back = read_toon_mirror(dir.path()).unwrap().expect("鏡像應存在");
        assert_eq!(read_back, state);
    }

    #[test]
    fn relay_dir_has_gitignore() {
        let dir = tempdir().unwrap();
        ensure_relay_dir(dir.path()).unwrap();
        let ignore = std::fs::read_to_string(dir.path().join(".relay/.gitignore")).unwrap();
        assert!(ignore.contains("!.gitignore"));
        // 二次呼叫保持冪等
        ensure_relay_dir(dir.path()).unwrap();
        assert!(dir.path().join(".relay/.gitignore").is_file());
    }

    #[test]
    fn timestamp_is_iso_utc_with_millis() {
        let t = now_iso();
        assert!(t.ends_with('Z'));
        assert!(t.len() >= 20);
    }
}
