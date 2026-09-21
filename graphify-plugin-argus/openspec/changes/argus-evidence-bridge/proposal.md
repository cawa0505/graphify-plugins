# Change: Argus Evidence Bridge（graphify-plugin-argus）

## Why

ArgusOrchestrator（閉源商業層）已具備 Graphify v2 JSON 匯出能力（`argus graph`
CLI，見 ArgusOrchestrator `docs/Phase07_M6_Openspec.md` §7）。缺的是把它接進
Graphify 生態的橋樑：讓 Argus 的執行／決策歷程進入知識圖譜（可被既有 MCP
查詢工具追查），並在 code 變動影響某 phase 時，把「該 phase 應重測」的訊號
推到邊界。

本變更依 Pure Symbol Bridge：plugin 不重造 Argus 的排程／執行引擎，只負責
「Argus 匯出 JSON ↔ Graphify canonical graph」的 Ingest 與領域事件產生。

## What Changes

1. 新增 crate `graphify-plugin-argus`，實作 `GraphifyPlugin` v1/v1.1 trait：
   - `get_id` / `bind` / `get_workspace_key`：標準 workspace 綁定。
   - `sync_toon(None)`（proactive）：執行 `argus graph -o <file>`，讀回
     Graphify v2 JSON，快取為 `GraphOutput`，回覆 toon 摘要封包。
   - `sync_toon(Some(toon))`（passive）：解析外部 toon 進快取。
   - `on_graph_updated(&GraphUpdateEvent)`：收到 `Indexed` 事件後，以
     `modified_nodes` 比對 Argus graph 中 worker 節點的 `source_file`，找出
     受影響 phase，產 `RetestSignal` 並經 `set_notify_callback` 推送。
   - `on_health_check()`：偵測 `argus` binary 是否可執行。
2. **第一階段僅通知**：`RetestSignal` 推到 host notify callback 即止——不發明
   consumer、不自動執行 `argus phase run`（Argus 側 M4 哲學：執行層真相在
   rc，決策留給人）。
3. 更新 GraphifyPlugins workspace `Cargo.toml` members、根 `README.md`
   已實作表。

## Out of Scope（第二階段）

- **worker → 檔案邊**：目前 Argus M6 匯出沒有「worker 讀取哪個原始碼檔」的
  邊，因此無法做到「某原始碼檔變動 → 該 worker 重測」的精確判定。第一階段
  只做 phase spec 檔變動的判定（worker 節點 `source_file` 指向
  `.argus/phases/<id>.toml`）。
- **A2A 自動觸發**：`max_retries` / `trigger_condition` / `cooldown` 等防護
  邊界（見 ArgusOrchestrator Phase07 §8.5）。A2A 節點實作後才啟用。
- **自動 promote**：GREEN/RED 裁決永遠留給人。

## Impact

- Graphify 生態新增一個 plugin，其他 plugin 行為不變。
- 唯一對外依賴是 `argus` binary（PATH 或設定檔指定）與其 `graph` 子命令的
  JSON 格式——契約由 ArgusOrchestrator Phase07 §7 定義並已凍結。
