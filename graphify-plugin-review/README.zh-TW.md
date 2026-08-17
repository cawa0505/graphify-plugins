# graphify-plugin-review

**語意點位審查橋接器 (Symbol-Native Review Bridge)** — Graphify 生態中，
以 `code-review-graph`（CRG）為 Review 資料源的內嵌型 Rust plugin。

## 定位

本 plugin 不重造 Code Review 引擎。它把 CRG 產出的結構化 Review 數據
（`file_path` + `line_number` 點位）透過 Graphify Core 的 AST 圖譜
**升維對齊**至穩定的 canonical symbol（如 `crate::auth::verify`），
並託管於本地 `graphify.db`。當程式碼改動觸及高風險審查點位時，
透過 `graphify-mcp` 主動廣播警示並支援自動銷案，形成 Review 防禦閉環。

## 核心機制

- **Line-to-Symbol Resolver**：將脆弱的 `file_path + line_number` 綁定為
  穩定的 canonical symbol（AST 重建或行號位移仍保持綁定）。
- **review_bindings 表**：併入專案共用的 `graphify.db`，記錄評語狀態、
  Severity 與綁定時節點的結構 hash（drift guard 用，Slice 1 採 Node.id
  presence diff，signature_hash 寫入固定預設值 `v1_default` — YAGNI 裁決）。
- **MCP 自動註冊**：`review_ingest` / `review_get_context` /
  `review_resolve` / `review_search_crg` 由 graphify-mcp 於啟動時自動註冊。
- **Drift Guard & Auto-Resolution（Slice 1）**：`on_graph_updated` 偵測
  review 綁定的 canonical node 已不存在於最新 GraphOutput（rename / 移除 /
  檔案消失）→ 自動標 `resolved` + `resolved_by='auto:node_gone'`，不需 CRG
  端配合。graphify-mcp 在 `graphify_notify_plugins` 與 `graph_reindex`
  後觸發；CLI 在每次 review 指令前 `feed_graph_and_drift` 觸發。
- **review_resolve 完整化**：手動銷案接受 `resolved_by`（如 `manual`）與
  `resolution_reason` 參數,寫入 `review_bindings` 的 `resolved_by` /
  `resolution_reason` / `resolved_at` 欄位。
- **Impact Guard（Slice 2）**：`on_graph_updated` 偵測變動觸及
  high/critical 未解決點位時，產出 ImpactAlert domain event 經由
  trait v1.1 notify callback closure 交由 graphify-mcp 轉發。
  **已 shipped**：BFS 衝擊半徑（複用 `graphify_core::build_graph` +
  `query_bfs`，種子為 `modified_nodes` 或 prev/cur diff）＋
  graphify-mcp 以 `notifications/review/impact_alert` 推送
  （notify buffer + response 後 drain）。
- **CRG Bridge（MCP-to-MCP）**：`review_search_crg` 透過 `crg_client` 以
  MCP-over-HTTP 呼叫已安裝的 `code-review-graph`（CRG）MCP server 的
  `detect_changes_tool`，取回 `review_priorities`（node dict + risk_score），
  剝絕對路徑為 workspace 相對路徑後升維綁定至 `review_bindings`。CRG 不可達
  時 graceful 退避（0 bound, 0 orphan，不阻塞）。CLI：`graphify review
  search-crg`；MCP：`reviewSearchCrg`。環境變數 `CRG_BASE_URL` 指定 CRG endpoint
  （未設則 NoOp）。

## 資料契約

`IngestPayload`（review JSON 檔案）schema 1.0：

```json
{
  "version": "1.0",
  "source": "code-review-graph",
  "workspace_key": "my-app-v1",
  "reviews": [
    {
      "review_id": "crg-sec-001",
      "file_path": "src/auth.rs",
      "line_number": 42,
      "severity": "high",
      "category": "security",
      "comment": "Potential timing attack on HMAC token comparison.",
      "created_at": "2026-08-10T00:00:00Z"
    }
  ]
}
```

## 目錄結構

```
├── src/
│   ├── lib.rs        # GraphifyPlugin trait（get_id/bind/get_workspace_key/sync_toon/on_graph_updated）
│   ├── ingest.rs     # file-based Review JSON import + 轉譯
│   ├── crg_client.rs # CRG MCP Client 骨架（Rust 說 MCP Protocol，對接 CRG 4 tools）
│   ├── resolver.rs   # Line-to-Symbol Resolver（對齊 GraphOutput）
│   ├── registry.rs   # review_bindings 表與 DAO（併入 graphify.db）
│   ├── review.rs     # review_ingest / review_get_context / review_resolve 業務 API
│   └── sync.rs       # sync_toon 記憶體 GraphOutput 快取與 .toon 上下文合成
├── openspec/         # proposal / design / tasks（本變更）
├── docs/             # 設計概念文件
└── README.md / README.zh-TW.md
```

## 生態對齊

- **Graphify Plugins 一員**：與 `graphify-plugin-handoff`、
  `graphify-plugin-opendoc` 平行；plugin 之間以 `workspace_key`（graphify-core v1
  契約）對齊，不各自 walk-up。
- **契約**：實作 `GraphifyPlugin`（`get_id` / `bind` /
  `get_workspace_key` / `sync_toon` / `on_graph_updated`）；傳輸與工具註冊由
  graphify-mcp 統一處理，plugin 不寫 MCP Protocol Server。
- **開源安全**：版本控制檔案無私有主機名、本地 IP、或本機路徑。

## 開發

```
cargo build / cargo check / cargo clippy / cargo test
```

## 參考

- 上游 Review 工具：[code-review-graph](https://github.com/tirth8205/code-review-graph)
  （Python，MIT）— 本 plugin 對接其產出數據，不 fork、不內嵌。
- 完整架構與任務拆解：`openspec/changes/code-review-native-plugin/`。
