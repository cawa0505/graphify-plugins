# Tasks — graphify-plugin-review（B+A 混合模式）

> 對齊 design.md / proposal.md（2026-08-10 方向變更後）。

## Slice 0 — Pure Bridge & Core Binding（基礎單向鏈路，零網絡）

- [x] **T0.0 Repo 清理**：砍 `legacy/`（Python fork）、`sdk/`、`python/`、
      `docs/integration/`；雙語 README 重寫為 bridge 定位
- [x] **T0.1 Docs 重寫**：design / proposal / tasks（本文件）
- [x] **T0.2 Crate Setup & Trait Stub**：Cargo.toml + `ReviewPlugin` struct
      實作 `GraphifyPlugin` trait（get_id / bind / get_workspace_key /
      sync_toon / on_graph_updated）— commit c344dc4
- [x] **T0.3 Database Migration**：`registry.rs` — review_bindings DDL 建表
      + CRUD DAO（併入 graphify.db，`workspace_key` scoped PK）
      — commit c344dc4
- [x] **T0.4 File-based Ingest**：`ingest.rs` — IngestPayload JSON 解析
      + 轉譯（file → 待綁定 review 列表）— commit c344dc4
- [x] **T0.5 Line-to-Symbol Resolver**：`resolver.rs` — innermost span 匹配，
      `file_path + line_number` → canonical_node_id（`{file_path}:{kind}:{name}`
      原樣保留，含 extract 的 `./` 前綴以對齊 `modified_nodes`）— commit c344dc4
- [x] **T0.6 Graph Cache**：`sync.rs` — sync_toon 收圖 → from_toon（全寬容）→
      記憶體 GraphOutput 快取 — commit c344dc4
- [x] **T0.7 Domain Logic**：lib.rs — review_ingest / review_ingest_file /
      review_get_context / review_resolve 業務 API。**workspace_key 範圍規則**：
      bindings 以 plugin 當前 bound 的 `workspace_key` 為主（與 relay/opendoc
      一致）；`IngestPayload.workspace_key` 僅作 CRG 端 provenance 標記，不參與
      綁定查詢範圍 — commit 69fa8bb
- [x] **T0.8 CRG MCP Client Skeleton**：`crg_client.rs` — MCP Handshake +
      tools/call 骨架（ureq，Box 化 ureq::Error 避免 large_enum_variant），
      Slice 0 僅 framing，真呼叫 Slice 1/2 接 — commit c344dc4
- [x] **T0.8b CRG Bridge 真呼叫**：`initialize()` 真呼叫（POST + 快取 session id）
      + `detect_changes()` 包裝 `detect_changes_tool`（`detail_level: "standard"`）
      + `CrgPriority` 解析；plugin `review_search_crg` 把 CRG 絕對路徑剝成 workspace
      相對路徑（對齊 graph 節點 `./src/...`）→ IngestPayload → ingest；CLI
      `graphify review search-crg` + MCP `reviewSearchCrg` tool 註冊 + dispatch +
      graph feed；e2e 實測（CRG build 153 nodes → search-crg **10 bound, 0 orphan**
      → get-context 命中 `initialize` medium risk 0.4）— commit daa0231 + 待 push
- [x] **T0.9 Tests + graphify-cli/mcp 註冊驗證**：plugin 33/33 單元測試全綠、
      clippy clean；graphify-cli `review` 子指令 + graphify-mcp 3 個 review*
      工具 auto-register（reviewIngest / reviewGetContext / reviewResolve），
      MCP 21/21 測試通過；CLI + MCP e2e（fixture: extract `.toon` → ingest
      binding 2 + orphan 1 → get-context 命中 → resolve 翻狀態 → 再查為空）
      — plugin c344dc4 + 69fa8bb；GraphifyRust 424cd72

## Slice 1 — Drift Guard & Auto-Resolution（雙向銷案與漂移防禦）— ✅ SHIPPED

> 細部 spec：design.md §7。CRG 端 RFC：`crg-requirements.md`。

- [x] **T1.1 Signature Hash 範圍裁決**：實作 YAGNI 砍法 — schema migration
      `ALTER TABLE review_bindings ADD COLUMN resolution_reason TEXT DEFAULT ''`
      `, ADD COLUMN resolved_at TEXT DEFAULT ''`
      `, ADD COLUMN resolved_by TEXT DEFAULT ''`，
      `signature_hash` 寫入固定預設值 `v1_default`（無比對路徑）。
- [x] **T1.2 on_graph_updated Auto-Resolver**：對 workspace 內所有
      `status='unresolved' AND canonical_node_id != ''` 的 binding，檢查
      canonical_node_id 是否還存在於當前快取 GraphOutput 的節點集 — 不存在
      → 自動標 `resolved` + `resolved_by='auto:node_gone'` +
      `resolved_at=now()` + `resolution_reason='canonical node no longer
      present in graph (renamed, moved, or removed)'`。graphify-mcp 在
      `graphify_notify_plugins` 與 `graph_reindex` 後觸發；CLI 在每次
      review 指令前 `feed_graph_and_drift` 觸發。
- [x] **T1.3 review_resolve 工具完整化**：`review_resolve` /
      `reviewResolve` 接受新 `resolved_by` 與 `resolution_reason` 參數
      （手動 path）；本地 graphify.db 更新 + 回應中含完整狀態。CRG 端
      反向銷案**已裁決廢除**（CRG 無 review 狀態 store，見
      `crg-requirements.md` §1/§6）— 不阻塞 T1.2，local 銷案可獨立 ship。
- [x] **T1.4 CRG Bridge 規格定案**：`crg-requirements.md` 改寫為純
      bridge 對接契約 — probe 實測 CRG 現役 4 tools（MCP-over-HTTP，
      endpoint 由 `CRG_BASE_URL` env 提供），R1 `search_reviews` / R2
      `resolve_review` 廢除；review 狀態以 graphify.db 為 source of truth。

## Slice 2 — Real-time Impact Guard（雙向主動衝擊防禦）

> 細部 spec：design.md §8。graphify-core v1.1 延伸需求見 §10。

- [x] **T2.1 Impact Radius Inspection Engine**：在 `on_graph_updated`
      中以 `event.modified_nodes` 為種子（MCP hook 目前不帶
      `modified_nodes` → plugin 端 prev/cur node-id diff 補位），用
      graphify-core `query_bfs` `max_depth=2` 走 BFS（雙向，含
      upstream callers + downstream callees）。**前置確認（已完成）**：
      `graphify_core::build_graph`（lib.rs:6，public）直接做
      `GraphOutput → DiGraph` + node_map 轉換 — **複用，不需自寫
      mapping 層**。首次 sync prev 空 → 空種子（只建 baseline，
      防誤報）。
- [x] **T2.2 ImpactAlert Domain Event 產出**：對 BFS 涵蓋集合內每個
      node，查 unresolved high/critical → 結構化 `ImpactAlert` 結構
      （design §8.2；impact.rs，uuid v4 event_id + RFC 3339
      generated_at）。
- [x] **T2.3 前置（trait v1.1）已 shipped**：graphify-core 加
      `NotifyCallback` + `set_notify_callback` default no-op；review
      plugin 覆寫儲存 + `emit_notify`；graphify-mcp `build_review_plugin()`
      注入 callback（v1.1 先 stderr log）。驗證三端全綠
      （core 10/10、plugin 38/38、mcp 21/21、clippy 0）。
- [x] **T2.3 graphify-mcp 轉發 ImpactAlert**：graphify-mcp 端把
      `emit_notify` 收到的 Value 包成 MCP `notifications/review/impact_alert`
      推送（notify buffer + response 後 drain；取代 v1.1 的 stderr log）。
      e2e 已驗證：v1 4 nodes baseline → 加 admin_login → reindex →
      diff 種子 → BFS 涵蓋 verify_token → critical r-201 alert 落地。
