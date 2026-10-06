# Proposal — graphify-plugin-review（Symbol-Native Review Bridge）

## Executive Summary

`graphify-plugin-review` 是 Graphify 專用的 **Review 橋接器 (Symbol-Native
Review Bridge)**：以 `code-review-graph`（CRG）為單一 Review 資料源，將
CRG 產出的結構化 Review 點位（`file_path` + `line_number`）透過 Graphify
Core AST 圖譜 **0ms 升維對齊**至 canonical symbol（Symbol Path，如
`crate::auth::verify`），並託管於本地 `graphify.db`。當程式碼改動觸及
高風險審查點位時，透過 `graphify-mcp` 主動廣播警示與自動銷案，
形成完整 Review 防禦閉環。

本 plugin **不重造 Code Review 引擎** — 它是純 bridge。

## Problem Statement

- **行號脆弱性 (Line-based Fragility)**：傳統 Review 工具僅記錄
  `file_path + line_number`，重構或增刪行數後點位立即失效。
- **重構開銷**：在 plugin 內重新用 Rust 實作完整 Review 引擎開銷巨大；
  Agent 現場執行重型 Review 導致 context 污染與延遲暴增。
- **資訊孤島**：Agent 重構時缺乏對歷史 Review Warnings 的即時感知，
  容易重複犯下曾被標記的 Security / Correctness 錯誤。

## 方向變更記錄

- 先前雙軌方案（Track A native rewrite + Track B Python SDK）已廢止。
- `legacy/`（Python fork）、`sdk/`、`python/`、`docs/integration/` 已刪除。
- Python SDK 發展移至獨立 repo（graphify-sdk-python），不在本 repo 討論。

## Proposed Solution：B + A 混合模式（Slice 0 零阻礙發行）

### 選項 B — File-based Import（Slice 0 主路徑，100% 確定性）

- `review_ingest` 讀取標準 `IngestPayload` JSON 檔案（CRG 導出格式）。
- 與 opendoc Layer 1 同哲學：離線落盤、零外部依賴、零 CRG 介面風險。
- Slice 0 核心能力（line-to-symbol 升維、review_bindings 寫入、.toon
  拓撲合成、review_get_context 查詢）立即 100% 閉環。

### 選項 A — CRG MCP Analysis 接入（crg_client.rs 骨架）

- `crg_client.rs` 實作 MCP Handshake（Rust 說 MCP Protocol），作為即時
  分析工具的調用骨架（對接 CRG 現有 4 tools）。
- Slice 0 預留介面，不阻擋主路徑。

### 選項 C — CRG Bridge 對接契約（定案，2026-08-10 實測後修正）

- ~~`search_reviews` / `resolve_review` 整理為 CRG RFC / Feature Request~~ —
  **已廢除**：probe 實測 CRG 無 review 狀態 store，不存在可新增的
  銷案 / 搜尋語意。
- 改為純 bridge：plugin 以 MCP-over-HTTP 呼叫 CRG 現役 4 tools
  （`get_review_context_tool` / `detect_changes_tool` 為 review 點位主來源），
  對接契約見 `crg-requirements.md` — 不阻擋當前進度。

## Key Decisions（裁決紀錄）

| # | 裁決 | 內容 |
|---|------|------|
| R1 | canonical_node_id | = Graphify extract 實際產生之 `Node.id` 原樣（`{file_path}:{kind}:{name}`，相對路徑執行時含 `./` 前綴），與 `GraphUpdateEvent.modified_nodes` 同 namespace；內部 hash NodeId 不另存不存在 |
| R2 | MCP 歸屬 | plugin 不寫 MCP Protocol Server；review\* 工具由 graphify-mcp 自動註冊；ImpactAlert 為 domain event 由 graphify-mcp 轉發 |
| R3 | Sampling | 砍除（反向 LLM 採樣過度複雜，回歸確定性映射 + drift resolution） |
| R4 | SQLite | 併入專案共用 graphify.db（review_bindings 表，`PRIMARY KEY (workspace_key, id)`），不單獨開檔 |
| R5 | Bridge 優先 | 純 bridge 不重造引擎；legacy Python fork 已刪除 |
| R6 | workspace_key 範圍 | bindings 一律以 plugin 當前 bound 的 `get_workspace_key()` 為範圍（與 relay/opendoc 一致），`IngestPayload.workspace_key` 僅作 CRG provenance（commit `69fa8bb`） |

## Success Criteria

### Slice 0（shipped —工事 `c344dc4` / `69fa8bb` / GraphifyRust `424cd72`）

- [x] `review_ingest`（file-based）→ resolver 升維 → `review_bindings`
      寫入 → `review_get_context` 查詢，全鏈路 100% 本地閉環、33/33 單元測試
- [x] graphify-cli `review` 子指令 + graphify-mcp 啟動時自動註冊 3 個 review\*
      工具（reviewIngest / reviewGetContext / reviewResolve），21/21 mcp
      測試
- [x] 零網絡依賴、零 mock、確定性輸出（e2e fixture 全鏈路通過）
- [x] 開源安全：版本控制無私有主機名、本地 IP、本機路徑（sensitive scan
      clean）

### Slice 1 — Drift Guard & Auto-Resolution

- [ ] `on_graph_updated` 偵測到綁定 Node.id 不再存在於新 GraphOutput → 該
      binding 自動標 `resolved` 並填 `resolution_reason='auto: node gone'`
      與 `resolved_at = now()`
- [ ] 不再依賴 `signature_hash` 的「結構比對」邏輯（§7.2 YAGNI 裁決
      後，欄位保留但無實作路徑）
- [ ] schema migration 對已 shipped bindings 無破壞性（`ALTER TABLE ADD
      COLUMN resolution_reason / resolved_at / resolved_by`）
- [ ] plugin 不引入新第三方依賴（YAGNI）；ureq 仍僅在 Slice 2
      `crg_client` 真呼叫時啟用（CRG API 到位後）
- [ ] graphify-mcp 3 tool 行為與 Slice 0 binary 相容；回應中 `resolved_by`
      為 optional 欄，舊 client 透明

### Slice 2 — Real-time Impact Guard

- [ ] `on_graph_updated` 沿 `event.modified_nodes` 走逆向 depth=2 BFS，
      命中含 unresolved high/critical binding 之衝擊節點 → 產 `ImpactAlert`
      domain event（結構見 design §8.2）
- [ ] graphify-mcp 把 `ImpactAlert` 轉發為 MCP
      `notifications/review/impact_alert`（trait v1.1 協商完成後）
- [ ] 效能：50 節點 + 5 reviews fixture 全鏈路（sync_toon →
      on_graph_updated → BFS → alerts）< 50ms
- [ ] 永不 panic：BFS 失敗（如 GraphOutput 空白）只 log + Err，
      與 Slice 0 「plugin 永不 panic」契約一致

### CRG Bridge 契約（T1.4，已定案）

- [x] `crg-requirements.md` 定案為純 bridge 對接契約 — probe 實測 CRG
      現役 4 tools（MCP-over-HTTP），~~`search_reviews` / `resolve_review`~~
      廢除（CRG 無 review 狀態 store）；review 狀態以 graphify.db
      `review_bindings` 為 source of truth
