# Spec — telemetry-bridge

> 以下內容為 Architecture & Technical Specification v1.0.0 逐字轉錄（§3, §6）。

## 3. 數據架構與 Schema 定義 (Data Architecture)

### 3.1 telemetry_ingest 輸入協定 (TelemetryIngestPayload)

JSON
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

### 3.2 本地 SQLite 儲存庫 Schema (graphify.db -> telemetry_bindings)

SQL
CREATE TABLE IF NOT EXISTS telemetry_bindings (
    id TEXT PRIMARY KEY,                  -- Metric ID (例如 tel-p99-001)
    workspace_key TEXT NOT NULL,          -- Workspace 隔離 Key
    canonical_node_id TEXT NOT NULL,      -- Graphify 實體 Node ID (src/db/query.rs:function:query_users)
    file_path TEXT NOT NULL,              -- 原始檔案路徑
    function_name TEXT NOT NULL,          -- 函數名稱
    p50_ms REAL NOT NULL,                 -- p50 延遲 (毫秒)
    p99_ms REAL NOT NULL,                 -- p99 延遲 (毫秒)
    alloc_bytes INTEGER DEFAULT 0,        -- 每次請求記憶體配置 (Bytes)
    call_count INTEGER DEFAULT 0,         -- 每分鐘調用次數
    is_hotspot BOOLEAN DEFAULT 0,         -- 是否超過 Critical 熱點門檻 (p99 > 1000ms 或 high alloc)
    environment TEXT NOT NULL,            -- production | staging | local
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- 索引優化：0ms 檢索
CREATE INDEX IF NOT EXISTS idx_telemetry_node ON telemetry_bindings(workspace_key, canonical_node_id);
CREATE INDEX IF NOT EXISTS idx_telemetry_hotspot ON telemetry_bindings(workspace_key, is_hotspot);

## 6. .toon 拓撲合成範例 (.toon Synthesis Output)
當 Coding Agent 在進行 Codebase 檢索或發起 handoff 時，從 .toon 看到的節點脈絡：

Plaintext
[src/payment/stripe.rs:function:process_payment]
 ├── ⚡ [Telemetry] p99: 2,450.0ms | Alloc: 15.2MB (PROD HOTSPOT)
 ├── ⚠️ [Review Bridge] PR #88 Warning: Unhandled RateLimit Error
 └── 🔗 [Impact Radius] 影響上游 4 個 Checkout API Endpoint

---

## ⚠️ Deviation & Status（非原規格內容）

- **§3.2 schema 實作差異**：實際 `telemetry_bindings` 的 PRIMARY KEY 為
  `(workspace_key, id)` composite（同 review plugin 慣例），`id` 不再單獨
  PK — 共用 graphify.db 時不同 workspace 的同名 metric 不互踩；另
  `function_name` 實作為可空（orphan metric 無函式名）、`p50_ms` /
  `p99_ms` 實作為 `DEFAULT 0`。其餘欄位/索引/門檻註解逐字落地。
- **門檻值矛盾已定案（Slice 1）**：schema 註解寫 `p99 > 1000ms`，Slice 1
  範例寫 `p99 > 500ms`。Slice 0 暫採 schema 註解值；Slice 1 定案為動態
  `ThresholdConfig` — 預設 `p99 > 500ms || alloc > 5MB`（roadmap 明示值），
  env `TELEMETRY_HOTSPOT_P99_MS` / `TELEMETRY_HOTSPOT_ALLOC_BYTES` 覆寫。
- **§6 合成格式為示意**（含 Review Bridge / Impact Radius 行）；Slice 0
  實作採 §5.1 內嵌範例格式（`⚡ [Draco Telemetry] p99 Latency: Xms
  (CRITICAL HOTSPOT)` + `📊 Alloc: Y | Calls: Z/min (env)`）。Impact
  Radius 行屬 Slice 2。

### Draco MCP 契約 v1（fetch_top_hotspots，Slice 1 實作）

2026-08 web research 確認 public 無 Draco observability server（僅
cawa0505 同名網頁 scraper）— 契約由本 plugin 定義（greenfield）。
`DRACO_BASE_URL`（預設 `http://127.0.0.1:9878`）指向實現本契約的 server；
client 自動 `initialize` handshake（記 `Mcp-Session-Id` header），
`tools/call` 呼叫 `fetch_top_hotspots`，server-side 聚合，text 回契約 JSON：

```json
{
  "window": { "start": "2026-08-10T00:00:00Z", "end": "2026-08-10T01:00:00Z" },
  "count": 2,
  "hotspots": [
    { "file_path": "src/db/query.rs", "function_name": "query_users",
      "p99_latency_ms": 1250.0, "call_count": 5000,
      "alloc_bytes": 10485760, "environment": "production" }
  ]
}
```

- `window` / `count` 選填；`hotspots` 缺省視為無熱點（非錯誤）。
- `environment` 缺省 `"production"`。
- 轉譯：熱點 → IngestPayload（source=`"draco-mcp"`）走同一 ingest 管線；
  Draco 無行號，以 `resolve_symbol(file_path, function_name)` 升維；
  metric_id 由 `tel-draco-{DefaultHasher(file:func):x}` 安定派生 —
  重複同步 upsert 同一列，不產生重複 binding。
- 同步端點：`telemetry_ingest(source="draco-mcp")` → Top 10。
- 測試：TcpListener mock server 實測 initialize + tools/call 兩段 handshake。
