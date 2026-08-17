//! telemetry 業務邏輯（§5.2 對應 `telemetry_ingest` / `telemetry_get_context`
//! 與 §5.1 sync_toon 的 hotspot 合成）。
//!
//! `TelemetryService` 持有 plugin 狀態（workspace_key、registry_path、
//! graph_cache、Draco client），實作：
//! - 檔案型 ingest：解析 → line→symbol 升維 → is_hotspot 評估 → 寫入
//!   graphify.db（telemetry_bindings）。
//! - 查詢：依 canonical node id 取其效能綁定。
//! - hotspot 合成：is_hotspot 綁定 → HotspotView → §6 可觀測性區塊。
//!
//! 門檻（動態設定，預設 500ms / 5MB，見 [`ThresholdConfig`]）：p99 > 500ms
//! 或 alloc/req > 5MB 即標 hotspot。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use graphify_core::plugin::GraphUpdateEvent;
use graphify_core::{from_toon, GraphOutput};

use crate::draco_client::{default_draco_url, DracoMcpClient};
use crate::ingest::{parse_payload_file, IngestError, Metric, TelemetryIngestPayload};
use crate::registry::{TelemetryBinding, TelemetryDb};
use crate::resolver::{resolve_line, resolve_symbol};
use crate::sync::{emit_error_packet, emit_packet, synthesize_hotspot_block, HotspotView};

/// 動態熱點門檻（Slice 1：動態門檻設定）。
///
/// 預設值採 Slice 1 roadmap 明示範例 `p99 > 500ms || alloc > 5MB`，取代
/// schema §3.2 註解的 `1000ms`（原規格兩處衝突，Slice 1 定案為 500ms）。
/// 可經由環境變數 `TELEMETRY_HOTSPOT_P99_MS`（f64 毫秒）與
/// `TELEMETRY_HOTSPOT_ALLOC_BYTES`（i64 位元組）覆寫 — 與 `DRACO_BASE_URL`
/// 同款動態設定慣例（啟動時讀取，非程序級秘密持久化）。
///
/// ponytail: 全域 env 門檻，per-workspace 設定（存 registry）於需要
/// workspace 級差異時再引入。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThresholdConfig {
    /// p99 延遲門檻（毫秒）。
    pub p99_ms: f64,
    /// 每次請求記憶體配置門檻（bytes）。
    pub alloc_bytes: i64,
}

impl Default for ThresholdConfig {
    fn default() -> Self {
        Self {
            p99_ms: 500.0,
            alloc_bytes: 5 * 1024 * 1024,
        }
    }
}

impl ThresholdConfig {
    /// 從環境變數建構；未設定（或解析失敗）時回退預設值。
    #[must_use]
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(v) = std::env::var("TELEMETRY_HOTSPOT_P99_MS") {
            if let Ok(ms) = v.trim().parse::<f64>() {
                cfg.p99_ms = ms;
            }
        }
        if let Ok(v) = std::env::var("TELEMETRY_HOTSPOT_ALLOC_BYTES") {
            if let Ok(b) = v.trim().parse::<i64>() {
                cfg.alloc_bytes = b;
            }
        }
        cfg
    }

    /// 評估單筆指標是否為 Critical Hotspot：
    /// `p99_ms > self.p99_ms`（延遲）或 `alloc_bytes > self.alloc_bytes`
    /// （記憶體）任一超過即為 hotspot。
    #[must_use]
    pub fn is_hotspot(self, p99_ms: f64, alloc_bytes: i64) -> bool {
        p99_ms > self.p99_ms || alloc_bytes > self.alloc_bytes
    }
}

/// 一次 ingest 的結果摘要。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngestReport {
    /// 處理的指標總數。
    pub total: usize,
    /// 成功升維綁定至 canonical node 數。
    pub bound: usize,
    /// 找不到對應節點（未索引/行號超界）數。
    pub orphan: usize,
    /// 依門檻判定為 hotspot 的指標數。
    pub hotspots: usize,
}

/// telemetry plugin 業務服務（狀態 + 業務 API；`lib.rs` 的
/// `TelemetryPlugin` 為薄 trait 殼，狀態與邏輯都在此）。
#[derive(Debug)]
pub struct TelemetryService {
    workspace_key: String,
    /// 覆寫 graphify.db 路徑（測試注入用）；`None` = 預設 XDG 路徑。
    registry_path: Option<PathBuf>,
    /// 記憶體 GraphOutput 快取（sync_toon 填入；resolver 使用）。
    graph_cache: RwLock<Option<GraphOutput>>,
    /// Draco MCP client 骨架（Slice 1/2 接真呼叫）。
    draco_client: DracoMcpClient,
    /// Hotspot 判定門檻（動態設定，env 可覆寫）。
    threshold: ThresholdConfig,
}

impl Default for TelemetryService {
    fn default() -> Self {
        Self::new()
    }
}

impl TelemetryService {
    /// 預設建構；以 [`graphify_registry::registry_db_path`] 為 db 路徑。
    #[must_use]
    pub fn new() -> Self {
        Self {
            workspace_key: String::new(),
            registry_path: None,
            graph_cache: RwLock::new(None),
            draco_client: DracoMcpClient::new(default_draco_url()),
            threshold: ThresholdConfig::from_env(),
        }
    }

    /// 覆寫 registry db 路徑（測試注入）。
    #[must_use]
    pub fn with_registry_path(mut self, path: PathBuf) -> Self {
        self.registry_path = Some(path);
        self
    }

    /// 覆寫 Draco MCP base url（Slice 1/2 用；測試注入）。
    #[must_use]
    pub fn with_draco_url(mut self, url: impl Into<String>) -> Self {
        self.draco_client = DracoMcpClient::new(url);
        self
    }

    /// 覆寫 hotspot 判定門檻（動態設定；測試注入）。
    #[must_use]
    pub fn with_threshold(mut self, threshold: ThresholdConfig) -> Self {
        self.threshold = threshold;
        self
    }

    /// bind 時設定 workspace_key。
    pub fn set_workspace_key(&mut self, workspace_key: String) {
        self.workspace_key = workspace_key;
    }

    /// 目前 workspace_key。
    #[must_use]
    pub fn workspace_key(&self) -> &str {
        &self.workspace_key
    }

    fn registry_path(&self) -> PathBuf {
        self.registry_path
            .clone()
            .unwrap_or_else(graphify_registry::registry_db_path)
    }

    fn db(&self) -> Result<TelemetryDb, rusqlite::Error> {
        TelemetryDb::open(&self.registry_path())
    }

    /// 目前快取的 GraphOutput（無則 `None`）。
    #[must_use]
    pub fn graph(&self) -> Option<GraphOutput> {
        self.graph_cache.read().ok()?.clone()
    }

    /// 以快取的 GraphOutput 執行 line→symbol 升維。
    #[must_use]
    pub fn resolve(&self, file_path: &str, line: u32) -> Option<crate::resolver::Resolved> {
        self.graph()
            .and_then(|g| resolve_line(&g, file_path, line))
    }

    // ---- 業務 API（graphify-mcp auto-register 對應的工具）----

    /// `telemetry_ingest`：讀取 TelemetryIngestPayload JSON 檔案，將每筆
    /// 指標升維綁定至 canonical node id、評估 is_hotspot 後寫入 graphify.db。
    ///
    /// # Errors
    /// 檔案讀取/解析失敗回傳 [`crate::ingest::IngestError`]；db 寫入失敗
    /// 亦以 [`crate::ingest::IngestError::Db`] 回傳。
    pub fn telemetry_ingest_file(&self, path: &Path) -> Result<IngestReport, IngestError> {
        let payload = parse_payload_file(path)?;
        self.telemetry_ingest(&payload)
    }

    /// `telemetry_ingest`（已解析 payload 版本；測試與程式化呼叫用）。
    ///
    /// # Errors
    /// db 寫入失敗回傳 [`crate::ingest::IngestError::Db`]。
    pub fn telemetry_ingest(
        &self,
        payload: &TelemetryIngestPayload,
    ) -> Result<IngestReport, IngestError> {
        let graph = self.graph();
        let db = self.db()?;
        let now = crate::sync::now_rfc3339();
        let mut bound = 0usize;
        let mut orphan = 0usize;
        let mut hotspots = 0usize;

        for metric in &payload.metrics {
            // line 未知（Draco 只給 symbol）時退到 symbol 解析；有行號仍
            // 以 line→symbol（最內層 span）為準。
            let canonical = graph
                .as_ref()
                .and_then(|g| {
                    if metric.line_number > 0 {
                        resolve_line(g, &metric.file_path, metric.line_number)
                    } else {
                        resolve_symbol(g, &metric.file_path, &metric.function_name)
                    }
                })
                .map(|r| r.node_id.0)
                .unwrap_or_default();

            if canonical.is_empty() {
                orphan += 1;
            } else {
                bound += 1;
            }

            let hot = self.threshold.is_hotspot(metric.p99_ms, metric.alloc_bytes_per_req);
            if hot {
                hotspots += 1;
            }

            db.upsert(&TelemetryBinding {
                workspace_key: payload.workspace_key.clone(),
                id: metric.metric_id.clone(),
                canonical_node_id: canonical,
                file_path: metric.file_path.clone(),
                function_name: metric.function_name.clone(),
                p50_ms: metric.p50_ms,
                p99_ms: metric.p99_ms,
                alloc_bytes: metric.alloc_bytes_per_req,
                call_count: metric.call_count_per_min,
                is_hotspot: hot,
                environment: metric.environment.clone(),
                created_at: metric.recorded_at.clone(),
                updated_at: now.clone(),
            })?;
        }
        Ok(IngestReport {
            total: payload.metrics.len(),
            bound,
            orphan,
            hotspots,
        })
    }

    /// `telemetry_ingest`（source = "draco-mcp"）：呼叫 Draco
    /// `fetch_top_hotspots()`，將 server 聚合的 Top 熱點轉譯為
    /// IngestPayload 後走同一條 ingest 管線（symbol 解析 + 門檻 + 寫入）。
    ///
    /// metric_id 由 `file_path:function_name` 安定雜湊派生
    /// （`tel-draco-{hash}`）——重複同步會 upsert 同一列，不產生重複
    /// binding。workspace_key 用 plugin 綁定的 key（與 review 的教訓
    /// 一致：bindings 不信任 payload 外來 key）。
    ///
    /// # Errors
    /// Draco 呼叫 / 契約解析 / db 寫入失敗回傳
    /// [`crate::ingest::IngestError`]。
    pub fn telemetry_ingest_draco(
        &self,
        limit: Option<usize>,
    ) -> Result<IngestReport, IngestError> {
        let mut client = self.draco_client.clone();
        let hotspots = client.fetch_top_hotspots(limit)?;
        let now = crate::sync::now_rfc3339();
        let wk = self.workspace_key.clone();
        let metrics: Vec<Metric> = hotspots
            .into_iter()
            .map(|h| Metric {
                metric_id: draco_metric_id(&h.file_path, &h.function_name),
                file_path: h.file_path,
                function_name: h.function_name,
                line_number: 0,
                p50_ms: 0.0,
                p99_ms: h.p99_latency_ms,
                alloc_bytes_per_req: h.alloc_bytes,
                call_count_per_min: i64::try_from(h.call_count).unwrap_or(i64::MAX),
                environment: h.environment,
                recorded_at: now.clone(),
            })
            .collect();
        let payload = TelemetryIngestPayload {
            version: "1.0".to_string(),
            source: "draco-mcp".to_string(),
            workspace_key: wk,
            metrics,
        };
        self.telemetry_ingest(&payload)
    }

    /// `telemetry_get_context`：查詢指定 canonical node 的效能綁定。
    ///
    /// `include_impact_radius` 目前保留參數（Slice 2 實作 BFS 衝擊半徑，
    /// 依 §5.2 列出 Upstream callers 的 p99 / Memory 數據）；現行實作為
    /// 直查該 node。回傳 `(node_id, bindings)`。
    ///
    /// # Errors
    /// db 查詢失敗回傳 [`rusqlite::Error`]。
    pub fn telemetry_get_context(
        &self,
        workspace_key: &str,
        node_id: &str,
        _include_impact_radius: bool,
    ) -> Result<(String, Vec<TelemetryBinding>), rusqlite::Error> {
        let db = self.db()?;
        let rows = db.query_by_node(workspace_key, node_id)?;
        Ok((node_id.to_string(), rows))
    }

    /// 目前 workspace 的全部 hotspot（is_hotspot = 1），依 p99 降冪。
    ///
    /// # Errors
    /// db 查詢失敗回傳 [`rusqlite::Error`]。
    pub fn hotspots(&self) -> Result<Vec<HotspotView>, rusqlite::Error> {
        let db = self.db()?;
        Ok(db
            .query_hotspots(&self.workspace_key)?
            .into_iter()
            .map(|b| HotspotView {
                node_id: b.canonical_node_id,
                p99_ms: b.p99_ms,
                alloc_bytes: b.alloc_bytes,
                call_count: b.call_count,
                environment: b.environment,
            })
            .collect())
    }

    /// 效能摘要（sync_toon plugin_data 用）：綁定數 + hotspot 數 + §6
    /// 可觀測性區塊（`toon_block`）。db 不可用時降級為零值（不 panic）。
    #[must_use]
    pub fn summary_json(&self) -> serde_json::Value {
        let (bindings, hotspots, toon_block) = match self.db() {
            Ok(db) => {
                let b = db.count(&self.workspace_key).unwrap_or(0);
                let h = db.count_hotspots(&self.workspace_key).unwrap_or(0);
                let views = self.hotspots().unwrap_or_default();
                let block = if views.is_empty() {
                    String::new()
                } else {
                    synthesize_hotspot_block(&views)
                };
                (b, h, block)
            }
            Err(_) => (0, 0, String::new()),
        };
        serde_json::json!({
            "telemetry": {
                "workspace_key": self.workspace_key,
                "bindings": bindings,
                "hotspots": hotspots,
                "plugin": crate::PLUGIN_ID,
                "toon_block": toon_block,
            }
        })
    }

    // ---- GraphifyPlugin trait 支援（lib.rs 委派至此）----

    /// `sync_toon`：被動（Some）收 .toon 快取 GraphOutput、回 hotspot 摘要
    /// 封包；主動（None）回目前 workspace 摘要。解析失敗回 error 封包。
    #[must_use]
    pub fn sync_toon(&self, opt_toon: Option<Vec<u8>>) -> Vec<u8> {
        match opt_toon {
            Some(toon_bytes) => {
                let raw = String::from_utf8_lossy(&toon_bytes);
                match from_toon(&raw) {
                    Ok(graph) => {
                        *self.graph_cache.write().unwrap() = Some(graph);
                        emit_packet(&self.workspace_key, &self.summary_json()).into_bytes()
                    }
                    Err(_) => emit_error_packet("Cannot parse .toon into GraphOutput.").into_bytes(),
                }
            }
            None => emit_packet(&self.workspace_key, &self.summary_json()).into_bytes(),
        }
    }

    /// `on_graph_updated`：Slice 2 實作 hotspot impact guard（BFS 衝擊半徑 +
    /// notifications/telemetry/hotspot_alert 推送）。目前為 no-op，core 預設
    /// 實作亦為 no-op，符合 v1 相容。
    pub fn on_graph_updated(&self, _event: &GraphUpdateEvent) {
        // Slice 2：若 delta.modified_nodes 含 is_hotspot 節點 → 廣播 alert。
    }
}

/// Draco 熱點的安定 metric id：`tel-draco-{DefaultHasher(file:func):x}`。
/// DefaultHasher（SipHash13，固定 key）跨 run 確定；同一 symbol 重複同步
/// 會更新同一列（upsert 同 PK），不產生重複 binding。
fn draco_metric_id(file_path: &str, function_name: &str) -> String {
    let mut hasher = DefaultHasher::new();
    file_path.hash(&mut hasher);
    function_name.hash(&mut hasher);
    format!("tel-draco-{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::Metric;

    fn service_with_tmp_db() -> (tempfile::TempDir, TelemetryService) {
        let dir = tempfile::tempdir().unwrap();
        let svc = TelemetryService::new()
            .with_registry_path(dir.path().join("graphify.db"))
            .with_draco_url("http://127.0.0.1:1/mcp");
        (dir, svc)
    }

    fn feed_graph(svc: &TelemetryService) {
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
        let _ = svc.sync_toon(Some(toon.into_bytes()));
    }

    fn metric(id: &str, p99: f64, alloc: i64) -> Metric {
        Metric {
            metric_id: id.to_string(),
            file_path: "src/db/query.rs".to_string(),
            function_name: "query_users".to_string(),
            line_number: 88,
            p50_ms: 12.5,
            p99_ms: p99,
            alloc_bytes_per_req: alloc,
            call_count_per_min: 5000,
            environment: "production".to_string(),
            recorded_at: "2026-08-10T06:00:00Z".to_string(),
        }
    }

    fn payload(ws: &str, metrics: Vec<Metric>) -> TelemetryIngestPayload {
        TelemetryIngestPayload {
            version: "1.0".to_string(),
            source: "draco-mcp".to_string(),
            workspace_key: ws.to_string(),
            metrics,
        }
    }

    #[test]
    fn hotspot_thresholds() {
        // 預設門檻（Slice 1 定案）：p99 > 500ms || alloc > 5MB。
        let def = ThresholdConfig::default();
        assert_eq!(def.p99_ms, 500.0);
        assert_eq!(def.alloc_bytes, 5 * 1024 * 1024);
        assert!(!def.is_hotspot(100.0, 1024));
        assert!(def.is_hotspot(501.0, 1024), "p99 over 500ms");
        assert!(!def.is_hotspot(500.0, 1024), "p99 must exceed 500ms");
        assert!(def.is_hotspot(100.0, 5 * 1024 * 1024 + 1), "alloc over 5MB");
        assert!(!def.is_hotspot(100.0, 5 * 1024 * 1024), "alloc must exceed 5MB");

        // 自訂門檻（動態設定可覆寫）。
        let custom = ThresholdConfig { p99_ms: 1000.0, alloc_bytes: 1024 };
        assert!(!custom.is_hotspot(600.0, 1024), "600ms 低於自訂 1000ms");
        assert!(custom.is_hotspot(100.0, 2048), "alloc 超過自訂 1KB");
    }

    #[test]
    fn ingest_respects_custom_threshold() {
        let (_d, mut svc) = service_with_tmp_db();
        svc = svc.with_threshold(ThresholdConfig { p99_ms: 1000.0, alloc_bytes: 1024 });
        feed_graph(&svc);
        // 600ms：低於自訂 1000ms → 非 hotspot。
        let report = svc
            .telemetry_ingest(&payload("w-1", vec![metric("tel-1", 600.0, 1024)]))
            .unwrap();
        assert_eq!(
            report,
            IngestReport { total: 1, bound: 1, orphan: 0, hotspots: 0 }
        );
        // 2KB alloc：超過自訂 1KB → hotspot。
        let report = svc
            .telemetry_ingest(&payload("w-1", vec![metric("tel-2", 100.0, 2048)]))
            .unwrap();
        assert_eq!(
            report,
            IngestReport { total: 1, bound: 1, orphan: 0, hotspots: 1 }
        );
    }

    #[test]
    fn ingest_binds_and_queries_context() {
        let (_d, svc) = service_with_tmp_db();
        feed_graph(&svc);
        let report = svc.telemetry_ingest(&payload("w-1", vec![metric("tel-1", 1250.0, 1024)]))
            .unwrap();
        assert_eq!(
            report,
            IngestReport {
                total: 1,
                bound: 1,
                orphan: 0,
                hotspots: 1
            }
        );

        let (node, rows) = svc
            .telemetry_get_context("w-1", "src/db/query.rs:function:query_users", true)
            .unwrap();
        assert_eq!(node, "src/db/query.rs:function:query_users");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "tel-1");
        assert!(rows[0].is_hotspot);
        assert_eq!(rows[0].p99_ms, 1250.0);
    }

    #[test]
    fn non_hotspot_metric_not_in_summary() {
        let (_d, mut svc) = service_with_tmp_db();
        svc.set_workspace_key("w-1".to_string());
        feed_graph(&svc);
        svc.telemetry_ingest(&payload("w-1", vec![metric("tel-2", 12.5, 1024)]))
            .unwrap();
        let summary = svc.summary_json();
        assert_eq!(summary["telemetry"]["bindings"], 1);
        assert_eq!(summary["telemetry"]["hotspots"], 0);
        assert_eq!(summary["telemetry"]["toon_block"], "");
        assert!(svc.hotspots().unwrap().is_empty());
    }

    #[test]
    fn hotspot_synthesized_into_toon_block() {
        let (_d, mut svc) = service_with_tmp_db();
        svc.set_workspace_key("w-1".to_string());
        feed_graph(&svc);
        svc.telemetry_ingest(&payload("w-1", vec![metric("tel-3", 2450.0, 1024 * 1024 * 15)]))
            .unwrap();
        let summary = svc.summary_json();
        let block = summary["telemetry"]["toon_block"].as_str().unwrap();
        assert!(block.contains("[src/db/query.rs:function:query_users (AST Node)]"));
        assert!(block.contains("⚡ [Draco Telemetry] p99 Latency: 2,450ms (CRITICAL HOTSPOT)"));
        assert!(block.contains("📊 Alloc: 15.0MB | Calls: 5,000/min (prod)"));
    }

    #[test]
    fn ingest_orphan_line_not_binding() {
        let (_d, svc) = service_with_tmp_db();
        // 無圖快取 → 全 orphan
        let mut m = metric("tel-4", 1250.0, 1024);
        m.file_path = "src/missing.rs".to_string();
        let report = svc.telemetry_ingest(&payload("w-1", vec![m])).unwrap();
        assert_eq!(report.orphan, 1);
        assert_eq!(report.bound, 0);
        // orphan 仍寫入（canonical 空）
        let (_, rows) = svc.telemetry_get_context("w-1", "", true).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn sync_toon_none_does_not_panic() {
        let (_d, svc) = service_with_tmp_db();
        let mut svc = svc;
        svc.set_workspace_key("w-9".to_string());
        let out = svc.sync_toon(None);
        assert!(String::from_utf8_lossy(&out).contains("workspace_key"));
    }

    #[test]
    fn sync_toon_garbage_is_lenient_and_caches_empty_graph() {
        let (_d, svc) = service_with_tmp_db();
        let out = svc.sync_toon(Some(b"not-a-toon".to_vec()));
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("workspace_key"), "summary packet expected: {text}");
        let g = svc.graph().expect("garbage toon caches an empty graph");
        assert!(g.nodes.is_empty());
    }

    #[test]
    fn ingest_file_roundtrip() {
        let (_d, svc) = service_with_tmp_db();
        feed_graph(&svc);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.json");
        std::fs::write(
            &path,
            r#"{"version":"1.0","source":"draco-mcp","workspace_key":"w-1","metrics":[{"metric_id":"tel-f1","file_path":"src/db/query.rs","function_name":"query_users","line_number":88,"p50_ms":12.5,"p99_ms":1250.0,"alloc_bytes_per_req":10485760,"call_count_per_min":5000,"environment":"production","recorded_at":"2026-08-10T06:00:00Z"}]}"#,
        )
        .unwrap();
        let report = svc.telemetry_ingest_file(&path).unwrap();
        assert_eq!(report.bound, 1);
        assert_eq!(report.hotspots, 1);
    }

    #[test]
    fn ingest_binds_by_symbol_when_line_unknown() {
        // Draco 只給 symbol 不給行號：line_number = 0 → resolve_symbol
        // 兜底，file + function_name 相符即可綁定。
        let (_d, svc) = service_with_tmp_db();
        feed_graph(&svc);
        let mut m = metric("tel-s1", 1250.0, 1024);
        m.line_number = 0;
        let report = svc.telemetry_ingest(&payload("w-1", vec![m])).unwrap();
        assert_eq!(report.total, 1);
        assert_eq!(report.bound, 1);
        assert_eq!(report.orphan, 0);
        // 綁定至正確 canonical id
        let (_node, rows) = svc
            .telemetry_get_context("w-1", "src/db/query.rs:function:query_users", false)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "tel-s1");
    }

    #[test]
    fn draco_ingest_e2e_binds_and_is_stable() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hotspot_json = r#"{"window":{"start":"2026-08-10T00:00:00Z","end":"2026-08-10T01:00:00Z"},"count":1,"hotspots":[{"file_path":"src/db/query.rs","function_name":"query_users","p99_latency_ms":1250.0,"call_count":5000,"alloc_bytes":10485760}]}"#;
        // 每次 telemetry_ingest_draco 兩段 handshake（initialize + tools/call）；
        // 跑兩次 sync 驗證安定性。
        thread::spawn(move || {
            for i in 0..4 {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 4096];
                    let _ = stream.read(&mut buf).unwrap();
                    let body = if i % 2 == 0 {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake-draco","version":"0.0.0"}}}"#
                    } else {
                        &format!(
                            r#"{{"jsonrpc":"2.0","id":2,"result":{{"content":[{{"type":"text","text":{}}}]}}}}"#,
                            serde_json::to_string(hotspot_json).unwrap()
                        )
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: ses-e2e\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    stream.write_all(resp.as_bytes()).unwrap();
                }
            }
        });

        let (_d, mut svc) = service_with_tmp_db();
        svc = svc.with_draco_url(format!("http://{addr}/mcp"));
        svc.set_workspace_key("w-1".to_string());
        feed_graph(&svc);

        let report = svc.telemetry_ingest_draco(Some(10)).unwrap();
        assert_eq!(report.total, 1);
        assert_eq!(report.bound, 1, "symbol 升維成功");
        assert_eq!(report.orphan, 0);
        assert_eq!(report.hotspots, 1, "p99 1250ms > 500ms 門檻");

        // 安定 metric id：第二次同步 upsert 同一列，不產生重複 binding。
        let (_node, rows) = svc
            .telemetry_get_context("w-1", "src/db/query.rs:function:query_users", false)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].id.starts_with("tel-draco-"), "id: {}", rows[0].id);
        let id_before = rows[0].id.clone();
        let _ = svc.telemetry_ingest_draco(Some(10)).unwrap();
        let (_node, rows) = svc
            .telemetry_get_context("w-1", "src/db/query.rs:function:query_users", false)
            .unwrap();
        assert_eq!(rows.len(), 1, "重複 sync 不產生重複 binding");
        assert_eq!(rows[0].id, id_before);
    }

    #[test]
    fn draco_metric_id_is_deterministic() {
        let a = draco_metric_id("src/db/query.rs", "query_users");
        let b = draco_metric_id("src/db/query.rs", "query_users");
        let c = draco_metric_id("src/db/query.rs", "query_orders");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("tel-draco-"));
    }
}
