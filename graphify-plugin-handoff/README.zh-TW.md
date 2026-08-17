# graphify-plugin-handoff

[English](README.md)

Graphify **內嵌型 plugin**：Code Relay 子系統的跨 Session、跨儲存庫（Repo）AI agent 狀態交接。以原生 Rust crate 實作 `GraphifyPlugin` trait，與 Graphify Core 直接整合。

本專案之設計概念啟發自原創 [code-relay](https://github.com/yan5xu/code-relay) 專案。我們高度致敬並尊重原作者的絕佳創意，並將此概念升級至常駐型的極速 Rust 內嵌 plugin 架構，融入 Graphify 生態系。

## 💡 核心特色

- **內嵌而非獨立伺服器**：以單一 Rust crate（`lib.rs`）提供，由 Graphify Core 在啟動時載入。無獨立 stdio JSON-RPC 進程、無需額外部署二進位。`relay*` 工具由 GraphifyMCP 在 Graphify 啟動時自動註冊。
- **雙軌客戶端介面**：同一套 7 個 relay 操作同時支援 MCP 工具（`graphify_relay*`，效率層）與 `graphify handoff ...` CLI（韌性層）— 即使 MCP 不可用，CLI 與直接讀取狀態檔仍可完整接力。crate 內附可自助安裝的 agent skill（`SKILL.md`）：`graphify handoff skill install` 可註冊至 opencode / Claude / Cursor / Cline。
- **記憶體常駐 Workspace 快取**：徹底解決舊版 Node.js 每次都要往上層目錄搜尋（walk-up disk search）導致的磁碟 I/O 延遲。工作區根目錄僅在 `GraphifyPlugin::bind`（透過注入的 `WorkspaceContext`）時定位一次並常駐於記憶體中，後續操作延遲低於 1ms。
- **雙軌混合記憶機制**：
  - **短期記憶 (Short-Term)**：採用對 Token 極度友善的 TOON (Token-Oriented Object Notation) 格式，儲存於 `.relay/relay.toon`，提供高速且精確的狀態交接。
  - **長期記憶 (Long-Term)**：整合 Qdrant 向量資料庫進行語意 RAG 檢索，可跨 Session 模糊搜尋過往的重大決策與技術脈絡。
- **與 plugin 生態對齊**：各 plugin（handoff, review, opendoc…）以 Graphify 注入的 `workspace_key` 對齊（graphify-core v1 契約）— 各自不 walk-up、不分歧 root 定位。
- **安全原子寫入**：寫入狀態時，採用「寫入臨時檔 + OS rename」的原子寫入機制，並結合 `fs2` 進行跨進程排他鎖定，防止並行寫入造成檔案毀損。

## 🤝 與 OpencodeCodeRelayPlugin 的相容關係

本 crate 是舊版 Node.js plugin 的 Rust 原生演化。向後相容（`npx opencode-code-relay-plugin <command>` 流程）為 **[待討論]** — 見 `openspec/changes/rust-mcp-migration/tasks.md`。

## 🛠️ 開發與驗證命令

```bash
# 建置專案
cargo build

# 程式碼品質與靜態檢查
cargo check
cargo clippy

# 執行單元測試
cargo test
```

## ⚙️ 設置說明

無需獨立伺服器設定。Graphify Core 依賴此 crate 並在啟動時載入為 plugin，GraphifyMCP 自動註冊 `relay*` 工具。設定採用完全動態與相對路徑設計 — 無環境層級機密、無硬編碼路徑。

## 📐 架構設計與細節

更詳細的需求書、系統設計以及 OpenSpec 規範文件，請參閱本專案的 `openspec/` 目錄。Graphify plugin 契約（`GraphifyPlugin` trait、`WorkspaceContext`）由 Graphify Core 定義，並與 GraphifyRust 專案協調。

## 📄 授權條款

MIT
