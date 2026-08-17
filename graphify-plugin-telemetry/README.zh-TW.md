# graphify-plugin-telemetry

**Pure Bridge Telemetry**（觀測資料橋接器）— Graphify 生態中，以 Draco MCP
為 Telemetry 資料源（Prometheus / OTEL / Jaeger / pprof）的內嵌型 Rust plugin。

## 定位

本 plugin 不自建 OTLP/Prometheus 服務端、不執行主動 Profiling、不把
Telemetry 節點塞進 Core AST 圖譜。它把 Draco MCP 抓取的觀測數據
（`file_path` + `line_number` 點位）透過 Graphify Core 的 AST 圖譜
**升維對齊**至穩定的 canonical symbol（如 `src/db/query.rs:function:query_users`），
以 `telemetry_bindings` 表託管於共用 `graphify.db`，並以門檻判定
hotspot 後合成 .toon 可觀測性區塊供 Coding Agent 消費。

## 核心機制

- **Line-to-Symbol Resolver**：將脆弱的 `file_path + line_number` 綁定為
  穩定的 canonical symbol（AST 重建或行號位移仍保持綁定）。
- **telemetry_bindings 表**：併入專案共用的 `graphify.db`（`workspace_key`
  隔離），記錄 p99 / alloc / call rate 與 hotspot 旗標。
- **Hotspot Threshold Guard**：動態門檻（預設 `p99 > 500ms` 或
  `alloc > 5MB`）判定 `is_hotspot`；可經由環境變數
  `TELEMETRY_HOTSPOT_P99_MS` / `TELEMETRY_HOTSPOT_ALLOC_BYTES` 覆寫。
- **MCP 自動註冊**：`telemetry_ingest` / `telemetry_get_context` 由
  graphify-mcp 於啟動時自動註冊。
- **.toon 語意合成**：`sync_toon` 將 hotspots 合成可觀測性區塊
  （§6 格式），隨 packet `plugin_data.telemetry.toon_block` 輸出。

## 資料契約

`IngestPayload`（telemetry JSON 檔案）schema 1.0：

```json
{
  "version": "1.0",
  "source": "draco-mcp",
  "workspace_key": "my-app-v1",
  "metrics": [
    {
      "metric_id": "tel-p99-001",
      "file_path": "src/db/query.rs",
      "function_name": "query_users",
      "line_number": 88,
      "p50_ms": 12.5,
      "p99_ms": 1250.0,
      "alloc_bytes_per_req": 10485760,
      "call_count_per_min": 5000,
      "environment": "production",
      "recorded_at": "2026-08-10T06:00:00Z"
    }
  ]
}
```

## Repository layout

```
├── src/
│   ├── lib.rs        # GraphifyPlugin trait（get_id/bind/get_workspace_key/sync_toon/on_graph_updated）
│   ├── ingest.rs     # file-based Telemetry JSON import + 轉譯
│   ├── draco_client.rs # Draco MCP Client（MCP-over-HTTP，fetch_top_hotspots 契約 v1）
│   ├── resolver.rs   # Line-to-Symbol Resolver（對齊 GraphOutput）
│   ├── registry.rs   # telemetry_bindings 表與 DAO（併入 graphify.db）
│   ├── telemetry.rs  # telemetry_ingest / telemetry_get_context 業務 API + 門檻判定
│   └── sync.rs       # sync_toon 記憶體 GraphOutput 快取與 hotspot 區塊合成
└── README.md / README.zh-TW.md
```

## Ecosystem alignment

- **Part of Graphify Plugins**：sibling to `graphify-plugin-handoff` /
  `graphify-plugin-opendoc` / `graphify-plugin-review`；plugins 以
  `workspace_key`（graphify-core v1 契約）對齊 — 不各自 walk-up。
- **Contract**：實作 `GraphifyPlugin`（`get_id` / `bind` /
  `get_workspace_key` / `sync_toon` / `on_graph_updated`）；transport 與
  tool 註冊由 graphify-mcp 統一處理，plugin 不寫 MCP Protocol Server。
- **Open-source safe**：版本控制檔案不含私有主機名、本地 IP 或機器路徑。

## Development

```
cargo build / cargo check / cargo clippy / cargo test
```

## Reference

- 完整架構與技術規格：Architecture & Technical Specification v1.0.0
  （Status: Approved）— 由 graphify-mcp 驗證 `telemetry_ingest` /
  `telemetry_get_context` 自動註冊（Slice 0 完成）。
- Slice 1：Draco MCP 主動輪詢（`fetch_top_hotspots`）+ 動態門檻 ✅（契約 v1 由本 plugin 定義，見 openspec — public 無 Draco observability server；`DRACO_BASE_URL` 指向實現契約的 server）。
- Slice 2：`on_graph_updated` BFS 衝擊半徑 + `notifications/telemetry/hotspot_alert` 廣播。
