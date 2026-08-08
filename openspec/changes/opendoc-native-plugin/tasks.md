# Tasks — opendoc-native-plugin

> 文件先行：實作前需先完成對應的 openspec 文件並與 GraphifyRust 對齊契約。

## Task 1: 文件同步與契約對齊
- [x] `SPEC.md`（原始規格草案）逐字保存
- [x] `proposal.md` / `design.md`：以 graphify-core 實際 v1 契約為基準建立
- [ ] 與 GraphifyRust 對齊：
  - [ ] `async_trait` vs 同步 trait（插件業務 API）
  - [ ] `workspace_uuid`（OpenDocuments 端）與 `workspace_key`（Graphify 端）對映規則
  - [ ] OpenDocuments Rust SDK 的 crate 暴露形式（published vs path/git dep）
- [ ] 雙語 README（`README.md` + `README.zh-TW.md`）

## Task 2: Crate 骨架
- [ ] 建立 `graphify-plugin-opendoc` crate（lib 為主，[待討論] 是否需要 cli binary）
- [ ] 依賴：`graphify-core`（path/git）、`async-trait`（待確認）、OpenDocuments Rust SDK（待確認）
- [ ] 實作 `GraphifyPlugin` trait（`get_id` / `bind` / `get_workspace_key` / `sync_toon` / `on_graph_updated`）
- [ ] 驗證：`cargo check` / `cargo clippy`

## Task 3: Entity Resolver（symbol ↔ Graph Node 對齊）
- [ ] 從真實向量 chunk 抽取 `linked_symbols`
- [ ] 於 Petgraph 實例查詢對應 Node ID
- [ ] 執行 BFS Impact Trace（16ms 為效能預算）

## Task 4: 雙向檢索 API
- [ ] `trace_doc_to_code`：業務意圖 → 向量檢索 → BFS Trace → `.toon` 壓縮
- [ ] `fetch_code_to_doc_context`：symbol → 反向檢索非結構化文檔
- [ ] 零 Mock 驗證：所有檢索執行真實向量查詢（workspace UUID Filter）

## Task 5: 測試與整合驗證
- [ ] 單元測試：Entity Resolver 對齊、雙鍵 Filter
- [ ] 整合測試：GraphifyRust workspace 加入此 crate，驗證 GraphifyMCP 自動註冊工具
- [ ] 效能基準：定義並量測 BFS Trace 時間（驗證 16ms 預算）
