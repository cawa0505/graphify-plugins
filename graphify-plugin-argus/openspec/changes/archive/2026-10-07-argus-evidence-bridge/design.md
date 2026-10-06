# Design — argus-evidence-bridge

## 1. 邊界與定位

Pure Symbol Bridge：plugin 只做兩件事，其餘借用既有機制。

1. **Ingest**：`argus graph` 的 Graphify v2 JSON → plugin 快取的 `GraphOutput`
   （供核心／其他查詢使用）。
2. **領域事件**：code 變動（`GraphUpdateEvent`）→ `RetestSignal` → v1.1
   `NotifyCallback`。

不重造 Argus 的 DAG 排程、不碰 SQLite、不啟動 worker、不自動 promote。
Argus 與 plugin 的唯一介面是 `argus graph` CLI 的 JSON 輸出（契約見
ArgusOrchestrator `docs/Phase07_M6_Openspec.md` §7）。

## 2. 資料流

```
graphify CLI/MCP
   └─ index/extract 完成 → broadcast(GraphUpdateEvent)
        └─ ArgusPlugin::on_graph_updated(event)
             ├─ 取快取 GraphOutput（若無，先跑 sync_toon(None)）
             ├─ seeds = event.modified_nodes（空時退回 prev/cur node-id diff）
             ├─ 命中 worker 節點（kind == "worker"）→ 由 source_file 取 phase
             └─ notify_cb(RetestSignal{...})
                  └─ host 轉發（本階段到此為止）

sync_toon(None)
   └─ 執行 `argus graph -o <tmpfile>`
        └─ 讀回 v2 JSON → GraphOutput → 快取 → 回 toon 摘要封包
```

## 3. Trait 對齊（graphify-core v1 + v1.1）

| trait 方法 | 本 plugin 實作 |
|---|---|
| `get_id` | `"graphify-plugin-argus"`（`PLUGIN_ID`） |
| `bind(ctx)` | 存 `workspace_key`、`root_path` |
| `get_workspace_key` | 回綁定值；未綁定回 `""` |
| `sync_toon(Some(toon))` | `from_toon` 解析進快取，回摘要封包 |
| `sync_toon(None)` | 跑 `argus graph` 取 JSON 進快取，回摘要封包 |
| `on_graph_updated(event)` | 產 `RetestSignal` 並經 callback 送出 |
| `set_notify_callback(cb)` | 存入 `notify_cb` |
| `on_health_check()` | 偵測 `argus` binary 可執行 |

契約遵循既有 plugin 慣例（`graphify-plugin-review`）：
- 封包版本 `format_version: 1.0.0` + `workspace_key`；MAJOR 不符以 error 封包拒絕。
- plugin 狀態放 `metadata.plugin_data["argus"]`；錯誤走 `metadata.error`。
- **永不 panic**：所有失敗路徑靜默跳過或回 error 封包。

## 4. RetestSignal 判定

第一階段（無 worker→檔案邊）判定來源只有一個：

- 快取 graph 中 `kind == "worker"` 的節點，其 `source_file` 指向
  `.argus/phases/<phase_id>.toml`（Argus Phase07 §7.5）。
- `modified_nodes`（或 diff 種子）中，若有 node id 等於該 worker 節點 id，
  或其 `source_file` 命中該 phase spec 路徑 → 該 phase 受影響。
- 每個受影響 phase 產一個 `RetestSignal`（同 phase 去重）。

種子來源比照 review plugin 的 `impact_seeds`：
`event.modified_nodes` 非空直接用；為空則以 prev/cur node-id 差集補位；首次
同步只建 baseline，不產噪音訊號。

## 5. RetestSignal schema

```json
{
  "kind": "RetestSignal",
  "workspace_key": "w-...",
  "phase": "probe",
  "reason": "phase spec changed: .argus/phases/probe.toml",
  "nodes": ["worker:probe/a", "phase:probe"],
  "event_id": "<uuid v4>",
  "generated_at": "<RFC3339 UTC>"
}
```

`kind` 讓 host 端能與其他 plugin 事件（如 review 的 `ImpactAlert`）區分。

## 6. 設定

| 設定 | 來源 | 預設 |
|---|---|---|
| `argus_bin` | plugin config / `ARGUS_BIN` 環境變數 | `argus`（PATH 查找） |
| `state_dir` | plugin config | `None`（沿用 Argus 的 `ARGUS_STATE_DIR` 或 XDG 預設） |
| `timeout` | 固定 | `10s`（`argus graph` 為純讀取，逾時即視為失敗） |

## 7. 已知限制

- `argus graph` 為同步 subprocess 呼叫；plugin 端不做 daemon／快取失效策略，
  每次 proactive sync 重跑一次（`argus graph` 為純 SQLite 讀取，成本低）。
- 無 worker→檔案邊，故原始碼檔變動無法對應到 worker（第二階段）。
- `retest` 判定只認 node id 與 phase spec 路徑，不做 AST 語意比對。
