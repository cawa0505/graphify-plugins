# Tasks — telemetry-bridge-v1

> 以下內容為 Architecture & Technical Specification v1.0.0 逐字轉錄（§7），
> 核取方塊為 Slice 0 執行後狀態（非原規格內容，原規格為全空 `[ ]`）。

## 7. 漸進式實作路線圖 (Implementation Roadmap)

🔹 Slice 0：基礎 Bridge 與確定性 Ingest (對齊 Slice 0 範式)
[x] Task 0.1: Crate Setup & TelemetryPlugin Stub。

[x] Task 0.2: 在 graphify.db 建立 telemetry_bindings 表與 DAO（workspace_key 隔離）。

[x] Task 0.3: 實作 resolver.rs，對齊實體 Node ID ({file_path}:{kind}:{name})。

[x] Task 0.4: 實作 ingest.rs（File-based JSON 載入）與 draco_client.rs（MCP-over-HTTP 骨架）。

[x] Task 0.5: 實作 sync.rs 將 Hotspot 資訊合成至 .toon 輸出。

[x] Task 0.6: 經由 graphify-mcp 驗證 telemetry_ingest 與 telemetry_get_context 自動註冊。

🔹 Slice 1：Draco MCP 主動輪詢與門檻過濾
[x] 實作動態門檻設定（如 p99 > 500ms 或 alloc > 5MB 自動設為 is_hotspot）。

[x] 透過 draco_client 實現一鍵同步當前 Cluster 的即時 Top 10 熱點。

🔹 Slice 2：雙向衝擊廣播 (Hotspot Alert Push)
[ ] 於 on_graph_updated 中整合 BFS 衝擊半徑計算。

[ ] 當 Agent 重構受影響的 Hotspot 上游時，經由 graphify-mcp 主動發送 notifications/telemetry/hotspot_alert。

---

## ⚠️ Deviation & Status（非原規格內容）

- Slice 0 六任務全部完成並驗證（41 tests green、clippy 0 warnings、
  graphify-mcp 端到端：tools/list 23 tools 含 telemetryIngest /
  telemetryGetContext，ingest 2 bound 1 hotspot）。
- Slice 1 動態門檻已完成：`ThresholdConfig`（預設 `p99 > 500ms ||
  alloc > 5MB`，env `TELEMETRY_HOTSPOT_P99_MS` / `TELEMETRY_HOTSPOT_ALLOC_BYTES`
  覆寫；42 tests green、clippy 0 warnings）。
- 注意：原規格 Slice 0 任務編號與先前重建版 tasks.md 不同（0.2=DAO /
  0.3=resolver / 0.4=ingest+draco / 0.5=sync / 0.6=mcp），本次已改為逐字
  原編號。
- Slice 1 Top 10 熱點同步受阻：workspace 內無 observability Draco（唯一
  Draco 為網頁 scraper）；`fetch_top_hotspots` 契約待 Draco 端確認後實作。
