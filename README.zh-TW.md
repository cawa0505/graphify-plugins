# graphify-plugin-opendoc

[English](README.md)

Graphify **內嵌型 plugin**：程式碼知識圖譜與非結構化文件之間的橋接層，提供
spec 區塊與程式碼 symbol 的雙向追蹤（doc → code / code → doc）及雙向 drift
稽核。以原生 Rust crate 實作 `GraphifyPlugin` trait，與 Graphify Core 直接整合。

## 為什麼是 Plugin（不是純 MCP）

純 MCP 只能讓 Agent 分別查詢文件和程式碼，兩者在 Agent 腦中是孤立資訊，需
消耗大量 Context Window 拼接。本 plugin 提供三個 MCP 做不到的跨維度能力：

1. **跨域圖譜綁定**：將 Markdown Header/Section 變成 AST 上的 Spec 節點，透過
   `implements_spec` 跨域邊連接原始碼圖譜。Agent 拿 .toon 拓撲時直接拿到
   「Spec 業務邏輯 + 程式碼依賴」的完整微型聯集圖譜。
2. **雙向 drift 偵測**：doc-side（檔案簽名 vs 索引簽名）+ code-side（symbol
   是否存在於 AST graph）。同時抓「程式碼改了但文檔過期」和「文檔新增了 spec
   但程式碼沒實作」。
3. **確定性 + 混合檢索**：硬鏈結（`# Symbol: <name>` / `@spec:<path>`）
   100% 確定性 match（0ms，不走向量）。軟性向量搜尋只在硬鏈結不存在時啟用。

## 架構（兩層）

**Layer 1 — plugin 自有領域（零外部依賴，現可實作）**

- pulldown-cmark 解析 Markdown AST → spec block。
- 硬鏈結（`# Symbol: <name>` / `@spec:<path>`）：100% 確定性文字 match —
  0ms、零誤判、不需要向量搜尋。
- SQLite link registry（doc ↔ code symbol + workspace mapping，透過
  `graphify-registry`）。
- 查詢 API：doc → code symbols、symbol → spec 區塊。
- 雙向 drift audit：doc-side（signature 比對）+ code-side（透過 `sync_toon`
  + `query_bfs` 查詢 symbol 是否存在於 AST graph）。
- `sync_toon`：跨 session 鏈結索引交換。

**Layer 2 — 向量軟搜尋（trait 介面，現行 NoOp fallback）**

- `SpecSearchBackend` trait（純 Rust 介面）。
- 現行實作：`NoOpBackend`（回空；硬鏈結優先）。
- `McpBackend`：graphify-mcp 啟動時注入，透過 MCP-to-MCP 轉發打 opendoc-mcp。
  不 path-dep `opendoc-storage`（因 `sqlx 0.7` 與 `rusqlite 0.32` 的
  `libsqlite3-sys` 版本衝突）。
- workspace mapping：手動設定，存 plugin SQLite。

## MCP 效率層

Plugin 核心引擎負責解析、硬鏈結計算、registry、drift audit。MCP 層只曝露
2~3 個極簡 API 讓 Agent 發起查詢或觸發 sync：

| MCP 工具 | 對應 plugin API | 說明 |
|----------|-----------------|------|
| `opendoc_get_context` | `fetch_code_to_doc_context` | 回傳該代碼節點最精準的 spec 區塊 |
| `opendoc_audit_drift` | `audit_drift` | 檢查全專案 Spec 與 Code 的失真度 |
| `opendoc_index` | `index_docs` | 索引文件（抽取硬鏈結寫入 registry） |

MCP 工具由 graphify-mcp 在啟動時自動註冊（與 handoff plugin 相同模式）。

## 內嵌而非獨立伺服器

以單一 Rust crate 提供，由 Graphify Core 在啟動時載入。無獨立 stdio
JSON-RPC 進程、無需額外部署二進位、無外部 IPC handlers。

## 開發與驗證命令

```bash
# 建置專案
cargo build

# 程式碼品質與靜態檢查
cargo check
cargo clippy

# 執行單元測試
cargo test
```

## 設置說明

無需獨立伺服器設定。Graphify Core 依賴此 crate 並在啟動時載入為 plugin。
設定採用完全動態與相對路徑設計 — 無環境層級機密、無硬編碼路徑。

## 架構設計與細節

實作規格見 `openspec/changes/opendoc-native-plugin/design.md`。`SPEC.md`
為歷史草案（superseded）。Graphify plugin 契約（`GraphifyPlugin` trait、
`WorkspaceContext`）由 Graphify Core 定義，並與 GraphifyRust 專案協調。

## 授權條款

MIT