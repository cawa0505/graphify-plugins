//! graphify-plugin-telemetry — 純 bridge（Draco MCP 可觀測性點位 → Graphify
//! canonical AST node id 升維綁定）。
//!
//! 不重造 Observability 引擎：效能數據 100% 來自 Draco MCP / file-based
//! IngestPayload import（Slice 0）；本 plugin 負責「行號 → canonical node id」
//! 升維、telemetry_bindings 持久化（併入 graphify.db）、Critical Hotspot
//! 門檻判定與 §6 可觀測性區塊合成。Slice 1/2 再接 Draco MCP 即時抓取
//! （`draco_client`）與 BFS 衝擊半徑 hotspot alert 廣播。
//!
//! 對齊規則（#3115、#3119、#3508）：`get_id`/`bind`/`get_workspace_key`/
//! `sync_toon`/`on_graph_updated` 為 core v1 trait 方法；業務 API
//! （`telemetry_ingest` / `telemetry_get_context`）為公開同步方法，非 trait
//! 方法（graphify-mcp auto-register 對應工具）。狀態與業務邏輯集中在
//! [`TelemetryService`]（telemetry.rs），本檔為薄 trait 殼。

use std::path::Path;

use graphify_core::plugin::{GraphUpdateEvent, GraphifyPlugin, WorkspaceContext};
use graphify_core::GraphOutput;

use crate::ingest::IngestError;
use crate::registry::TelemetryBinding;
use crate::telemetry::{IngestReport, TelemetryService};

pub mod draco_client;
pub mod ingest;
pub mod registry;
pub mod resolver;
pub mod sync;
pub mod telemetry;

/// plugin 唯一識別（graphify-mcp auto-register 的 id 前綴）。
pub const PLUGIN_ID: &str = "graphify-plugin-telemetry";

/// telemetry plugin 狀態（薄殼；全部狀態與邏輯委派至 [`TelemetryService`]）。
#[derive(Debug)]
pub struct TelemetryPlugin {
    service: TelemetryService,
}

impl Default for TelemetryPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl TelemetryPlugin {
    /// 預設建構；以 [`graphify_registry::registry_db_path`] 為 db 路徑。
    #[must_use]
    pub fn new() -> Self {
        Self {
            service: TelemetryService::new(),
        }
    }

    /// 覆寫 registry db 路徑（測試注入）。
    #[must_use]
    pub fn with_registry_path(mut self, path: std::path::PathBuf) -> Self {
        self.service = self.service.with_registry_path(path);
        self
    }

    /// 覆寫 Draco MCP base url（Slice 1/2 用；測試注入）。
    #[must_use]
    pub fn with_draco_url(mut self, url: impl Into<String>) -> Self {
        self.service = self.service.with_draco_url(url);
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

    /// 目前快取的 GraphOutput（無則 `None`）。
    #[must_use]
    pub fn graph(&self) -> Option<GraphOutput> {
        self.service.graph()
    }

    /// 以快取的 GraphOutput 執行 line→symbol 升維。
    #[must_use]
    pub fn resolve(&self, file_path: &str, line: u32) -> Option<crate::resolver::Resolved> {
        self.service.resolve(file_path, line)
    }

    // ---- 業務 API（graphify-mcp auto-register 對應的工具）----

    /// `telemetry_ingest`：讀取 Draco TelemetryIngestPayload JSON 檔案，
    /// 升維綁定至 canonical node id、評估 is_hotspot 後寫入 graphify.db。
    ///
    /// 回傳 [`IngestReport`]（total / bound / orphan / hotspots）。
    ///
    /// # Errors
    /// 檔案讀取/解析失敗回傳 [`crate::ingest::IngestError`]；db 寫入失敗
    /// 亦以 [`crate::ingest::IngestError::Db`] 回傳。
    pub fn telemetry_ingest_file(&self, path: &Path) -> Result<IngestReport, IngestError> {
        self.service.telemetry_ingest_file(path)
    }

    /// `telemetry_ingest`（source = "draco-mcp"）：呼叫 Draco
    /// `fetch_top_hotspots()` 一鍵同步當前 Cluster 的 Top 熱點。
    ///
    /// # Errors
    /// Draco 呼叫/契約解析失敗回傳 [`crate::ingest::IngestError::Draco`]；
    /// db 寫入失敗回傳 [`crate::ingest::IngestError::Db`]。
    pub fn telemetry_ingest_draco(
        &self,
        limit: Option<usize>,
    ) -> Result<IngestReport, IngestError> {
        self.service.telemetry_ingest_draco(limit)
    }

    /// `telemetry_ingest`（已解析 payload 版本；測試與程式化呼叫用）。
    ///
    /// # Errors
    /// db 寫入失敗回傳 [`crate::ingest::IngestError::Db`]。
    pub fn telemetry_ingest(
        &self,
        payload: &ingest::TelemetryIngestPayload,
    ) -> Result<IngestReport, IngestError> {
        self.service.telemetry_ingest(payload)
    }

    /// `telemetry_get_context`：查詢指定 canonical node 的效能綁定。
    ///
    /// `include_impact_radius` 目前保留參數（Slice 2 實作 BFS 衝擊半徑）；
    /// 現行實作為直查該 node。回傳 `(node_id, bindings)`。
    ///
    /// # Errors
    /// db 查詢失敗回傳 [`rusqlite::Error`]。
    pub fn telemetry_get_context(
        &self,
        workspace_key: &str,
        node_id: &str,
        include_impact_radius: bool,
    ) -> Result<(String, Vec<TelemetryBinding>), rusqlite::Error> {
        self.service
            .telemetry_get_context(workspace_key, node_id, include_impact_radius)
    }
}

impl GraphifyPlugin for TelemetryPlugin {
    fn get_id(&self) -> &str {
        PLUGIN_ID
    }

    fn bind(&mut self, ctx: WorkspaceContext) {
        self.service.set_workspace_key(ctx.workspace_key);
    }

    fn get_workspace_key(&self) -> &str {
        self.service.workspace_key()
    }

    fn sync_toon(&mut self, opt_toon: Option<Vec<u8>>) -> Vec<u8> {
        self.service.sync_toon(opt_toon)
    }

    fn on_graph_updated(&mut self, event: &GraphUpdateEvent) {
        self.service.on_graph_updated(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphify_core::plugin::WorkspaceContext;

    fn plugin_with_tmp_db() -> (tempfile::TempDir, TelemetryPlugin) {
        let dir = tempfile::tempdir().unwrap();
        let plugin = TelemetryPlugin::new()
            .with_registry_path(dir.path().join("graphify.db"))
            .with_draco_url("http://127.0.0.1:1/mcp");
        (dir, plugin)
    }

    #[test]
    fn plugin_id_and_workspace_key_roundtrip() {
        let mut p = TelemetryPlugin::new();
        assert_eq!(p.get_id(), "graphify-plugin-telemetry");
        assert_eq!(p.get_workspace_key(), "");
        let ctx = WorkspaceContext::new("w-abc", "telemetry-demo", "/tmp/ws");
        p.bind(ctx);
        assert_eq!(p.get_workspace_key(), "w-abc");
    }

    #[test]
    fn bind_for_cli_derives_workspace_key() {
        let p = TelemetryPlugin::new().bind_for_cli("/tmp/some-workspace");
        // derive_workspace_key 以路徑產出穩定 key（hash），非目錄名本身。
        assert!(!p.get_workspace_key().is_empty());
    }

    #[test]
    fn ingest_binds_lines_and_queries_context() {
        let (_d, p) = plugin_with_tmp_db();
        let mut p = p;
        let ctx = WorkspaceContext::new("w-1", "ws", "/tmp/ws");
        p.bind(ctx);

        // 先餵一張圖進快取（query.rs 88 行在 query_users 內）
        let toon = graphify_core::to_toon(&GraphOutput {
            nodes: vec![graphify_core::Node {
                id: graphify_core::NodeId(
                    "src/db/query.rs:function:query_users".to_string(),
                ),
                label: "query_users".to_string(),
                file_type: graphify_core::FileType::Code,
                kind: "function".to_string(),
                language: "rust".to_string(),
                source_file: "src/db/query.rs".to_string(),
                start_line: 80,
                end_line: 120,
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

        let payload = crate::ingest::TelemetryIngestPayload {
            version: "1.0".to_string(),
            source: "draco-mcp".to_string(),
            workspace_key: "w-1".to_string(),
            metrics: vec![crate::ingest::Metric {
                metric_id: "tel-p99-001".to_string(),
                file_path: "src/db/query.rs".to_string(),
                function_name: "query_users".to_string(),
                line_number: 88,
                p50_ms: 12.5,
                p99_ms: 1250.0,
                alloc_bytes_per_req: 10_485_760,
                call_count_per_min: 5000,
                environment: "production".to_string(),
                recorded_at: "2026-08-10T06:00:00Z".to_string(),
            }],
        };
        let report = p.telemetry_ingest(&payload).unwrap();
        assert_eq!(report.bound, 1);
        assert_eq!(report.orphan, 0);
        assert_eq!(report.hotspots, 1);

        let (node, rows) = p
            .telemetry_get_context("w-1", "src/db/query.rs:function:query_users", true)
            .unwrap();
        assert_eq!(node, "src/db/query.rs:function:query_users");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "tel-p99-001");
        assert_eq!(rows[0].p99_ms, 1250.0);
        assert!(rows[0].is_hotspot);
    }

    #[test]
    fn sync_toon_packet_carries_hotspot_block() {
        let (_d, p) = plugin_with_tmp_db();
        let mut p = p;
        p.bind(WorkspaceContext::new("w-1", "ws", "/tmp/ws"));
        let toon = graphify_core::to_toon(&GraphOutput {
            nodes: vec![graphify_core::Node {
                id: graphify_core::NodeId(
                    "src/payment/stripe.rs:function:process_payment".to_string(),
                ),
                label: "process_payment".to_string(),
                file_type: graphify_core::FileType::Code,
                kind: "function".to_string(),
                language: "rust".to_string(),
                source_file: "src/payment/stripe.rs".to_string(),
                start_line: 10,
                end_line: 40,
                doc_comment: None,
                description: None,
                metadata: None,
            }],
            edges: Vec::new(),
            metadata: Default::default(),
        });
        let _ = p.sync_toon(Some(toon.into_bytes()));

        let payload = crate::ingest::TelemetryIngestPayload {
            version: "1.0".to_string(),
            source: "draco-mcp".to_string(),
            workspace_key: "w-1".to_string(),
            metrics: vec![crate::ingest::Metric {
                metric_id: "tel-p99-002".to_string(),
                file_path: "src/payment/stripe.rs".to_string(),
                function_name: "process_payment".to_string(),
                line_number: 20,
                p50_ms: 88.0,
                p99_ms: 2450.0,
                alloc_bytes_per_req: 1024,
                call_count_per_min: 1200,
                environment: "production".to_string(),
                recorded_at: "2026-08-10T06:00:00Z".to_string(),
            }],
        };
        let _ = p.telemetry_ingest(&payload).unwrap();

        let out = p.sync_toon(None);
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("[src/payment/stripe.rs:function:process_payment (AST Node)]"),
            "hotspot block expected in packet: {text}"
        );
        assert!(text.contains("CRITICAL HOTSPOT"));
    }

    #[test]
    fn sync_toon_none_does_not_panic() {
        let (_d, p) = plugin_with_tmp_db();
        let mut p = p;
        p.bind(WorkspaceContext::new("w-9", "ws", "/tmp/ws"));
        let out = p.sync_toon(None);
        assert!(String::from_utf8_lossy(&out).contains("workspace_key"));
    }
}
