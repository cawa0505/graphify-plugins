//! graphify-plugin-handoff — Code Relay 交接狀態 plugin（Graphify 內嵌型 crate）。
//!
//! 實作 `graphify-core` 的 `GraphifyPlugin` v1 trait（`graphify-core/src/plugin.rs`）。
//! relay* 工具語意以公開 API 函式提供（PROTOCOL.md），由 GraphifyMCP 在啟動時
//! 註冊為 MCP tools — 本 crate 不實作任何 MCP transport。
//!
//! 架構（openspec/changes/rust-mcp-migration/design.md）：
//! - `bind(ctx)` 時以 `ctx.root_path` 一次定位 relay root 並常駐記憶體快取。
//! - `relay.json` 為狀態權威；`.relay/relay.toon` 為 TOON 鏡像。
//! - `sync_toon` 交換 .toon 封包（sync-toon-packet 契約）。
//! - `on_graph_updated` 追蹤 active nodes（§4.2）。

pub mod handoff;
pub mod relay;
pub mod skill_install;
pub mod root;
pub mod state;
pub mod sync;

use std::path::Path;

use graphify_core::plugin::{GraphUpdateEvent, GraphifyPlugin, WorkspaceContext};

use state::RelayState;

/// 插件唯一識別碼（同時是 graphify-mcp 自動註冊工具的前綴來源）。
pub const PLUGIN_ID: &str = "graphify-plugin-handoff";

/// 統一錯誤型別。文字為 PROTOCOL.md 凍結語意，消費端可能逐字比對。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("No relay.json found. Run relayInit first.")]
    NoRoot,
    #[error("relay.json already exists at {0}. Edit it or run relaySave.")]
    RootExists(String),
    #[error("repo \"{0}\" not registered. Run relaySave in that repo first.")]
    RepoNotRegistered(String),
    #[error("No active baton set and no repo given. Run relaySwitch <repo> first.")]
    NoActiveBaton,
    #[error("repo \"{0}\" not registered.")]
    RepoUnknown(String),
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("toon: {0}")]
    Toon(String),
}

/// Code Relay plugin 的內嵌實作。
///
/// 生命週期：`new()` → `bind(ctx)`（一次定位 root、載入 state 入記憶體）
/// → 由 Graphify CLI / MCP 驅動 relay* API 與 trait 鉤子。
pub struct RelayPlugin {
    ctx: Option<WorkspaceContext>,
    /// 啟動時定位的 relay root（常駐快取，零重複 walk-up）。
    root: Option<std::path::PathBuf>,
    /// 記憶體中的運行時狀態（runtime source of truth）。
    state: Option<RelayState>,
    /// 本次 session 觸及的 active nodes（on_graph_updated 累積）。
    active_nodes: Vec<String>,
    /// graphify.db 路徑覆寫（測試用；None = graphify-registry 預設 XDG 解析）。
    registry_path: Option<std::path::PathBuf>,
}

impl RelayPlugin {
    /// 建立新的 plugin 實例（尚未 bind）。
    pub fn new() -> Self {
        Self {
            ctx: None,
            root: None,
            state: None,
            active_nodes: Vec::new(),
            registry_path: None,
        }
    }

    /// 指定 graphify.db 路徑（測試注入；正式使用走 registry 預設）。
    pub fn with_registry_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.registry_path = Some(path.into());
        self
    }

    /// 當前綁定的 workspace_key（未 bind 為 None）。
    pub fn workspace_key(&self) -> Option<&str> {
        self.ctx.as_ref().map(|c| c.workspace_key.as_str())
    }

    /// 以目前目錄合成 WorkspaceContext 並 bind（供 handoff-cli 等本地工具使用；
    /// 正式路徑由 Graphify 注入真實 context）。
    pub fn bind_for_cli(&mut self, cwd: &Path) {
        use graphify_core::plugin::derive_workspace_key;
        self.bind(WorkspaceContext::new(
            derive_workspace_key(cwd),
            cwd.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            cwd.display().to_string(),
        ));
    }

    /// 目前已定位的 relay root（未 bind 或未 init 時為 `None`）。
    pub fn root(&self) -> Option<&std::path::PathBuf> {
        self.root.as_ref()
    }

    /// 目前記憶體中的 relay 狀態。
    pub fn state(&self) -> Option<&RelayState> {
        self.state.as_ref()
    }

    /// 本次 session 累積的 active nodes。
    pub fn active_nodes(&self) -> &[String] {
        &self.active_nodes
    }

    /// 以當前綁定上下文產出 .toon 封包；無 root 時回傳 `error` 封包（不得 panic）。
    fn produce_packet(&self) -> Vec<u8> {
        let key = self.get_workspace_key().to_string();
        let Some(state) = self.state.as_ref() else {
            return sync::emit_error_packet("No relay.json found. Run relayInit first.").into_bytes();
        };
        let data = serde_json::json!({
            "handoff": state,
            "active_nodes": self.active_nodes,
        });
        sync::emit_packet(&key, &data).into_bytes()
    }

    /// 消費外部 .toon 封包：驗證版本與 workspace_key，還原 handoff 快照並回 ack。
    fn consume_packet(&mut self, bytes: &[u8]) -> Vec<u8> {
        let err = |msg: &str| sync::emit_error_packet(msg).into_bytes();
        let text = String::from_utf8_lossy(bytes);
        let meta = sync::parse_meta(&text);

        let Some(fv) = meta.format_version.as_deref() else {
            return err("missing format_version in toon metadata");
        };
        if sync::major_mismatch(fv) {
            return err(&format!("unsupported format_version: {fv}"));
        }
        if let Some(k) = meta.workspace_key.as_deref() {
            let bound = self.get_workspace_key();
            if !bound.is_empty() && k != bound {
                return err(&format!("workspace_key mismatch: {k}"));
            }
        }

        let graph = match graphify_core::from_toon(&text) {
            Ok(g) => g,
            Err(e) => return err(&format!("malformed toon: {e}")),
        };
        if let Some(value) = graph.metadata.plugin_data.get("handoff") {
            if let Ok(relay_state) = serde_json::from_value::<RelayState>(value.clone()) {
                // 還原交接快照至記憶體（root 已定位才允許覆寫本地狀態）
                if self.root.is_some() {
                    self.state = Some(relay_state);
                }
            }
        }
        self.produce_packet()
    }
}

impl Default for RelayPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphifyPlugin for RelayPlugin {
    fn get_id(&self) -> &str {
        PLUGIN_ID
    }

    fn bind(&mut self, ctx: WorkspaceContext) {
        let root = root::resolve_root(Path::new(&ctx.root_path));
        self.state = root
            .as_deref()
            .and_then(|r| state::load(&r.join(root::RELAY_JSON)).ok().flatten());
        self.root = root;
        self.ctx = Some(ctx);
    }

    fn get_workspace_key(&self) -> &str {
        self.ctx
            .as_ref()
            .map(|c| c.workspace_key.as_str())
            .unwrap_or("")
    }

    fn sync_toon(&mut self, opt_toon: Option<Vec<u8>>) -> Vec<u8> {
        match opt_toon {
            None => self.produce_packet(),
            Some(bytes) => self.consume_packet(&bytes),
        }
    }

    fn on_graph_updated(&mut self, event: &GraphUpdateEvent) {
        if self.ctx.as_ref().is_none_or(|c| c.workspace_key != event.workspace_key) {
            return;
        }
        for node in &event.modified_nodes {
            if !self.active_nodes.contains(&node.0) {
                self.active_nodes.push(node.0.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphify_core::plugin::{GraphUpdateKind, WorkspaceContext};
    use tempfile::tempdir;

    fn ctx(root: &std::path::Path) -> WorkspaceContext {
        WorkspaceContext::new(
            graphify_core::plugin::derive_workspace_key(root),
            "test-workspace",
            root.display().to_string(),
        )
    }

    #[test]
    fn plugin_id_and_unbound_key() {
        let p = RelayPlugin::new();
        assert_eq!(p.get_id(), PLUGIN_ID);
        assert_eq!(p.get_workspace_key(), "");
        assert!(p.root().is_none());
        assert!(p.state().is_none());
    }

    #[test]
    fn bind_roundtrips_workspace_key() {
        let dir = tempdir().unwrap();
        let mut p = RelayPlugin::new();
        p.bind(ctx(dir.path()));
        assert_eq!(
            p.get_workspace_key(),
            graphify_core::plugin::derive_workspace_key(dir.path())
        );
    }

    #[test]
    fn bind_without_root_produces_error_packet() {
        let dir = tempdir().unwrap();
        let mut p = RelayPlugin::new();
        p.bind(ctx(dir.path()));
        assert!(p.root().is_none());
        let out = p.sync_toon(None);
        let meta = sync::parse_meta(&String::from_utf8_lossy(&out));
        assert_eq!(
            meta.error.as_deref(),
            Some("No relay.json found. Run relayInit first.")
        );
    }

    #[test]
    fn bind_loads_existing_state() {
        let dir = tempdir().unwrap();
        let mut state = RelayState::fresh();
        state.active_baton = "api".into();
        state::save_atomic(&dir.path().join("relay.json"), &state).unwrap();
        let mut p = RelayPlugin::new();
        p.bind(ctx(dir.path()));
        assert_eq!(p.root(), Some(&dir.path().to_path_buf()));
        assert_eq!(p.state().unwrap().active_baton, "api");
    }

    #[test]
    fn proactive_sync_emits_compliant_packet() {
        let dir = tempdir().unwrap();
        let state = RelayState::fresh();
        state::save_atomic(&dir.path().join("relay.json"), &state).unwrap();
        let mut p = RelayPlugin::new();
        p.bind(ctx(dir.path()));
        let out = p.sync_toon(None);
        let meta = sync::parse_meta(&String::from_utf8_lossy(&out));
        assert_eq!(meta.format_version.as_deref(), Some("1.0.0"));
        assert_eq!(meta.workspace_key.as_deref(), Some(p.get_workspace_key()));
        assert!(meta.error.is_none());
        // 封包可被 core from_toon 解析，且 handoff 承載等於本地狀態
        let graph = graphify_core::from_toon(&String::from_utf8_lossy(&out)).unwrap();
        let handoff = graph.metadata.plugin_data.get("handoff").unwrap();
        let decoded: RelayState = serde_json::from_value(handoff.clone()).unwrap();
        assert_eq!(decoded, state);
    }

    #[test]
    fn passive_sync_restores_snapshot_and_acks() {
        let dir = tempdir().unwrap();
        let mut local = RelayState::fresh();
        local.active_baton = "local".into();
        state::save_atomic(&dir.path().join("relay.json"), &local).unwrap();
        let mut p = RelayPlugin::new();
        p.bind(ctx(dir.path()));

        // 外部快照封包
        let mut external = RelayState::fresh();
        external.active_baton = "remote".into();
        external.state_snapshot.open_threads = vec!["t1".into()];
        let data = serde_json::json!({"handoff": external});
        let packet = sync::emit_packet(p.get_workspace_key(), &data);

        let out = p.sync_toon(Some(packet.into_bytes()));
        let meta = sync::parse_meta(&String::from_utf8_lossy(&out));
        assert!(meta.error.is_none(), "ack 不應帶錯誤: {:?}", meta.error);
        assert_eq!(p.state().unwrap().active_baton, "remote");
        assert_eq!(p.state().unwrap().state_snapshot.open_threads, vec!["t1"]);
    }

    #[test]
    fn passive_sync_rejects_foreign_workspace_key() {
        let dir = tempdir().unwrap();
        let state = RelayState::fresh();
        state::save_atomic(&dir.path().join("relay.json"), &state).unwrap();
        let mut p = RelayPlugin::new();
        p.bind(ctx(dir.path()));

        let data = serde_json::json!({"handoff": state});
        let packet = sync::emit_packet("w-foreign-key", &data);
        let out = p.sync_toon(Some(packet.into_bytes()));
        let meta = sync::parse_meta(&String::from_utf8_lossy(&out));
        assert!(meta
            .error
            .as_deref()
            .is_some_and(|e| e.contains("workspace_key mismatch")));
    }

    #[test]
    fn passive_sync_rejects_major_mismatch() {
        let dir = tempdir().unwrap();
        let state = RelayState::fresh();
        state::save_atomic(&dir.path().join("relay.json"), &state).unwrap();
        let mut p = RelayPlugin::new();
        p.bind(ctx(dir.path()));

        let packet = "metadata:\n  format_version: \"2.0.0\"\n  workspace_key: \"w\"\n  plugin_data: {}\n";
        let out = p.sync_toon(Some(packet.as_bytes().to_vec()));
        let meta = sync::parse_meta(&String::from_utf8_lossy(&out));
        assert!(meta
            .error
            .as_deref()
            .is_some_and(|e| e.contains("unsupported format_version")));
    }

    #[test]
    fn on_graph_updated_tracks_only_matching_workspace() {
        let dir = tempdir().unwrap();
        let mut p = RelayPlugin::new();
        p.bind(ctx(dir.path()));
        let wk = p.get_workspace_key().to_string();

        let event = GraphUpdateEvent::new(
            wk.clone(),
            vec![graphify_core::NodeId("a".into()), graphify_core::NodeId("b".into())],
            GraphUpdateKind::Indexed,
        );
        p.on_graph_updated(&event);
        assert_eq!(p.active_nodes(), &["a".to_string(), "b".to_string()]);

        // 重複節點去重
        let dup = GraphUpdateEvent::new(wk, vec![graphify_core::NodeId("a".into())], GraphUpdateKind::Manual);
        p.on_graph_updated(&dup);
        assert_eq!(p.active_nodes().len(), 2);

        // 其他 workspace 的事件被忽略
        let other = GraphUpdateEvent::new(
            "w-other".to_string(),
            vec![graphify_core::NodeId("x".into())],
            GraphUpdateKind::Extracted,
        );
        p.on_graph_updated(&other);
        assert_eq!(p.active_nodes().len(), 2);
    }
}
