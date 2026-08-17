# Design — graphify-plugin-telemetry（telemetry-bridge-v1）

> 以下內容為 Architecture & Technical Specification v1.0.0 逐字轉錄（§4-5）。

## 4. 目錄結構與模組分工 (Directory Layout)
完全對齊 graphify-plugin-review 與 relay 的手寫內嵌 Pattern：

Plaintext
graphify-plugin-telemetry/
├── Cargo.toml
└── src/
    ├── lib.rs          # 實作 GraphifyPlugin Trait (lifecycle & API)
    ├── ingest.rs       # 檔案型 IngestPayload JSON 解析與 Draco 轉譯
    ├── draco_client.rs # MCP-over-HTTP Client 骨架 (呼叫 Draco MCP 4 工具)
    ├── resolver.rs     # Line/Stack-to-Symbol Resolver (對齊 GraphOutput NodeId)
    ├── registry.rs     # telemetry_bindings 表與 DAO (併入 graphify.db)
    ├── telemetry.rs    # telemetry_ingest / telemetry_get_context 業務邏輯
    └── sync.rs         # sync_toon 熱點上下文合成 (.toon packet v1.0.0)

## 5. Trait 實作與 MCP 工具註冊

### 5.1 GraphifyPlugin 介面宣告 (lib.rs)

Rust
pub struct TelemetryPlugin {
    db_conn: Arc<Mutex<Connection>>,
    draco_client: DracoMcpClient,
    graph_cache: Arc<RwLock<Option<GraphOutput>>>,
}

impl GraphifyPlugin for TelemetryPlugin {
    fn get_id(&self) -> &'static str { "telemetry" }

    fn sync_toon(&self, graph: &GraphOutput) -> Result<()> {
        // 1. 快取 GraphOutput
        // 2. 查詢 telemetry_bindings 中 is_hotspot = 1 的節點
        // 3. 合成 .toon 可觀測性區塊，格式如：
        //    [src/db/query.rs:function:query_users (AST Node)]
        //     ├── ⚡ [Draco Telemetry] p99 Latency: 1,250ms (CRITICAL HOTSPOT)
        //     └── 📊 Alloc: 10MB/req | Calls: 5,000/min (prod)
    }

    fn on_graph_updated(&self, delta: &GraphDelta) -> Result<()> {
        // 1. 檢查 delta.modified_nodes 是否包含已標記為 is_hotspot 的 NodeId
        // 2. 若有改動，發起 HotspotAlert Event 由 graphify-mcp 廣播給 Agent
    }
}

### 5.2 由 graphify-mcp 自動註冊之工具鏈

telemetry_ingest(source: String, path_or_draco_params: Option<String>)

可讀取本地 JSON 檔，或直接發起 draco_client.fetch_top_hotspots() ➔ 將結果經過 Resolver 升維寫入 graphify.db。

telemetry_get_context(canonical_node_id: String, include_impact_radius: Option<bool>)

查詢指定 Node ID 及其衝擊半徑（Upstream callers）的 p99 / Memory 效能數據。

---

## ⚠️ Deviation & Status（非原規格內容）

- **§5.1 trait 簽名為藍圖草案，非 graphify-core v1 實際契約**。實作採用
  review plugin 同款真契約：`sync_toon(&mut self, Option<Vec<u8>>) -> Vec<u8>`
  與 `on_graph_updated(&mut self, &GraphUpdateEvent)`；§5.1 的
  `sync_toon(&self, graph: &GraphOutput) -> Result<()>` 語意以「被動快取
  GraphOutput + 合成 hotspot 區塊」落地於 telemetry.rs / sync.rs。
- §4 目錄結構完全落地（七模組齊全），`TelemetryPlugin` 持有
  `service: TelemetryService { workspace_key, registry_path, graph_cache,
  draco_client }`，db 連線為惰性開啟（registry_path-based），非藍圖的
  `Arc<Mutex<Connection>>` 常駐欄位。
- MCP 工具名依 graphify-mcp 慣例註冊為 camelCase：`telemetryIngest` /
  `telemetryGetContext`。
