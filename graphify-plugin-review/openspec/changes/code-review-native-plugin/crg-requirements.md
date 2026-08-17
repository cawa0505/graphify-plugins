# CRG Bridge 對接規格書 — graphify-plugin-review ↔ code-review-graph

> 對接方：code-review-graph (CRG) MCP Server（已安裝，MCP-over-HTTP）
>
> 需求方：graphify-plugin-review（Graphify 內嵌 Rust plugin）
>
> 狀態：定案（2026-08-10，probe 實測後修正）
>
> 架構：**純 MCP-to-MCP bridge** — plugin 以 MCP-over-HTTP 呼叫 CRG **現有**
> 4 個工具，**不要求 CRG 新增任何工具**。

## 1. 架構定位（重要修正）

早期草案（2026-08-10 上午）假設「CRG 是 review 狀態持有者，需新增
`search_reviews`（R1）/ `resolve_review`（R2）工具」。對實際安裝的 CRG
MCP server probe 後**修正**：

- CRG 是**即時 review context 產生器**：現役 4 個工具全部是查詢 / 分析類，
  沒有 review 持久化 store，也不存在「unresolved/resolved」狀態概念。
- **review 狀態的 source of truth 是 plugin 的 graphify.db
  `review_bindings` 表**，不是 CRG。
- bridge 的職責：plugin 向 CRG 取回 review 發現（風險、受影響節點、審查
  建議）→ 升維成 `canonical_node_id` → 綁定至 `review_bindings`。
- **R1 / R2 廢除**：不需要 CRG 新增工具，不需要反向銷案（CRG 不持有狀態）。

## 2. 已驗證事實（probe，2026-08-10）

- CRG MCP server 以 **streamable HTTP transport** 運行；endpoint 由
  `CRG_BASE_URL` 環境變數提供（預設 `http://127.0.0.1:8080/mcp`，
  對齊 OD_BASE_URL 慣例）。
- `initialize` handshake 回 `Mcp-Session-Id` header（後續請求必須帶）。
- `tools/list` 確認現役 **4 個工具**：
  1. `get_minimal_context_tool` — 極簡 context 入口（~100 tokens）
  2. `query_graph_tool` — 圖譜關係查詢（callers/callees/imports/tests…）
  3. `get_review_context_tool` — 結構化 review context（subgraph + 建議）
  4. `detect_changes_tool` — 風險評分變更審查（risk + 變更點 + test gaps）
- `tools/call` 需帶 `repo_root` 參數：無效路徑回 `isError:true`（"does not
  look like a project root"）；有效 repo 回 `structuredContent` + text
  content（實測 GraphifyRust 回 risk/changed_file_count/key_entities）。

## 3. Bridge 協定（crg_client.rs）

- framing 已 shipped（Slice 0，198 行）：`initialize_request` /
  `call_tool_request` / `extract_result_content`（剝 SSE 前綴）/
  `call_tool`（POST + Mcp-Session-Id + timeout 10s）。
- **已 shipped（CRG bridge 實測段）**：`initialize()` 真呼叫（POST +
  快取 session id）已實作並對實機 daemon 驗證；`detect_changes()`
  包裝 `detect_changes_tool`（`detail_level: "standard"` — `"minimal"` 只回
  name strings，無法產出 file:line 點位，實測確認後改用 standard 拿完整 node dict）。
- 錯誤處理：CRG 不可達 / 無效回應 → `CrgError`，plugin 端 **graceful 退避**
  （不阻塞 ingest 主路徑；對齊 OD RestBackend 的 NoOp 預設模式）。

## 4. 四個工具的對接契約

| CRG 工具 | plugin 用途 | 呼叫參數（arguments） | 回應 → IngestPayload 對映 |
|---|---|---|---|
| `get_minimal_context_tool` | 開場快速判斷（風險級別、下一步） | `{ "repo_root", "task" }` | 不直接產 review 點位；作 context 輔助 |
| `query_graph_tool` | BFS 衝擊的交叉驗證（可選） | `{ "repo_root", "pattern", "target" }` | 不直接產 review 點位；作驗證 |
| `get_review_context_tool` | **主來源**：受影響節點 + 審查建議 | `{ "repo_root", "changed_files", "detail_level" }` | `key_entities` / `summary` 內含的變更節點 → file:line |
| `detect_changes_tool` | **主來源（已實作）**：git diff 風險審查 | `{ "repo_root", "base", "include_source" }` | `review_priorities`（node dict + risk_score）→ file:line + severity |

- `repo_root` 由 plugin 的 `WorkspaceContext` 提供（插件綁定時已知）。
- 實測：`review_priorities` 內含 `file_path`（**絕對路徑**）、`line_start`、
  `risk_score`（0.0–1.0）、`name`、`kind`。plugin 在對映時剝掉 `{root_path}/`
  前綴轉成 workspace 相對路徑（與 graph 節點 `./src/...` 對齊），review_id
  以相對路徑組成（不含本機絕對路徑，符合開源去識別化）。

## 5. IngestPayload 對映規則

CRG 回應（text content 或 `structuredContent`）→ plugin ingest 輸入：

| CRG 欄位 | IngestPayload 欄位 | 備註 |
|---|---|---|
| file path（回應內含） | `file_path` | 相對 repo path |
| line number（回應內含） | `line_number` | 快照性質；升維由 resolver 做 |
| risk level | `severity` | 對映表見下 |
| review guidance / summary | `message` | 人可讀審查建議 |
| `workspace_key` | （plugin 綁定 key） | payload 的 workspace_key 僅 provenance，不參與 scoping |

severity 對映：

| CRG risk | review_bindings.severity |
|---|---|
| critical | critical |
| high | high |
| medium | medium |
| low | low |
| （無風險欄位） | info |

找不到對應節點 / 行號的點位 → 不綁定，記 log（不產生 orphan 噪音）。

## 6. 狀態與銷案（與舊草案的差異）

- **review 狀態以 graphify.db `review_bindings` 為準**（status /
  resolved_by / resolution_reason / resolved_at 全在 plugin 端）。
- **無 CRG 反向銷案**：CRG 不持有狀態，沒有 `resolve_review` 可呼叫。
- Slice 1 auto-resolver（`auto:node_gone`）與 Slice 2 ImpactAlert 都是
  plugin 內 graphify.db 的閉環 — 已 shipped，不受此修正影響。

## 7. 錯誤處理 / 降級

- CRG server 未啟動 → bridge 層回 `CrgError::Ureq`，plugin 視為
  「無外部 review 來源」，ingest 主路徑（file-based import）照常。
- 單一工具呼叫失敗 → 記 log 跳過，不中斷整個 ingest。
- 回應無 `structuredContent` 且 text 無法 parse → `CrgError::EmptyResult`，
  呼叫方 graceful 退避。

## 8. 待討論項目

1. **`repo_root` 的授權範圍**：plugin 以自身 workspace 為 repo_root 呼叫
   CRG — 需確認 CRG 對「非其常駐目錄」的 repo 是否可查（probe 顯示
   傳入任意有效 repo 路徑可行，但 CRG 的 graph store 是否已為該 repo
   indexing 需實測）。
2. **`query_graph_tool` 是否真的需要**：Slice 2 BFS 已用 graphify-core
   自己的 `query_bfs`，CRG 的 query_graph 僅作可選交叉驗證 — 傾向
   YAGNI，先不接。
