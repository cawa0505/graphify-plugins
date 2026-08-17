//! graphify-plugin-review — 純 bridge（code-review-graph 語意點位 → Graphify
//! canonical AST node id 升維綁定）。
//!
//! 不重造 Review 引擎：審查意見 100% 來自 code-review-graph（file-based
//! IngestPayload import，Slice 0）；本 plugin 負責「行號 → canonical node id」
//! 升維、review_bindings 持久化（併入 graphify.db）、查詢與銷案。Slice 1/2
//! 再接 CRG MCP 即時 analysis（`crg_client`）與 impact guard。
//!
//! 對齊規則（#3115、#3119）：`get_id`/`bind`/`get_workspace_key`/`sync_toon`/
//! `on_graph_updated` 為 core v1 trait 方法；業務 API（`review_ingest` /
//! `review_get_context` / `review_resolve`）為公開同步方法，非 trait 方法。

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use graphify_core::plugin::{GraphUpdateEvent, GraphifyPlugin, WorkspaceContext};
use graphify_core::{from_toon, GraphOutput, NotifyCallback};

use crate::crg_client::CrgMcpClient;
use crate::ingest::{parse_payload_file, IngestError, IngestPayload};
use crate::registry::{ReviewBinding, ReviewDb};
use crate::resolver::resolve_line;
use crate::sync::{emit_error_packet, emit_packet};

pub mod crg_client;
pub mod impact;
pub mod ingest;
pub mod registry;
pub mod resolver;
pub mod sync;

/// plugin 唯一識別（graphify-mcp auto-register 的 id 前綴）。
pub const PLUGIN_ID: &str = "graphify-plugin-review";

/// review plugin 狀態。
pub struct ReviewPlugin {
    workspace_key: String,
    /// workspace 根目錄（`WorkspaceContext.root_path`；CRG bridge 的
    /// `repo_root` 參數來源）。
    root_path: String,
    /// 覆寫 graphify.db 路徑（測試注入用）；`None` = 預設 XDG 路徑。
    registry_path: Option<PathBuf>,
    /// 記憶體 GraphOutput 快取（sync_toon 填入；resolver 使用）。
    graph_cache: RwLock<Option<GraphOutput>>,
    /// CRG MCP client 骨架（Slice 1/2 接真呼叫）。
    crg_client: CrgMcpClient,
    /// v1.1 host 注入的 notify callback（Slice 2 產 ImpactAlert 時呼叫；
    /// graphify-mcp 注入，plugin 不開自己的 event bus）。
    notify_cb: Option<NotifyCallback>,
    /// Slice 2 衝擊偵測用：上一次 on_graph_updated 的 node-id 集合。
    /// 當 event.modified_nodes 為空（mcp 兩處 hook 都傳空）時，以
    /// prev/cur diff 作為 BFS 種子（與 Slice 1「Node.id 消失/變更」同語意）。
    prev_node_ids: RwLock<std::collections::HashSet<String>>,
}

// `Box<dyn Fn>` 不實作 `Debug`，手寫 impl（callback 欄位只印存在與否）。
impl std::fmt::Debug for ReviewPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewPlugin")
            .field("workspace_key", &self.workspace_key)
            .field("registry_path", &self.registry_path)
            .field("graph_cache", &self.graph_cache)
            .field("crg_client", &self.crg_client)
            .field("notify_cb", &self.notify_cb.is_some())
            .finish()
    }
}

impl Default for ReviewPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ReviewPlugin {
    /// 預設建構；以 [`graphify_registry::registry_db_path`] 為 db 路徑。
    #[must_use]
    pub fn new() -> Self {
        Self {
            workspace_key: String::new(),
            root_path: String::new(),
            registry_path: None,
            graph_cache: RwLock::new(None),
            crg_client: CrgMcpClient::new(default_crg_url()),
            notify_cb: None,
            prev_node_ids: RwLock::new(std::collections::HashSet::new()),
        }
    }

    /// 覆寫 registry db 路徑（測試注入）。
    #[must_use]
    pub fn with_registry_path(mut self, path: PathBuf) -> Self {
        self.registry_path = Some(path);
        self
    }

    /// 覆寫 CRG MCP base url（Slice 1/2 用；測試注入）。
    #[must_use]
    pub fn with_crg_url(mut self, url: impl Into<String>) -> Self {
        self.crg_client = CrgMcpClient::new(url);
        self
    }

    /// 以 `cwd` 合成 `WorkspaceContext` 並 bind（CLI 整合模式，比照 opendoc）。
    #[must_use]
    pub fn bind_for_cli(mut self, cwd: impl AsRef<Path>) -> Self {
        let cwd_ref = cwd.as_ref();
        let workspace_key = graphify_core::plugin::derive_workspace_key(cwd_ref);
        let name = cwd_ref
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "workspace".to_string());
        let ctx = WorkspaceContext::new(
            workspace_key,
            name,
            cwd_ref.to_string_lossy().into_owned(),
        );
        self.bind(ctx);
        self
    }

    fn registry_path(&self) -> PathBuf {
        self.registry_path
            .clone()
            .unwrap_or_else(graphify_registry::registry_db_path)
    }

    fn db(&self) -> Result<ReviewDb, rusqlite::Error> {
        ReviewDb::open(&self.registry_path())
    }

    /// 目前快取的 GraphOutput（無則 `None`）。
    #[must_use]
    pub fn graph(&self) -> Option<GraphOutput> {
        self.graph_cache.read().ok()?.clone()
    }

    /// 以快取的 GraphOutput 執行 line→symbol 升維。
    #[must_use]
    pub fn resolve(&self, file_path: &str, line: u32) -> Option<resolver::Resolved> {
        self.graph()
            .and_then(|g| resolve_line(&g, file_path, line))
    }

    // ---- 業務 API（graphify-mcp auto-register 對應的工具）----

    /// `review_ingest`：讀取 CRG IngestPayload JSON 檔案，將每筆 review 的
    /// 行號升維綁定至 canonical node id，寫入 graphify.db。
    ///
    /// 回傳 `(bound_count, orphan_lines_count)`。orphan = graph 快取中找不到
    /// 對應節點（檔案未索引或行號超界）——仍寫入但不綁定 node（留
    /// `canonical_node_id` 空字串，Slice 1 drift/resolve 可再處理）。
    ///
    /// # Errors
    /// 檔案讀取/解析失敗回傳 [`crate::ingest::IngestError`]；db 寫入失敗
    /// 亦以 [`crate::ingest::IngestError::Db`] 回傳。
    pub fn review_ingest_file(&self, path: &Path) -> Result<(usize, usize), IngestError> {
        let payload = parse_payload_file(path)?;
        self.review_ingest(&payload)
    }

    /// `review_ingest`（已解析 payload 版本；測試與程式化呼叫用）。
    ///
    /// # Errors
    /// db 寫入失敗回傳 [`crate::ingest::IngestError::Db`]。
    pub fn review_ingest(&self, payload: &IngestPayload) -> Result<(usize, usize), IngestError> {
        let graph = self.graph();
        let db = self.db()?;
        let now = crate::sync::now_rfc3339();
        let mut bound = 0usize;
        let mut orphan = 0usize;

        for review in &payload.reviews {
            let canonical = graph
                .as_ref()
                .and_then(|g| resolve_line(g, &review.file_path, review.line_number))
                .map(|r| r.node_id.0)
                .unwrap_or_default();

            if canonical.is_empty() {
                orphan += 1;
            } else {
                bound += 1;
            }

            db.upsert(&ReviewBinding {
                // 採用 plugin 當前 bound 的 workspace_key（與 relay/opendoc 一致）；
                // IngestPayload 的 workspace_key 僅作為 CRG 端來源標記，不參與綁定。
                workspace_key: self.workspace_key.clone(),
                id: review.review_id.clone(),
                canonical_node_id: canonical,
                file_path: review.file_path.clone(),
                line_number: i64::from(review.line_number),
                signature_hash: "v1_default".to_string(), // YAGNI sentinel（design §7.2）
                severity: review.severity.clone(),
                category: review.category.clone(),
                comment: review.comment.clone(),
                status: "unresolved".to_string(),
                created_at: review.created_at.clone(),
                updated_at: now.clone(),
                resolution_reason: String::new(),
                resolved_at: String::new(),
                resolved_by: String::new(),
            })?;
        }
        Ok((bound, orphan))
    }

    /// `review_search_crg`：呼叫 CRG `detect_changes_tool`，把
    /// `review_priorities`（top-10 風險節點）對映成 IngestPayload 後
    /// 走 `review_ingest` 綁定。
    ///
    /// 對映規則見 crg-requirements.md §4/§5：file:line 由 CRG 節點提供，
    /// severity 由 `risk_score` 對映，workspace_key 用 plugin 綁定 key。
    ///
    /// 回傳 `(bound_node_ids, orphan_count)`：`bound_node_ids` 是本輪
    /// 成功綁定（line→symbol 解析命中）的 canonical node ids，供呼叫端
    /// 直接鏈式 `review_get_context` round-trip；orphan = 未命中的行數。
    ///
    /// # Errors
    /// CRG bridge 失敗回傳 [`crate::crg_client::CrgError`]；綁定失敗回傳
    /// [`IngestError`]。
    pub fn review_search_crg(
        &mut self,
        base: Option<&str>,
    ) -> Result<(Vec<String>, usize), ReviewSearchError> {
        if self.root_path.is_empty() {
            return Err(ReviewSearchError::NoRepoRoot);
        }
        let priorities = self.crg_client.detect_changes(&self.root_path, base)?;
        let now = crate::sync::now_rfc3339();
        // CRG 回傳絕對路徑；graph 節點用 workspace 相對路徑（如 `./src/...`），
        // 先剝掉 `{root_path}/` 前綴再進 resolver。
        let root_prefix = format!("{}/", self.root_path.trim_end_matches('/'));
        let reviews: Vec<crate::ingest::ReviewItem> = priorities
            .into_iter()
            .filter_map(|p| {
                let file_path = p.file_path?;
                let rel = file_path.strip_prefix(&root_prefix).unwrap_or(&file_path);
                let line_number = p.line_start?;
                let name = p.name.unwrap_or_else(|| rel.to_string());
                Some(crate::ingest::ReviewItem {
                    review_id: format!("crg-{rel}:{line_number}:{name}"),
                    file_path: rel.to_string(),
                    line_number,
                    severity: severity_from_risk(p.risk_score.unwrap_or(0.0)),
                    category: p.kind.unwrap_or_else(|| "code-review".to_string()),
                    comment: format!("{name} (risk {})", p.risk_score.unwrap_or(0.0)),
                    created_at: now.clone(),
                })
            })
            .collect();
        let payload = IngestPayload {
            version: "1.0".to_string(),
            source: "code-review-graph".to_string(),
            workspace_key: self.workspace_key.clone(),
            reviews,
        };
        let (_, orphan) = self.review_ingest(&payload)?;
        // 回傳本輪實際綁定的 canonical node ids（用 review_id 精確取回，
        // 不含先前輪次的殘留）。
        let db = self.db()?;
        let bound_ids = payload
            .reviews
            .iter()
            .filter_map(|r| {
                db.get(&self.workspace_key, &r.review_id)
                    .ok()
                    .flatten()
                    .map(|b| b.canonical_node_id)
                    .filter(|n| !n.is_empty())
            })
            .collect();
        Ok((bound_ids, orphan))
    }

    /// `review_get_context`：查詢指定 canonical node 的未解決 review。
    ///
    /// `include_impact_radius` 目前保留參數（Slice 2 實作 BFS 衝擊半徑）；
    /// 現行實作為直查該 node。回傳 `(node_id, unresolved bindings)`。
    ///
    /// # Errors
    /// db 查詢失敗回傳 [`rusqlite::Error`]。
    pub fn review_get_context(
        &self,
        workspace_key: &str,
        node_id: &str,
        _include_impact_radius: bool,
    ) -> Result<(String, Vec<ReviewBinding>), rusqlite::Error> {
        let db = self.db()?;
        let rows = db.query_unresolved_by_node(workspace_key, node_id)?;
        Ok((node_id.to_string(), rows))
    }

    /// `review_resolve`：將指定 review_id 標記為 resolved。回傳 `true` =
    /// 已更新；`false` = review_id 不存在。
    ///
    /// `resolved_by` 標記銷案來源（`"manual"`；自動路徑由 plugin 內部以
    /// `"auto:node_gone"` 寫入，不走此 API）。`resolution_reason` 記錄原因
    /// （手動路徑可空字串）。
    ///
    /// # Errors
    /// db 寫入失敗回傳 [`rusqlite::Error`]。
    pub fn review_resolve(
        &self,
        workspace_key: &str,
        review_id: &str,
        resolved_by: &str,
        resolution_reason: &str,
    ) -> Result<bool, rusqlite::Error> {
        let db = self.db()?;
        let now = crate::sync::now_rfc3339();
        Ok(db.resolve(
            workspace_key,
            review_id,
            &now,
            resolved_by,
            resolution_reason,
            &now,
        )? > 0)
    }
}

/// 預設 CRG base url（`CRG_BASE_URL` 環境變數覆寫；未設定時回
/// `http://127.0.0.1:8080/mcp`，loopback 範例端口，對齊 crg-requirements.md）。
#[must_use]
pub fn default_crg_url() -> String {
    std::env::var("CRG_BASE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8080/mcp".to_string())
}

/// risk_score（0.0-1.0）→ severity 對映（crg-requirements.md §5）。
#[must_use]
pub fn severity_from_risk(risk: f64) -> String {
    if risk >= 0.8 {
        "critical".to_string()
    } else if risk >= 0.5 {
        "high".to_string()
    } else if risk >= 0.3 {
        "medium".to_string()
    } else if risk > 0.0 {
        "low".to_string()
    } else {
        "info".to_string()
    }
}

/// `review_search_crg` 錯誤（CRG bridge 或綁定失敗）。
#[derive(Debug, thiserror::Error)]
pub enum ReviewSearchError {
    #[error("no repo_root: plugin not bound to a workspace")]
    NoRepoRoot,
    #[error("CRG bridge error: {0}")]
    Crg(#[from] crate::crg_client::CrgError),
    #[error("ingest error: {0}")]
    Ingest(#[from] IngestError),
    #[error("registry db error: {0}")]
    Db(#[from] rusqlite::Error),
}

impl GraphifyPlugin for ReviewPlugin {
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
            // 被動 sync：收下 .toon，解析 GraphOutput 進快取，回覆審查摘要。
            Some(toon_bytes) => {
                let raw = String::from_utf8_lossy(&toon_bytes);
                match from_toon(&raw) {
                    Ok(graph) => {
                        *self.graph_cache.write().unwrap() = Some(graph);
                        let summary = self.summary_json();
                        emit_packet(&self.workspace_key, &summary).into_bytes()
                    }
                    Err(_) => emit_error_packet("Cannot parse .toon into GraphOutput.").into_bytes(),
                }
            }
            // 主動 sync：無圖可收，仍可回應 workspace 狀態摘要。
            None => {
                let summary = self.summary_json();
                emit_packet(&self.workspace_key, &summary).into_bytes()
            }
        }
    }

    fn on_graph_updated(&mut self, event: &GraphUpdateEvent) {
        // Slice 1：drift auto-resolver — presence diff。
        //
        // 裁決（design §7.2）：不做 signature_hash 比對（v1 trait 無 AST handle，
        // 且任何「結構變」都會改變 Node.id）。判定唯一依據 = canonical_node_id
        // 是否仍存在於最新快取 GraphOutput 的 node 集合中：
        //   - 節點消失（改名 / 檔案移動 / 刪除）→ Node.id 不再存在 → 自動銷案。
        //   - 節點仍在 → review 仍適用 → 維持 unresolved（不誤殺重構）。
        //
        // orphan 綁定（canonical_node_id 為空）不在此判定範圍（本來就沒綁定
        // 節點；由 review_resolve 手動處理）。此方法為 best-effort：任何
        // 失敗都靜默跳過（v1 契約：plugin 永不 panic）。
        let Some(graph) = self.graph() else { return };
        let live_ids: std::collections::HashSet<&str> =
            graph.nodes.iter().map(|n| n.id.0.as_str()).collect();
        let db = match self.db() {
            Ok(db) => db,
            Err(_) => return,
        };
        let bindings = match db.list_unresolved_non_orphan(&self.workspace_key) {
            Ok(rows) => rows,
            Err(_) => return,
        };
        let now = crate::sync::now_rfc3339();
        for binding in &bindings {
            if !live_ids.contains(binding.canonical_node_id.as_str()) {
                let _ = db.resolve(
                    &self.workspace_key,
                    &binding.id,
                    &now,
                    "auto:node_gone",
                    "canonical node no longer present in graph (renamed, moved, or removed)",
                    &now,
                );
            }
        }

        // Slice 2：Impact Guard — 以變動節點為種子做 BFS 衝擊半徑，命中
        // unresolved critical/high 綁定則產 ImpactAlert（design §8.1/8.2）。
        //
        // 種子來源：event.modified_nodes 優先；為空（graphify-mcp 兩處 hook
        // 目前都傳空 Vec）則用 prev/cur node-id diff 補位 — 與 Slice 1 的
        // 「Node.id 消失/變更」判定同語意，讓 CLI + MCP 兩路徑都能觸發。
        // best-effort：任何失敗靜默跳過，plugin 永不 panic。
        let seeds = self.impact_seeds(&graph, event);
        let alerts = crate::impact::detect_impact(&graph, &seeds, &db, &self.workspace_key);
        for alert in alerts {
            if let Ok(payload) = serde_json::to_value(&alert) {
                self.emit_notify(payload);
            }
        }
    }

    fn set_notify_callback(&mut self, cb: Option<NotifyCallback>) {
        self.notify_cb = cb;
    }
}

impl ReviewPlugin {
    /// v1.1 事件推送：把序列化 payload 交給 host 注入的 callback（若存在）。
    /// host（graphify-mcp）負責轉發；無 callback 時靜默跳過。
    pub(crate) fn emit_notify(&self, payload: serde_json::Value) {
        if let Some(cb) = &self.notify_cb {
            cb(payload);
        }
    }

    /// 計算 Slice 2 BFS 種子：event.modified_nodes 非空直接用；為空則以
    /// 本次 graph 與上次快照的 node-id 差集補位。回傳後更新快照。
    ///
    /// ponytail: mcp 兩處 hook 目前都傳空 modified_nodes，diff 是唯一能讓
    /// impact guard 在真實路徑觸發的種子來源；若日後 host 開始傳真實
    /// modified_nodes，diff 分支自然不再被走到，但保留作為 fallback。
    fn impact_seeds(&self, graph: &GraphOutput, event: &GraphUpdateEvent) -> Vec<graphify_core::NodeId> {
        if !event.modified_nodes.is_empty() {
            return event.modified_nodes.clone();
        }
        let cur: std::collections::HashSet<&str> =
            graph.nodes.iter().map(|n| n.id.0.as_str()).collect();
        let prev = self.prev_node_ids.read().ok();
        // 首次同步（prev 尚無 baseline）→ 只建立快照，不視為變動（避免
        // 初次載入就對所有 critical/high 綁定發出噪音 alert）。
        let seeds = match prev {
            Some(prev) if prev.is_empty() => Vec::new(),
            Some(prev) => prev
                .iter()
                .filter(|id| !cur.contains(id.as_str()))
                .map(|id| graphify_core::NodeId(id.clone()))
                .chain(
                    cur.iter()
                        .filter(|id| !prev.contains(**id))
                        .map(|id| graphify_core::NodeId(id.to_string())),
                )
                .collect(),
            None => Vec::new(),
        };
        // 更新快照（讀鎖已 drop）。
        if let Ok(mut prev) = self.prev_node_ids.write() {
            *prev = graph.nodes.iter().map(|n| n.id.0.clone()).collect();
        }
        seeds
    }

    /// 審查摘要（sync_toon plugin_data 用）：bound 數 + 未解決數。
    fn summary_json(&self) -> serde_json::Value {
        let db = self.db();
        let (bound, unresolved) = match db {
            Ok(db) => {
                let b = db.count(&self.workspace_key).unwrap_or(0);
                let u = db.count_unresolved(&self.workspace_key).unwrap_or(0);
                (b, u)
            }
            Err(_) => (0, 0),
        };
        serde_json::json!({
            "review": {
                "workspace_key": self.workspace_key,
                "bound": bound,
                "unresolved": unresolved,
                "plugin": PLUGIN_ID,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphify_core::plugin::WorkspaceContext;

    fn plugin_with_tmp_db() -> (tempfile::TempDir, ReviewPlugin) {
        let dir = tempfile::tempdir().unwrap();
        let plugin = ReviewPlugin::new()
            .with_registry_path(dir.path().join("graphify.db"))
            .with_crg_url("http://127.0.0.1:1/mcp");
        (dir, plugin)
    }

    #[test]
    fn plugin_id_and_workspace_key_roundtrip() {
        let mut p = ReviewPlugin::new();
        assert_eq!(p.get_id(), "graphify-plugin-review");
        assert_eq!(p.get_workspace_key(), "");
        let ctx = WorkspaceContext::new("w-abc", "review-demo", "/tmp/ws");
        p.bind(ctx);
        assert_eq!(p.get_workspace_key(), "w-abc");
    }

    #[test]
    fn severity_from_risk_maps_thresholds() {
        assert_eq!(severity_from_risk(0.9), "critical");
        assert_eq!(severity_from_risk(0.6), "high");
        assert_eq!(severity_from_risk(0.4), "medium");
        assert_eq!(severity_from_risk(0.1), "low");
        assert_eq!(severity_from_risk(0.0), "info");
    }

    #[test]
    fn search_crg_unbound_returns_no_repo_root() {
        let mut p = ReviewPlugin::new().with_crg_url("http://127.0.0.1:1/mcp");
        let err = p.review_search_crg(None).unwrap_err();
        assert!(matches!(err, ReviewSearchError::NoRepoRoot));
    }

    #[test]
    fn ingest_binds_lines_and_queries_context() {
        let (_d, p) = plugin_with_tmp_db();
        let mut p = p;
        let ctx = WorkspaceContext::new("w-1", "ws", "/tmp/ws");
        p.bind(ctx);

        // 先餵一張圖進快取（auth.rs 42 行在 verify_token 內）
        let toon = graphify_core::to_toon(&GraphOutput {
            nodes: vec![graphify_core::Node {
                id: graphify_core::NodeId(
                    "src/auth.rs:function:verify_token".to_string(),
                ),
                label: "verify_token".to_string(),
                file_type: graphify_core::FileType::Code,
                kind: "function".to_string(),
                language: "rust".to_string(),
                source_file: "src/auth.rs".to_string(),
                start_line: 30,
                end_line: 60,
                doc_comment: None,
                description: None,
                metadata: None,
            }],
            edges: Vec::new(),
            metadata: Default::default(),
        });
        let packet = p.sync_toon(Some(toon.into_bytes()));
        assert!(packet.starts_with(b"metadata:\n"));
        assert!(p.graph().is_some());

        let payload = IngestPayload {
            version: "1.0".to_string(),
            source: "code-review-graph".to_string(),
            workspace_key: "w-1".to_string(),
            reviews: vec![crate::ingest::ReviewItem {
                review_id: "crg-sec-001".to_string(),
                file_path: "src/auth.rs".to_string(),
                line_number: 42,
                severity: "high".to_string(),
                category: "security".to_string(),
                comment: "timing attack".to_string(),
                created_at: "2026-08-10T00:00:00Z".to_string(),
            }],
        };
        let (bound, orphan) = p.review_ingest(&payload).unwrap();
        assert_eq!((bound, orphan), (1, 0));

        let (node, rows) = p
            .review_get_context("w-1", "src/auth.rs:function:verify_token", true)
            .unwrap();
        assert_eq!(node, "src/auth.rs:function:verify_token");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "crg-sec-001");
        assert_eq!(rows[0].severity, "high");
    }

    #[test]
    fn ingest_orphan_line_not_binding() {
        let (_d, p) = plugin_with_tmp_db();
        let mut p = p;
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));

        // 無圖快取 → 全 orphan
        let payload = IngestPayload {
            version: "1.0".to_string(),
            source: "code-review-graph".to_string(),
            workspace_key: "w-1".to_string(),
            reviews: vec![crate::ingest::ReviewItem {
                review_id: "crg-002".to_string(),
                file_path: "src/missing.rs".to_string(),
                line_number: 10,
                severity: "medium".to_string(),
                category: "correctness".to_string(),
                comment: "x".to_string(),
                created_at: "2026-08-10T00:00:00Z".to_string(),
            }],
        };
        let (bound, orphan) = p.review_ingest(&payload).unwrap();
        assert_eq!((bound, orphan), (0, 1));

        // orphan 仍寫入（canonical 空）→ query 空 node 找得到
        let (_, rows) = p.review_get_context("w-1", "", true).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn resolve_marks_closed() {
        let (_d, p) = plugin_with_tmp_db();
        let mut p = p;
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));
        let payload = IngestPayload {
            version: "1.0".to_string(),
            source: "code-review-graph".to_string(),
            workspace_key: "w-1".to_string(),
            reviews: vec![crate::ingest::ReviewItem {
                review_id: "crg-003".to_string(),
                file_path: "src/auth.rs".to_string(),
                line_number: 42,
                severity: "low".to_string(),
                category: "style".to_string(),
                comment: "nit".to_string(),
                created_at: "2026-08-10T00:00:00Z".to_string(),
            }],
        };
        p.review_ingest(&payload).unwrap();

        assert!(p
            .review_resolve("w-1", "crg-003", "manual", "")
            .unwrap());
        assert!(!p.review_resolve("w-1", "nope", "manual", "").unwrap());

        let (_, rows) = p.review_get_context("w-1", "", true).unwrap();
        assert_eq!(rows.len(), 0, "resolved reviews no longer surface as unresolved");
    }

    /// 假 CRG server：對 `initialize` 回 `Mcp-Session-Id` header，對
    /// `detect_changes_tool` 回罐頭 `review_priorities`（1 筆）。
    /// 回傳 base URL（`http://127.0.0.1:{port}/mcp`）。
    fn fake_crg_server() -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let priorities = serde_json::json!({
            "status": "ok",
            "risk_score": 0.42,
            "changed_file_count": 1,
            "review_priorities": [{
                "name": "verify_token",
                "qualified_name": "crate::auth::verify_token",
                "file_path": "/tmp/ws/src/auth.rs",
                "line_start": 42,
                "line_end": 50,
                "risk_score": 0.4,
                "kind": "function",
            }],
        })
        .to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                // 讀 request head + body（依 Content-Length）
                let mut all = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            all.extend_from_slice(&chunk[..n]);
                            if let Some(hdr_end) =
                                all.windows(4).position(|w| w == b"\r\n\r\n")
                            {
                                let head =
                                    String::from_utf8_lossy(&all[..hdr_end]);
                                let clen: usize = head
                                    .lines()
                                    .find_map(|l| {
                                        l.to_ascii_lowercase()
                                            .strip_prefix("content-length:")
                                            .map(|v| v.trim().parse().unwrap_or(0))
                                    })
                                    .unwrap_or(0);
                                if all.len() >= hdr_end + 4 + clen {
                                    break;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
                let head = String::from_utf8_lossy(&all);
                let (status, body) = if head.contains("initialize") {
                    (
                        "200 OK",
                        r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake-crg","version":"0.0.0"}}}"#
                            .to_string(),
                    )
                } else if head.contains("detect_changes_tool") {
                    let text = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 2,
                        "result": { "content": [{ "type": "text", "text": priorities }] },
                    })
                    .to_string();
                    ("200 OK", text)
                } else {
                    ("404 Not Found", "{}".to_string())
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nMcp-Session-Id: ses-fake\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}/mcp")
    }

    #[test]
    fn search_crg_roundtrip_binds_and_returns_node_ids() {
        let url = fake_crg_server();
        let dir = tempfile::tempdir().unwrap();
        let mut p = ReviewPlugin::new()
            .with_registry_path(dir.path().join("graphify.db"))
            .with_crg_url(url);
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));
        // graph 快取：src/auth.rs:function:verify_token 涵蓋 line 42
        let toon =
            graphify_core::to_toon(&node_graph("src/auth.rs:function:verify_token"));
        p.sync_toon(Some(toon.into_bytes()));

        // search → 回實際綁定的 node ids（不再只回計數）
        let (node_ids, orphan) = p.review_search_crg(None).unwrap();
        assert_eq!(orphan, 0);
        assert_eq!(
            node_ids,
            vec!["src/auth.rs:function:verify_token".to_string()]
        );

        // round-trip：用回傳的 node id 讀回綁定
        let (node, rows) = p
            .review_get_context("w-1", &node_ids[0], true)
            .unwrap();
        assert_eq!(node, "src/auth.rs:function:verify_token");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "crg-src/auth.rs:42:verify_token");
        assert_eq!(rows[0].severity, "medium"); // risk 0.4
    }

    fn node_graph(node_id: &str) -> GraphOutput {
        GraphOutput {
            nodes: vec![graphify_core::Node {
                id: graphify_core::NodeId(node_id.to_string()),
                label: node_id.rsplit(':').next().unwrap_or("n").to_string(),
                file_type: graphify_core::FileType::Code,
                kind: "function".to_string(),
                language: "rust".to_string(),
                source_file: node_id
                    .rsplitn(3, ':')
                    .nth(2)
                    .unwrap_or("src/auth.rs")
                    .to_string(),
                start_line: 1,
                end_line: 50,
                doc_comment: None,
                description: None,
                metadata: None,
            }],
            edges: Vec::new(),
            metadata: Default::default(),
        }
    }

    /// 雙節點 + calls edge 圖：caller → callee（Slice 2 BFS 衝擊測試用）。
    fn caller_graph(caller: &str, callee: &str) -> GraphOutput {
        let mut g = node_graph(caller);
        let callee_node = graphify_core::Node {
            id: graphify_core::NodeId(callee.to_string()),
            label: callee.rsplit(':').next().unwrap_or("n").to_string(),
            file_type: graphify_core::FileType::Code,
            kind: "function".to_string(),
            language: "rust".to_string(),
            source_file: callee
                .rsplitn(3, ':')
                .nth(2)
                .unwrap_or("src/auth.rs")
                .to_string(),
            start_line: 1,
            end_line: 50,
            doc_comment: None,
            description: None,
            metadata: None,
        };
        g.nodes.push(callee_node);
        g.edges.push(graphify_core::Edge {
            source: graphify_core::NodeId(caller.to_string()),
            target: graphify_core::NodeId(callee.to_string()),
            relation: "calls".to_string(),
            source_file: "src/main.rs".to_string(),
            confidence: "high".to_string(),
            source_location: "src/main.rs:1".to_string(),
            description: None,
        });
        g
    }

    #[test]
    fn on_graph_updated_auto_resolves_drifted_node() {
        let (_d, p) = plugin_with_tmp_db();
        let mut p = p;
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));

        // 圖 v1：verify_token 存在 → ingest 綁定成功（1 bound / 0 orphan）
        let toon = graphify_core::to_toon(&node_graph("src/auth.rs:function:verify_token"));
        p.sync_toon(Some(toon.into_bytes()));
        let payload = IngestPayload {
            version: "1.0".to_string(),
            source: "code-review-graph".to_string(),
            workspace_key: "w-1".to_string(),
            reviews: vec![crate::ingest::ReviewItem {
                review_id: "crg-drift-001".to_string(),
                file_path: "src/auth.rs".to_string(),
                line_number: 42,
                severity: "high".to_string(),
                category: "security".to_string(),
                comment: "timing attack".to_string(),
                created_at: "2026-08-10T00:00:00Z".to_string(),
            }],
        };
        let (bound, orphan) = p.review_ingest(&payload).unwrap();
        assert_eq!((bound, orphan), (1, 0));

        // 圖 v2：節點改名（函數 rename）→ 舊 Node.id 消失
        let toon2 =
            graphify_core::to_toon(&node_graph("src/auth.rs:function:verify_authentication_token"));
        p.sync_toon(Some(toon2.into_bytes()));

        // on_graph_updated → presence diff → 自動銷案
        let event = graphify_core::GraphUpdateEvent::new(
            "w-1",
            Vec::new(),
            graphify_core::GraphUpdateKind::Indexed,
        );
        p.on_graph_updated(&event);

        let (_, rows) = p
            .review_get_context("w-1", "src/auth.rs:function:verify_token", true)
            .unwrap();
        assert!(rows.is_empty(), "drifted binding auto-resolved");
        let (_, orphan_rows) = p.review_get_context("w-1", "", true).unwrap();
        assert!(orphan_rows.is_empty(), "no orphan leakage from auto-resolve");
    }

    #[test]
    fn on_graph_updated_keeps_present_node_unresolved() {
        let (_d, p) = plugin_with_tmp_db();
        let mut p = p;
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));

        let toon = graphify_core::to_toon(&node_graph("src/auth.rs:function:verify_token"));
        p.sync_toon(Some(toon.into_bytes()));
        let payload = IngestPayload {
            version: "1.0".to_string(),
            source: "code-review-graph".to_string(),
            workspace_key: "w-1".to_string(),
            reviews: vec![crate::ingest::ReviewItem {
                review_id: "crg-keep-001".to_string(),
                file_path: "src/auth.rs".to_string(),
                line_number: 42,
                severity: "medium".to_string(),
                category: "correctness".to_string(),
                comment: "edge case".to_string(),
                created_at: "2026-08-10T00:00:00Z".to_string(),
            }],
        };
        p.review_ingest(&payload).unwrap();

        // 同一張圖再更新 → 節點仍在 → 不誤殺
        let toon2 = graphify_core::to_toon(&node_graph("src/auth.rs:function:verify_token"));
        p.sync_toon(Some(toon2.into_bytes()));
        let event = graphify_core::GraphUpdateEvent::new(
            "w-1",
            Vec::new(),
            graphify_core::GraphUpdateKind::Indexed,
        );
        p.on_graph_updated(&event);

        let (_, rows) = p
            .review_get_context("w-1", "src/auth.rs:function:verify_token", true)
            .unwrap();
        assert_eq!(rows.len(), 1, "present node stays unresolved");
    }

    /// Slice 2 baseline：首次 on_graph_updated（prev 快照為空）不視為變動，
    /// 即使存在 critical 綁定也不發 ImpactAlert（避免初次載入噪音）。
    #[test]
    fn first_sync_is_baseline_no_impact_alert() {
        let (_d, p) = plugin_with_tmp_db();
        let mut p = p;
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));

        let alerts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&alerts);
        p.set_notify_callback(Some(Box::new(move |payload| {
            sink.lock().expect("sink lock").push(payload);
        })));

        // 圖 v1：verify_token（critical 綁定）→ 首次同步 = baseline
        let toon = graphify_core::to_toon(&node_graph("src/auth.rs:function:verify_token"));
        p.sync_toon(Some(toon.into_bytes()));
        let payload = IngestPayload {
            version: "1.0".to_string(),
            source: "code-review-graph".to_string(),
            workspace_key: "w-1".to_string(),
            reviews: vec![crate::ingest::ReviewItem {
                review_id: "crg-base-001".to_string(),
                file_path: "src/auth.rs".to_string(),
                line_number: 42,
                severity: "critical".to_string(),
                category: "security".to_string(),
                comment: "hardcoded secret".to_string(),
                created_at: "2026-08-10T00:00:00Z".to_string(),
            }],
        };
        p.review_ingest(&payload).unwrap();

        let event = graphify_core::GraphUpdateEvent::new(
            "w-1",
            Vec::new(),
            graphify_core::GraphUpdateKind::Indexed,
        );
        p.on_graph_updated(&event);
        assert!(
            alerts.lock().expect("alerts lock").is_empty(),
            "first sync must not alert (baseline)"
        );
    }

    #[test]
    fn sync_toon_none_does_not_panic() {
        let (_d, p) = plugin_with_tmp_db();
        let mut p = p;
        p.bind(WorkspaceContext::new("w-9", "ws", "/tmp/ws"));
        let out = p.sync_toon(None);
        assert!(String::from_utf8_lossy(&out).contains("workspace_key"));
    }

    #[test]
    fn sync_toon_garbage_is_lenient_and_caches_empty_graph() {
        // from_toon 是全寬容 parser：garbage → 空 GraphOutput（不 panic）。
        let (_d, p) = plugin_with_tmp_db();
        let mut p = p;
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));
        let out = p.sync_toon(Some(b"not-a-toon".to_vec()));
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("workspace_key"), "summary packet expected: {text}");
        let g = p.graph().expect("garbage toon caches an empty graph");
        assert!(g.nodes.is_empty());
    }

    /// v1.1：host 注入 callback 後，emit_notify 的 payload 到達 host 端。
    #[test]
    fn notify_callback_reaches_host_with_payload() {
        let (_d, p) = plugin_with_tmp_db();
        let mut p = p;
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));

        let received = std::sync::Arc::new(std::sync::Mutex::new(None));
        let sink = std::sync::Arc::clone(&received);
        p.set_notify_callback(Some(Box::new(move |payload| {
            *sink.lock().expect("sink lock") = Some(payload);
        })));

        p.emit_notify(serde_json::json!({
            "event": "impact_alert",
            "severity": "critical",
            "node": "src/auth.rs:function:verify_token",
        }));

        let got = received.lock().expect("received lock");
        let got = got.as_ref().expect("payload must reach host");
        assert_eq!(got["event"], "impact_alert");
        assert_eq!(got["severity"], "critical");
    }

    /// v1.1：未注入 callback 時 emit_notify 靜默跳過（不 panic）。
    #[test]
    fn emit_notify_without_callback_is_noop() {
        let (_d, p) = plugin_with_tmp_db();
        let mut p = p;
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));
        p.emit_notify(serde_json::json!({ "event": "impact_alert" }));
    }

    /// Slice 2 e2e：caller 變動 → BFS 涵蓋 callee 上的 critical 綁定 →
    /// 透過 notify callback 發出 ImpactAlert。
    #[test]
    fn impact_alert_emitted_when_caller_changes() {
        let (_d, p) = plugin_with_tmp_db();
        let mut p = p;
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));

        let alerts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&alerts);
        p.set_notify_callback(Some(Box::new(move |payload| {
            sink.lock().expect("sink lock").push(payload);
        })));

        // 圖 v1（baseline）：main → calls → verify_token，critical 綁定
        let toon = graphify_core::to_toon(&caller_graph(
            "src/main.rs:function:main",
            "src/auth.rs:function:verify_token",
        ));
        p.sync_toon(Some(toon.into_bytes()));
        let payload = IngestPayload {
            version: "1.0".to_string(),
            source: "code-review-graph".to_string(),
            workspace_key: "w-1".to_string(),
            reviews: vec![crate::ingest::ReviewItem {
                review_id: "crg-impact-001".to_string(),
                file_path: "src/auth.rs".to_string(),
                line_number: 42,
                severity: "critical".to_string(),
                category: "security".to_string(),
                comment: "hardcoded secret".to_string(),
                created_at: "2026-08-10T00:00:00Z".to_string(),
            }],
        };
        p.review_ingest(&payload).unwrap();
        p.on_graph_updated(&graphify_core::GraphUpdateEvent::new(
            "w-1",
            Vec::new(),
            graphify_core::GraphUpdateKind::Indexed,
        ));
        assert!(alerts.lock().expect("alerts lock").is_empty(), "baseline: no alert");

        // 圖 v2：新 caller 出現（main2 → calls → verify_token）→ 種子變動
        let toon2 = graphify_core::to_toon(&caller_graph(
            "src/admin.rs:function:admin_login",
            "src/auth.rs:function:verify_token",
        ));
        p.sync_toon(Some(toon2.into_bytes()));
        p.on_graph_updated(&graphify_core::GraphUpdateEvent::new(
            "w-1",
            Vec::new(),
            graphify_core::GraphUpdateKind::Indexed,
        ));

        let got = alerts.lock().expect("alerts lock");
        assert_eq!(got.len(), 1, "exactly one impact alert expected");
        let alert = got.first().expect("alert");
        assert_eq!(alert["max_severity"], "critical");
        assert!(
            serde_json::to_string(alert).unwrap().contains("verify_token"),
            "impacted node must be in payload: {alert}"
        );
    }
}
