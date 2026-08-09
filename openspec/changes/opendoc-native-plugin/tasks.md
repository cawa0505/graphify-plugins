# Tasks — opendoc-native-plugin

> 文件先行：實作前需先完成對應的 openspec 文件並與 GraphifyRust 對齊契約。

## Task 1: 文件同步與契約對齊
- [x] `SPEC.md`（原始規格草案）逐字保存，標註 superseded
- [x] `proposal.md` / `design.md`：以 graphify-core 實際 v1 契約為基準，鎖定兩層架構
- [x] 雙語 README（`README.md` + `README.zh-TW.md`）
- [x] 契約對齊（已定案）：
  - 同步 trait（非 async_trait）
  - workspace mapping 手動設定（存 plugin SQLite）
  - 不直接依賴 `opendoc-storage`（libsqlite3-sys 衝突）
  - Layer 2 傳輸：MCP-to-MCP 轉發（graphify-mcp → opendoc-mcp）
  - Graph 邊界：plugin 自有 registry + query-time merge
  - `implements_spec` 為 query-time virtual edge（不持久化進 Core graph）
  - Markdown 解析用 pulldown-cmark
  - Qdrant 在 OD 端，不在 plugin 端

## Task 2: Crate 骨架（Slice A）
- [ ] 建立 `graphify-plugin-opendoc` crate（lib）
- [ ] `Cargo.toml`：依賴 graphify-core（path）、graphify-registry（path）、
      rusqlite（bundled）、serde / serde_json、sha1、pulldown-cmark、thiserror；
      dev: tempfile
- [ ] 實作 `GraphifyPlugin` trait（`get_id` / `bind` / `get_workspace_key` /
      `sync_toon` / `on_graph_updated`）
- [ ] `OpendocPlugin` struct + `with_registry_path` builder + `bind_for_cli`
      + `with_backend` builder（注入 `Box<dyn SpecSearchBackend>`）
- [ ] `SpecSearchBackend` trait + `NoOpBackend` + `SearchHit`（Layer 2 介面）
- [ ] 驗證：`cargo check` / `cargo clippy` 通過

## Task 3: Markdown spec 解析 + 硬鏈結抽取（Slice B）
- [ ] `spec.rs`：pulldown-cmark 解析 → `extract_blocks`（ATX heading 切塊）、
      `slug`、`block_signature`
- [ ] `links.rs`：`index_docs`（root + doc_paths → `Vec<LinkRow>`）
- [ ] 測試：heading 切塊、symbol 抽取、slug 正規化、signature 穩定性、
      code fence 內 `Symbol:` 捕獲（ponytail: 可接受極限）
- [ ] 驗證：`cargo test` 通過

## Task 4: SQLite link registry（Slice B）
- [ ] `registry.rs`：`LinkRegistry::open` / `replace_links` /
      `query_by_doc` / `query_by_symbol` / `all_links` /
      `set_workspace_mapping` / `get_workspace_mapping`
- [ ] workspace 隔離（所有查詢以 `workspace_key` 為前置條件）
- [ ] `opendoc_workspace_mapping` 表（schema 先定義，Layer 1 不寫入）
- [ ] 測試：workspace 隔離、idempotent replace、roundtrip 查詢、
      mapping 設定/讀取
- [ ] 驗證：`cargo test` 通過

## Task 5: sync_toon 封包（Slice B）
- [ ] `sync.rs`：`emit_packet` / `parse_meta` / `major_mismatch` /
      `emit_error_packet`
- [ ] metadata MUST：`format_version: "1.0.0"` + `workspace_key`
- [ ] plugin 狀態放 `metadata.plugin_data["opendoc"].links`
- [ ] 測試：封包 roundtrip、error 封包、major mismatch 拒絕、
      foreign workspace_key 拒絕、escape roundtrip
- [ ] 驗證：`cargo test` 通過

## Task 6: 業務 API + 雙向 drift audit（Slice C）
- [ ] `index_docs`：解析 + 抽取 + 寫入 registry（reindex 語意）
- [ ] `trace_doc_to_code`：doc → code symbols（硬鏈結查詢）
- [ ] `fetch_code_to_doc_context`：symbol → spec 區塊（硬鏈結查詢）
- [ ] `audit_drift` doc-side：重讀文件、比對 signature、回報
      UpToDate/DocChanged/DocMissing
- [ ] `audit_drift` code-side：透過 sync_toon + query_bfs 查詢 symbol
      是否存在於 AST graph，不存在 → CodeMissing
- [ ] `set_workspace_mapping`：存 `opendoc_workspace_mapping` 表
- [ ] 測試：index/trace/fetch roundtrip、雙向 drift 偵測
      （UpToDate/DocChanged/DocMissing/CodeMissing）、sync_toon 跨 session
      還原、unbound 行為、NoOpBackend 回空
- [ ] 驗證：`cargo test` + `cargo clippy` 通過

## Task 7: 整合驗證
- [ ] GraphifyPlugins workspace 加入此 crate 為 member
- [ ] `cargo check -p graphify-plugin-opendoc` 通過
- [ ] `cargo test -p graphify-plugin-opendoc` 全綠
- [ ] `cargo clippy -p graphify-plugin-opendoc --all-targets` 零警告
- [ ] 零 mock 檢查：無 stub 向量資料、無偽造查詢結果
- [ ] 開源去識別檢查：無本地主機名、IP、絕對路徑

## Task 8: MCP 效率層（graphify-mcp 側，後續）
- [ ] graphify-mcp 註冊 MCP tools：`opendoc_get_context` /
      `opendoc_audit_drift` / `opendoc_index`
- [ ] `McpBackend` 實作（graphify-mcp 側）：MCP-to-MCP 轉發打 opendoc-mcp
- [ ] workspace mapping 注入：graphify-mcp 啟動時帶 `X-Workspace` header
- [ ] 整合測試：graphify-mcp 啟動時自動註冊 opendoc tools
- [ ] 待 OpenDocuments 搜尋管線完成後驗證真 backend