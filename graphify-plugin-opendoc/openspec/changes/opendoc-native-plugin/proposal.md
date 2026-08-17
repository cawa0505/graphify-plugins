# Change Proposal — opendoc-native-plugin

## 1. Problem

Graphify 的記憶體知識圖譜涵蓋程式碼（AST 語意樹），但非結構化文件
（Markdown spec、PDF、DOCX 等）不在圖譜內。純 MCP 只能讓 Agent 分別查詢
文件和程式碼，兩者在 Agent 腦中是孤立資訊，需消耗大量 Context Window 拼接。

三個無法用純 MCP 解決的問題：

- **跨域圖譜綁定**：無法將文件 spec 區塊與程式碼 AST 節點連成聯集圖譜。
- **雙向 drift 偵測**：無法自動偵測「程式碼改了但文檔過期」和「文檔新增了
  spec 但程式碼沒實作」。
- **確定性檢索**：純語意搜尋容易因 Embedding 漂移回傳不相干 chunk；需要
  100% 確定性的硬鏈結匹配。

## 2. Proposed Solution

在 Graphify 生態加入原生 Rust 內部插件 `graphify-plugin-opendoc`，實作
`graphify-core::plugin::GraphifyPlugin` trait，直接編譯併入 Graphify 核心
（無外掛進程、無 Stdio、無 JSON-RPC）。

### 三個跨維度能力

1. **跨域圖譜綁定**：將 Markdown Header/Section 變成 AST 上的 Spec 節點，
   透過 `implements_spec` 跨域邊連接原始碼圖譜。Agent 拿 .toon 拓撲時直接
   拿到「Spec 業務邏輯 + 程式碼依賴」的完整微型聯集圖譜。
2. **雙向 drift 偵測**：doc-side（signature 比對）+ code-side（graph 查詢）。
3. **雙軌儲存**：硬鏈結（`# Symbol:` / `@spec:`）100% 確定性 match（0ms）；
   軟搜尋（向量 fallback）只在硬鏈結不存在時啟用。

### 兩層架構

**Layer 1（plugin 自有領域，現可實作）**：

- pulldown-cmark 解析 Markdown AST → spec block。
- 硬鏈結（`# Symbol: <name>` / `@spec:<path>`）：100% 確定性文字 match，
  0ms，零誤判。
- SQLite link registry（doc ↔ code symbol + workspace mapping）。
- 雙向查詢 API：doc → code、code → doc。
- 雙向 drift audit：doc-side（signature 比對）+ code-side（graph 查詢）。
- `sync_toon`：跨 session 鏈結索引交換。

**Layer 2（向量軟搜尋，trait 介面，現行 NoOp）**：

- `SpecSearchBackend` trait（純 Rust 介面）。
- 現行 `NoOpBackend`（回空，硬鏈結優先）。
- `RestBackend`：plugin 內建，以 ureq 直連 OD REST API（Layer 2 啟用）。
- workspace mapping：手動設定，存 plugin SQLite。

### 為什麼分兩層

Layer 1 的硬鏈結是核心價值：文件中明確標記的 symbol 對應是確定性 match，
不需要向量搜尋。這層現在就能完整實作與測試。

Layer 2 的向量軟搜尋是 fallback：當硬鏈結不存在時才需要。OpenDocuments
的 R1-R5（search endpoint / index path / workspace 隔離 / TEXT id）已驗證
通過，plugin 以 ureq 直連 OD REST API。Layer 2 先定義介面 + RestBackend
實作；未設定 OD base URL 時回退 NoOp。

## 3. Key Decisions

| 項目 | 決策 | 依據 |
|------|------|------|
| 實作語言 | Rust（原生 crate） | Graphify core 為 Rust |
| 通訊方式 | 無 — 直接編譯併入 core（in-process） | 避免 JSON-RPC / Stdio / IPC 開銷 |
| Trait 契約 | `graphify-core` v1 `GraphifyPlugin`（同步） | `workspace_key` 為跨 plugin 硬對齊鍵 |
| 業務 API | 同步公開函式（非 trait 方法） | 與 trait 一致；Layer 2 同步 ureq 呼叫 |
| Markdown 解析 | pulldown-cmark | 純 Rust、無系統依賴、AST 級解析 |
| 文件檢索（Layer 1） | 硬鏈結確定性 match + SQLite registry | 0ms、零誤判、零外部依賴 |
| 文件檢索（Layer 2） | `SpecSearchBackend` trait，RestBackend（ureq） | OD R1-R5 已驗證；未設定 URL 時 NoOp |
| Layer 2 傳輸 | plugin 內 ureq 直連 OD REST API | 避免 libsqlite3-sys 衝突；同步、無 tokio |
| Graph 邊界 | plugin 自有 registry + query-time merge | `GraphifyPlugin` v1 無直接 graph handle；不修改 Core |
| `implements_spec` 邊 | query-time virtual edge（不持久化進 Core graph） | Q1 決策 (a) |
| workspace mapping | 手動設定，存 plugin SQLite | 可控、不猜測、不自動建立 |
| `opendoc-storage` 依賴 | 不依賴 | libsqlite3-sys 版本衝突（sqlx 0.7 vs rusqlite 0.32） |
| Qdrant | 在 OD 端，不在 plugin 端 | Plugin 不 bundle 向量資料庫 |

## 4. Out of Scope

- **Layer 2 真 backend 實作**：RestBackend（ureq 直連 OD REST API）在 plugin
  內實作；未設定 OD base URL 時以 NoOp fallback。向量檢索品質（RRF 調參、
  threshold）為 OD 端職責。
- **Qdrant / 長期語意記憶**：與 handoff plugin 相同，RAG 供給邊界未定。
  opendoc 插件不自行 bundle 向量資料庫。
- **16ms BFS Trace 量測**：為 Graphify Core 的效能目標，非本 plugin 的 SLA。
  硬鏈結查詢為 0ms 確定性 match。

## 5. Resolved Questions

- [x] ~~`workspace_uuid` 與 `workspace_key` 的對映規則？~~ → 手動設定，
  存 plugin SQLite `opendoc_workspace_mapping` 表。Layer 1 不寫入（不跟 OD
  溝通），Layer 2 搜尋時查表取得 `od_workspace_id`。
- [x] ~~`async_trait` vs 同步 trait？~~ → 同步。業務 API 為同步公開函式，
  與 `GraphifyPlugin` trait 一致。Layer 2 的 async 在 McpBackend 側處理。
- [x] ~~OpenDocuments Rust SDK 的 crate 暴露形式？~~ → 不直接依賴。
  Layer 2 透過 MCP-to-MCP 轉發，不 path-dep `opendoc-storage`。
- [x] ~~業務 API 是 MCP tools 還是內部函式庫？~~ → 兩者皆是。為 plugin 公開
  函式；graphify-mcp 註冊為 MCP tools（`opendoc_get_context` /
  `opendoc_audit_drift` / `opendoc_index`）。
- [x] ~~Graph 邊界：spec node 放進 AST graph？~~ → 不放。Plugin 自有
  registry + query-time merge（Q1 決策 (a)）。`implements_spec` 為 query-time
  virtual edge。