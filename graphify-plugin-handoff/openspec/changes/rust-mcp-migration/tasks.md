# Tasks: Embedded Plugin Architecture & Verification

## Overview

此計畫以 **文件先行** 原則執行 — 文件與規格永遠先於實作，每一階段皆須通過對應驗證門檻（validation gate）才進入下一階段。

## Roadmap

### Task 1: 同步 Graphify Plugin Trait 與 crate 結構
- [x] 等待 GraphifyRust 完成 `graphify-core/src/plugin.rs` 中的 `GraphifyPlugin` trait 定義 → **已完成**（v1 trait + reference 測試 + `PluginHost` 廣播機制已 shipped）
- [x] 更新 `openspec/design.md`：揭露 Plugin 介設計，強調「Embedded Crate」模式 → **已完成**（修正 bind by value、移除 repo_paths/perform_handoff、補 sync-toon 封包契約）
- [x] 更新 `openspec/proposal.md`：描述 Plugin SDK，與 GraphifyMCP 工具註冊機制 → **已完成**
- [ ] 於 `graphify-plugin-handoff` 建立 `src/lib.rs`，實作 `GraphifyPlugin` trait
- [ ] `Cargo.toml`：`[lib]` 為主，optional `[[bin]]` 僅供本地測試
- [ ] 依賴：`serde`、`serde_json`、`thiserror`、`fs2`、`sha1`、`chrono`、`graphify-core`（path dep `../../GraphifyRust/graphify-core`）

### Task 2: TOON 狀態模型與 Workspace Root 解析
- [ ] `src/state.rs`：`RelayState`（relay.json schema 1.0.0：project_context / active_baton / repos / state_snapshot / spec_sync / updated_at）+ `.relay/relay.toon` TOON 鏡像（短程記憶格式）
- [ ] `src/workspace.rs`：`bind` 時以 `ctx.root_path` 一次定位 relay root 並常駐快取；`relayInit` 為唯一寫入式 walk-up（向上找 `relay.json`）；具名 repo 走 `repos[name].path` registry 查表，零 walk-up

### Task 3: relay* 核心邏輯（供 graphify-mcp 呼叫）
- [ ] `src/relay.rs`：`save`, `resume`, `status`, `switch`, `add`, `close`, `init`（回傳語意依 PROTOCOL.md 凍結文字）
- [ ] `src/sync.rs`：`sync_toon` 封包 — metadata MUST `format_version: "1.0.0"` + `workspace_key`；relay 狀態放 `metadata.plugin_data.<plugin_id>`；錯誤以 `error` metadata 回傳、不得 panic
- [ ] `on_graph_updated`：將 `modified_nodes` 併入 active session 節點（依 §4.2 active nodes 追蹤）
- [ ] 使用 `fs2::FileExt::lock_exclusive` + temp-write + rename

### Task 4: 測試與驗證
- [ ] `cargo test`：狀態讀寫、workspace 解析、檔案鎖併發、sync_toon 封包合規（MUST metadata、error 路徑、MAJOR 不符拒絕）
- [ ] 整合測試（GraphifyRust 側）：在 GraphifyRust workspace 加入此 crate，驗證 `graphify-mcp` 能自動註冊 relay tools → 屬 GraphifyRust 整合範圍（#3124），此 repo 僅確保公開 API 穩定

### Task 5: 效能驗證 (Performance Benchmark)
- [ ] 使用 `criterion` 或 `std::time::Instant` 量測：
  - `relayResume` / `relaySave` 單次呼叫時間
  - workspace 常駐快取 vs 每次 walk-up 的延遲對比
- [ ] 效能目標：記憶體快取路徑 < 1ms

### Task 6: 整合與驗證 (INTEGRATION.md)
- [ ] 與 `opendoc-mcp`：確認 `workspace_key`（plugin 對齊鍵）與 OpenDocuments 端 `doc_meta.workspace_uuid` RAG 過濾欄位的對映規則
- [ ] 與 `graphify`：確認 `.toon` 在 graph 節點間的傳遞
- [ ] 整合測試（GraphifyRust 側）：加入此 crate，驗證 `graphify-mcp` 自動註冊 relay tools → 屬 GraphifyRust 整合範圍

### Task 7: 雙端 README 互聯與 Parity 驗證
- [ ] 更新 `README.md` 及 `README.zh-TW.md`，預告內嵌 plugin 定位
- [ ] 建立 `openspec/` 與實作的雙向連結（docs ↔ code）
- [ ] 執行 parity 驗證：doc 中描述的功能必須對應至實作

### Task 8: 編譯與發布 (可選)
- [ ] 編譯各平台二進位產物（僅供本地測試；正式版由 GraphifyRust 打包）
- [ ] 更新 `CHANGELOG.md`（若適用）

### Task 9: 工具整合研究 (INTEGRATION.md)
- [ ] 整合 use cases：code-relay 與 opendoc-mcp / graphify 的搭配場景
- [ ] 整合測試：輸出 `INTEGRATION.md`（若尚未存在）

## [待討論] 未決項目
- ~~Thin Node.js wrapper（`npx opencode-code-relay-plugin` 向後相容）是否保留~~ → **決議：不保留**。slim wrapper 的唯一用途是將獨立 MCP server 註冊進 opencode.json，該路徑在嵌入式架構下已不存在；`PLUGIN_SLIM.md` 已刪除。
- 是否保留原 plugin repo（OpencodeCodeRelayPlugin）作為向後相容橋接 → 視實際使用需求而定，但無 MCP 註冊需求即無橋接必要。
