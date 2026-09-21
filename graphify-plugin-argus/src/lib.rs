//! graphify-plugin-argus — 純 bridge（Argus 執行歷程 → Graphify canonical graph
//! 升維綁定 + 變動觸發 RetestSignal）。
//!
//! 不重造 Argus 的排程／執行引擎：歷程 100% 來自 `argus graph` CLI 的
//! Graphify v2 JSON（契約見 ArgusOrchestrator `docs/Phase07_M6_Openspec.md` §7）。
//! 本 plugin 負責兩件事：
//!   1. Ingest：`argus graph` JSON → 快取 `GraphOutput`（proactive sync）。
//!   2. 領域事件：`GraphUpdateEvent` → `RetestSignal`，經 v1.1 `NotifyCallback`
//!      送出（第一階段僅通知，host 決定轉發；不自動重跑）。
//!
//! 對齊規則：`get_id`/`bind`/`get_workspace_key`/`sync_toon`/`on_graph_updated`/
//! `set_notify_callback`/`on_health_check` 為 core v1/v1.1 trait 方法。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::RwLock;

use graphify_core::plugin::{GraphUpdateEvent, GraphifyPlugin, WorkspaceContext};
use graphify_core::{from_toon, GraphOutput, NotifyCallback};

use crate::retest::{detect_retests, RetestSignal};
use crate::sync::{emit_error_packet, emit_packet};

pub mod retest;
pub mod sync;

/// plugin 唯一識別。
pub const PLUGIN_ID: &str = "graphify-plugin-argus";

/// plugin 狀態。
pub struct ArgusPlugin {
    workspace_key: String,
    /// workspace 根目錄（`WorkspaceContext.root_path`）。
    root_path: String,
    /// `argus` binary（PATH 上的名字或絕對路徑）。
    argus_bin: PathBuf,
    /// 覆寫 `ARGUS_STATE_DIR`（`None` = 沿用 Argus 預設）。
    state_dir: Option<PathBuf>,
    /// 記憶體 GraphOutput 快取（sync_toon 填入；retest 判定使用）。
    graph_cache: RwLock<Option<GraphOutput>>,
    /// v1.1 host 注入的 notify callback（產 RetestSignal 時呼叫）。
    notify_cb: Option<NotifyCallback>,
    /// 上一次 on_graph_updated 的 node-id 集合（modified_nodes 為空時的 diff 種子）。
    prev_node_ids: RwLock<HashSet<String>>,
}

// `Box<dyn Fn>` 不實作 `Debug`，手寫 impl（callback 欄位只印存在與否）。
impl std::fmt::Debug for ArgusPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArgusPlugin")
            .field("workspace_key", &self.workspace_key)
            .field("root_path", &self.root_path)
            .field("argus_bin", &self.argus_bin)
            .field("state_dir", &self.state_dir)
            .field("graph_cache", &self.graph_cache)
            .field("notify_cb", &self.notify_cb.is_some())
            .finish()
    }
}

impl Default for ArgusPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ArgusPlugin {
    /// 預設建構：`argus` 取自 PATH，state_dir 沿用環境。
    #[must_use]
    pub fn new() -> Self {
        Self {
            workspace_key: String::new(),
            root_path: String::new(),
            argus_bin: default_argus_bin(),
            state_dir: None,
            graph_cache: RwLock::new(None),
            notify_cb: None,
            prev_node_ids: RwLock::new(HashSet::new()),
        }
    }

    /// 覆寫 `argus` binary 路徑。
    #[must_use]
    pub fn with_argus_bin(mut self, bin: impl Into<PathBuf>) -> Self {
        self.argus_bin = bin.into();
        self
    }

    /// 覆寫 `ARGUS_STATE_DIR`。
    #[must_use]
    pub fn with_state_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.state_dir = Some(dir.into());
        self
    }

    /// 以 `cwd` 合成 `WorkspaceContext` 並 bind（CLI 整合模式）。
    #[must_use]
    pub fn bind_for_cli(mut self, cwd: impl AsRef<Path>) -> Self {
        let cwd_ref = cwd.as_ref();
        let workspace_key = graphify_core::plugin::derive_workspace_key(cwd_ref);
        let name = cwd_ref
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "workspace".to_string());
        let ctx =
            WorkspaceContext::new(workspace_key, name, cwd_ref.to_string_lossy().into_owned());
        self.bind(ctx);
        self
    }

    /// 目前快取的 GraphOutput（無則 `None`）。
    #[must_use]
    pub fn graph(&self) -> Option<GraphOutput> {
        self.graph_cache.read().ok()?.clone()
    }

    /// 執行 `argus graph -o <file>` 並讀回 `GraphOutput`。
    ///
    /// # Errors
    /// binary 不存在、非零退出、或 JSON 解析失敗時回 [`ArgusError`]。
    pub fn run_argus_graph(&self) -> Result<GraphOutput, ArgusError> {
        let tmp = tempfile_path()?;
        let mut cmd = Command::new(&self.argus_bin);
        cmd.arg("graph").arg("-o").arg(&tmp);
        if let Some(dir) = &self.state_dir {
            cmd.env("ARGUS_STATE_DIR", dir);
        }
        let output = cmd.output().map_err(|e| ArgusError::Spawn {
            bin: self.argus_bin.display().to_string(),
            source: e,
        })?;
        if !output.status.success() {
            return Err(ArgusError::Exit {
                code: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        let raw = std::fs::read_to_string(&tmp).map_err(ArgusError::Io)?;
        let _ = std::fs::remove_file(&tmp);
        let graph: GraphOutput = serde_json::from_str(&raw).map_err(ArgusError::Parse)?;
        Ok(graph)
    }

    /// v1.1 事件推送：把序列化 payload 交給 host 注入的 callback（若存在）。
    /// host 負責轉發；無 callback 時靜默跳過。
    fn emit_notify(&self, payload: serde_json::Value) {
        if let Some(cb) = &self.notify_cb {
            cb(payload);
        }
    }

    /// 計算 BFS 種子：`event.modified_nodes` 非空直接用；為空則以本次 graph
    /// 與上次快照的 node-id 差集補位。回傳後更新快照。
    ///
    /// ponytail: 與 review plugin 同策略——host 目前可能傳空 modified_nodes，
    /// diff 是唯一能讓事件在真實路徑觸發的種子來源；首次同步只建 baseline。
    fn impact_seeds(
        &self,
        graph: &GraphOutput,
        event: &GraphUpdateEvent,
    ) -> Vec<graphify_core::NodeId> {
        if !event.modified_nodes.is_empty() {
            return event.modified_nodes.clone();
        }
        let cur: HashSet<&str> = graph.nodes.iter().map(|n| n.id.0.as_str()).collect();
        let prev = self.prev_node_ids.read().ok();
        let seeds = match prev.as_deref() {
            Some(prev) if prev.is_empty() => Vec::new(),
            Some(prev) => prev
                .iter()
                .filter(|id| !cur.contains(id.as_str()))
                .map(|id| graphify_core::NodeId(id.clone()))
                .chain(
                    cur.iter()
                        .filter(|id| !prev.contains(**id))
                        .map(|id| graphify_core::NodeId((*id).to_string())),
                )
                .collect(),
            None => Vec::new(),
        };
        if let Ok(mut prev) = self.prev_node_ids.write() {
            *prev = graph.nodes.iter().map(|n| n.id.0.clone()).collect();
        }
        seeds
    }

    /// sync_toon plugin_data 摘要。
    fn summary_json(&self) -> serde_json::Value {
        let (nodes, edges) = match self.graph() {
            Some(g) => (g.nodes.len(), g.edges.len()),
            None => (0, 0),
        };
        serde_json::json!({
            "argus": {
                "workspace_key": self.workspace_key,
                "nodes": nodes,
                "edges": edges,
                "plugin": PLUGIN_ID,
            }
        })
    }
}

impl GraphifyPlugin for ArgusPlugin {
    fn get_id(&self) -> &str {
        PLUGIN_ID
    }

    fn bind(&mut self, ctx: WorkspaceContext) {
        self.workspace_key = ctx.workspace_key;
        self.root_path = ctx.root_path;
    }

    fn get_workspace_key(&self) -> &str {
        &self.workspace_key
    }

    fn sync_toon(&mut self, opt_toon: Option<Vec<u8>>) -> Vec<u8> {
        match opt_toon {
            // 被動 sync：收下 .toon，解析 GraphOutput 進快取，回覆摘要。
            Some(toon_bytes) => {
                let raw = String::from_utf8_lossy(&toon_bytes);
                match from_toon(&raw) {
                    Ok(graph) => {
                        if let Ok(mut cache) = self.graph_cache.write() {
                            *cache = Some(graph);
                        }
                        emit_packet(&self.workspace_key, &self.summary_json()).into_bytes()
                    }
                    Err(_) => {
                        emit_error_packet("Cannot parse .toon into GraphOutput.").into_bytes()
                    }
                }
            }
            // 主動 sync：跑 argus graph 取歷程進快取，回覆摘要。
            None => match self.run_argus_graph() {
                Ok(graph) => {
                    if let Ok(mut cache) = self.graph_cache.write() {
                        *cache = Some(graph);
                    }
                    emit_packet(&self.workspace_key, &self.summary_json()).into_bytes()
                }
                Err(e) => emit_error_packet(&format!("argus graph failed: {e}")).into_bytes(),
            },
        }
    }

    fn on_graph_updated(&mut self, event: &GraphUpdateEvent) {
        // best-effort：任何失敗靜默跳過（契約：plugin 永不 panic）。
        let Some(graph) = self.graph() else { return };
        let seeds = self.impact_seeds(&graph, event);
        if seeds.is_empty() {
            return;
        }
        for signal in detect_retests(&graph, &seeds, &self.workspace_key) {
            if let Ok(payload) = serde_json::to_value(&signal) {
                self.emit_notify(payload);
            }
        }
    }

    fn set_notify_callback(&mut self, cb: Option<NotifyCallback>) {
        self.notify_cb = cb;
    }

    fn on_health_check(&self) -> bool {
        // 被動探測：binary 可執行即視為健康（<10s，純 spawn --version）。
        Command::new(&self.argus_bin)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

/// `argus` binary 預設值（`ARGUS_BIN` 環境變數覆寫）。
#[must_use]
pub fn default_argus_bin() -> PathBuf {
    std::env::var_os("ARGUS_BIN").map_or_else(|| PathBuf::from("argus"), PathBuf::from)
}

/// 產生唯一暫存檔路徑（不引入 tempfile 於 lib 依賴；以 pid + 時間戳命名）。
fn tempfile_path() -> Result<PathBuf, ArgusError> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    Ok(std::env::temp_dir().join(format!("argus-plugin-{}-{nanos}.json", std::process::id())))
}

/// `run_argus_graph` 錯誤。
#[derive(Debug, thiserror::Error)]
pub enum ArgusError {
    #[error("spawn `{bin}` failed: {source}")]
    Spawn {
        bin: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`argus graph` exited with code {code}: {stderr}")]
    Exit { code: i32, stderr: String },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot parse argus graph JSON: {0}")]
    Parse(#[from] serde_json::Error),
}

/// 便捷：對快取圖判定受影響 phase（供呼叫端直接使用）。
#[must_use]
pub fn retests_for_seeds(
    graph: &GraphOutput,
    seeds: &[graphify_core::NodeId],
    workspace_key: &str,
) -> Vec<RetestSignal> {
    detect_retests(graph, seeds, workspace_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphify_core::plugin::WorkspaceContext;
    use graphify_core::types::{FileType, Node};
    use std::sync::{Arc, Mutex};

    fn node(id: &str, kind: &str, source_file: &str) -> Node {
        Node {
            id: graphify_core::NodeId(id.to_string()),
            label: id.to_string(),
            file_type: FileType::Code,
            kind: kind.to_string(),
            language: "argus-spec".to_string(),
            source_file: source_file.to_string(),
            start_line: 1,
            end_line: 1,
            doc_comment: None,
            description: None,
            metadata: None,
        }
    }

    fn sample_graph() -> GraphOutput {
        GraphOutput {
            nodes: vec![
                node("phase:probe", "phase", ".argus/phases/probe.toml"),
                node("worker:probe/a", "worker", ".argus/phases/probe.toml"),
            ],
            edges: vec![],
            metadata: Default::default(),
        }
    }

    #[test]
    fn plugin_id_and_workspace_key_roundtrip() {
        let mut p = ArgusPlugin::new();
        assert_eq!(p.get_id(), "graphify-plugin-argus");
        assert_eq!(p.get_workspace_key(), "");
        p.bind(WorkspaceContext::new("w-abc", "argus-demo", "/tmp/ws"));
        assert_eq!(p.get_workspace_key(), "w-abc");
    }

    #[test]
    fn sync_toon_passive_parses_and_summarizes() {
        let mut p = ArgusPlugin::new();
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));
        let toon = graphify_core::to_toon(&sample_graph());
        let out = p.sync_toon(Some(toon.into_bytes()));
        let raw = String::from_utf8(out).unwrap();
        let meta = crate::sync::parse_meta(&raw);
        assert_eq!(meta.format_version.as_deref(), Some("1.0.0"));
        assert_eq!(meta.workspace_key.as_deref(), Some("w-1"));
        assert!(meta.error.is_none());
        let cache = p.graph().unwrap();
        assert_eq!(cache.nodes.len(), 2);
    }

    #[test]
    fn sync_toon_passive_garbage_parses_to_empty_graph() {
        // core `from_toon` 對無法辨識的輸入回 Ok（空圖），不會 Err；
        // 契約行為：快取覆蓋為空圖，回正常（非 error）封包。
        let mut p = ArgusPlugin::new();
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));
        let out = p.sync_toon(Some(b"not a toon graph".to_vec()));
        let raw = String::from_utf8(out).unwrap();
        let meta = crate::sync::parse_meta(&raw);
        assert!(meta.error.is_none(), "{raw}");
        assert_eq!(p.graph().map(|g| g.nodes.len()), Some(0));
    }

    #[test]
    fn sync_toon_proactive_missing_binary_returns_error_packet() {
        let mut p = ArgusPlugin::new()
            .with_argus_bin("/nonexistent/argus-definitely-not-here")
            .with_state_dir("/tmp/nonexistent-state");
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));
        let out = p.sync_toon(None);
        let raw = String::from_utf8(out).unwrap();
        let meta = crate::sync::parse_meta(&raw);
        assert!(meta.error.is_some(), "expected error packet, got: {raw}");
    }

    #[test]
    fn health_check_false_for_missing_binary() {
        let p = ArgusPlugin::new().with_argus_bin("/nonexistent/argus-definitely-not-here");
        assert!(!p.on_health_check());
    }

    #[test]
    fn on_graph_updated_emits_retest_signal_via_callback() {
        let mut p = ArgusPlugin::new();
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));
        // 先餵圖進快取
        let toon = graphify_core::to_toon(&sample_graph());
        let _ = p.sync_toon(Some(toon.into_bytes()));

        let seen: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        p.set_notify_callback(Some(Box::new(move |v| {
            sink.lock().unwrap().push(v);
        })));

        let event = GraphUpdateEvent::new(
            "w-1",
            vec![graphify_core::NodeId("worker:probe/a".into())],
            graphify_core::plugin::GraphUpdateKind::Indexed,
        );
        p.on_graph_updated(&event);

        let got = seen.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["kind"], "RetestSignal");
        assert_eq!(got[0]["phase"], "probe");
    }

    #[test]
    fn on_graph_updated_without_callback_does_not_panic() {
        let mut p = ArgusPlugin::new();
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));
        let toon = graphify_core::to_toon(&sample_graph());
        let _ = p.sync_toon(Some(toon.into_bytes()));
        let event = GraphUpdateEvent::new(
            "w-1",
            vec![graphify_core::NodeId("worker:probe/a".into())],
            graphify_core::plugin::GraphUpdateKind::Indexed,
        );
        p.on_graph_updated(&event); // 無 callback，不 panic
    }

    #[test]
    fn on_graph_updated_without_graph_is_noop() {
        let mut p = ArgusPlugin::new();
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));
        let event = GraphUpdateEvent::new(
            "w-1",
            vec![graphify_core::NodeId("worker:probe/a".into())],
            graphify_core::plugin::GraphUpdateKind::Indexed,
        );
        p.on_graph_updated(&event); // 無快取，靜默
    }
}

#[cfg(test)]
mod real_argus_e2e {
    //! 真 `argus graph` 產出對接驗證（openspec 5.1/5.2）。
    //!
    //! 需要 `argus` binary 與 `ARGUS_STATE_DIR` 指向已跑過 phase 的 state。
    //! 以環境變數驅動，CI 無 binary 時自動 skip（不假造資料）。
    use super::*;
    use graphify_core::plugin::{GraphUpdateEvent, GraphUpdateKind, WorkspaceContext};

    fn e2e_bin() -> Option<std::path::PathBuf> {
        let p = std::env::var_os("ARGUS_E2E_BIN")?;
        let p = std::path::PathBuf::from(p);
        p.exists().then_some(p)
    }

    #[test]
    fn real_argus_graph_json_roundtrips_into_plugin() {
        let Some(bin) = e2e_bin() else {
            eprintln!("skip: ARGUS_E2E_BIN not set");
            return;
        };
        let mut p = ArgusPlugin::new().with_argus_bin(bin);
        if let Some(dir) = std::env::var_os("ARGUS_E2E_STATE_DIR") {
            p = p.with_state_dir(dir);
        }
        p.bind(WorkspaceContext::new("w-e2e", "argus", "/tmp/argus-e2e"));

        let out = p.sync_toon(None);
        let raw = String::from_utf8(out).unwrap();
        let meta = crate::sync::parse_meta(&raw);
        assert!(meta.error.is_none(), "proactive sync failed: {raw}");

        let g = p.graph().expect("graph cached");
        assert!(!g.nodes.is_empty(), "expected nodes from real argus graph");
        // 契約：worker 節點的 source_file 指向 phase spec
        let workers: Vec<_> = g.nodes.iter().filter(|n| n.kind == "worker").collect();
        assert!(!workers.is_empty(), "expected worker nodes");
        for w in &workers {
            assert!(
                w.source_file.ends_with(".toml"),
                "worker source_file should be the phase spec: {}",
                w.source_file
            );
        }

        // 5.2：以 worker 節點為種子 → 產出該 phase 的 RetestSignal
        let seed = graphify_core::NodeId(workers[0].id.0.clone());
        let signals = crate::retest::detect_retests(&g, std::slice::from_ref(&seed), "w-e2e");
        assert_eq!(signals.len(), 1, "expected one phase signal");
        let expected_phase = workers[0]
            .id
            .0
            .strip_prefix("worker:")
            .and_then(|r| r.split_once('/'))
            .map(|(p, _)| p.to_string())
            .unwrap();
        assert_eq!(signals[0].phase, expected_phase);

        // on_graph_updated 走完整路徑：種子 → callback 收到 RetestSignal
        let seen: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&seen);
        p.set_notify_callback(Some(Box::new(move |v| {
            sink.lock().unwrap().push(v);
        })));
        let ev = GraphUpdateEvent::new("w-e2e", vec![seed], GraphUpdateKind::Indexed);
        p.on_graph_updated(&ev);
        let got = seen.lock().unwrap();
        assert_eq!(got.len(), 1, "callback should receive one RetestSignal");
        assert_eq!(got[0]["kind"], "RetestSignal");
        assert_eq!(got[0]["phase"], signals[0].phase.as_str());
    }
}
