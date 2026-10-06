# Tasks — graphify-plugin-test-coverage（Coverage Symbol Bridge）

> 對齊 design.md / proposal.md。

## Slice 0 — 基礎單向 Bridge（快照取代式覆蓋率綁定）

- [x] **T0.1 Crate Setup & Trait Stub**：Cargo.toml + `CoveragePlugin` struct 實作 `GraphifyPlugin` trait（get_id / bind / get_workspace_key / sync_toon / on_graph_updated）
- [x] **T0.2 Database Migration**：`registry.rs` — coverage_bindings DDL 建表 + CRUD DAO
- [x] **T0.3 LCOV Parser**：`ingest.rs` — LCOV 格式解析器（SF / DA / end_of_record）
- [x] **T0.4 JSON Parser**：`ingest.rs` — JSON 格式解析器，共用 `CoverageData` struct
- [x] **T0.5 Reverse Resolver**：`resolver.rs` — 反轉解析器（range coverage 統計、suffix path 比對）
- [x] **T0.6 Graph Cache**：`sync.rs` — sync_toon 收圖快取
- [x] **T0.7 Domain Logic**：`lib.rs` — `coverage_ingest` 業務 API
- [x] **T0.8 .toon 盲區合成**：`sync.rs` — sync_toon 盲區 metadata 摘要合成
- [x] **T0.9 Tests + clippy**：LCOV/JSON 解析、反轉解析、快照取代、.toon 盲區合成等 34 個測試全綠

## 非 Slice 0（未來增量）

- [x] **Slice 1**：可配置盲區閾值、branch coverage、覆蓋率歷史表（留待後續版本規劃）