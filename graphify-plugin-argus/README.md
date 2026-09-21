# graphify-plugin-argus

ArgusOrchestrator 執行歷程 → Graphify 知識圖譜的橋樑 plugin。

遵循 **Pure Symbol Bridge**：不重造 Argus 的排程／執行引擎；只把
`argus graph` CLI 的 Graphify v2 JSON 匯入，並在 code 變動影響某個 phase 時
產出 `RetestSignal` 領域事件。

## 定位

| 項目 | 內容 |
|---|---|
| 外部資料源 | ArgusOrchestrator（`argus graph` CLI，Graphify v2 JSON） |
| 綁定語意 | phase / worker / evidence / decision 節點與其 relation 進入 Graphify 圖 |
| 事件方向 | Graphify `GraphUpdateEvent` → `RetestSignal`（經 v1.1 `NotifyCallback`） |

Argus 與本 plugin 的唯一介面是 `argus graph` 的 JSON 輸出（契約見
ArgusOrchestrator `docs/Phase07_M6_Openspec.md` §7）。

## 能力

### 1. Ingest（`sync_toon(None)`，proactive）

執行 `argus graph -o <tmpfile>`，讀回 JSON 轉成 `GraphOutput` 快取，回覆
toon 摘要封包（`metadata.plugin_data["argus"]`）。

### 2. 被動同步（`sync_toon(Some(toon))`）

解析外部 `.toon` 進快取。

### 3. RetestSignal（`on_graph_updated`）

收到 `GraphUpdateEvent` 後：

- 種子取 `event.modified_nodes`；為空時以 prev/cur node-id 差集補位
  （首次同步只建 baseline，不發噪音訊號）。
- 掃描 `kind == "worker"` 節點：節點 id 命中種子，或其 `source_file`
  （`.argus/phases/<id>.toml`）命中種子 → 該 phase 受影響。
- 同 phase 去重，每個受影響 phase 產一個 `RetestSignal`，經 host 注入的
  `NotifyCallback` 送出。

```json
{
  "kind": "RetestSignal",
  "workspace_key": "w-...",
  "phase": "probe",
  "reason": "change touches worker worker:probe/a (phase spec .argus/phases/probe.toml)",
  "nodes": ["worker:probe/a", "phase:probe"],
  "event_id": "<uuid v4>",
  "generated_at": "<RFC3339 UTC>"
}
```

### 4. 健康探測（`on_health_check`）

以 `argus --version` 是否成功判斷 binary 可用性。

## 設定

| 設定 | 來源 | 預設 |
|---|---|---|
| `argus_bin` | `ArgusPlugin::with_argus_bin` / `ARGUS_BIN` 環境變數 | `argus`（PATH） |
| `state_dir` | `ArgusPlugin::with_state_dir` | 沿用 Argus 的 `ARGUS_STATE_DIR` / XDG 預設 |

## 限制（第一階段）

- **僅通知，不自動重跑**：`RetestSignal` 推到 host notify callback 即止；
  不發明 consumer、不執行 `argus phase run`。
- **無 worker → 檔案邊**：Argus M6 匯出沒有「worker 讀取哪個原始碼檔」的邊，
  因此原始碼檔變動無法對應到 worker。目前只做 phase spec 檔變動的判定。
  待 Argus 擴充匯出後於第二階段接上。
- **A2A 自動觸發未實作**：`max_retries` / `trigger_condition` / `cooldown`
  等防護邊界（ArgusOrchestrator Phase07 §8.5）於 A2A 化後才啟用。
- 不做自動 promote：GREEN/RED 裁決永遠留給人。

## 驗證

```bash
# workspace 內
cargo test -p graphify-plugin-argus
cargo clippy -p graphify-plugin-argus --all-targets

# 真資料對接（需 argus binary 與已跑過 phase 的 state）
ARGUS_E2E_BIN=/path/to/argus ARGUS_E2E_STATE_DIR=/path/to/state \
  cargo test -p graphify-plugin-argus real_argus_e2e
```

未設定 `ARGUS_E2E_BIN` 時 e2e 測試自動 skip（不假造資料）。

## 授權

MIT
