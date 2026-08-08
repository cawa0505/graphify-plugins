# graphify-plugin-opendoc

[English](README.md)

Graphify **內嵌型 plugin**：程式碼知識圖譜與 OpenDocuments 向量庫之間的橋接層，提供程式碼與非結構化文件的雙向檢索（doc → code / code → doc）。以原生 Rust crate 實作 `GraphifyPlugin` trait，與 Graphify Core 直接整合。

## 💡 核心特色

- **內嵌而非獨立伺服器**：以單一 Rust crate 提供，由 Graphify Core 在啟動時載入。無獨立 stdio JSON-RPC 進程、無需額外部署二進位、無外部 IPC handlers。
- **零 Mock 原則**：所有向量檢索直接連通 OpenDocuments 真實的 Rust SDK / Storage Layer；所有 AST 端點直連 Graphify 記憶體內的真實語意樹（Petgraph）。不允許任何模擬或偽造數據。
- **雙鍵隔離 (Dual-Key Alignment)**：
  - `workspace_key`：Graphify 本地 AST 圖譜的路由鍵（依 `graphify-core` v1 `WorkspaceContext`），確保 BFS Trace 只作用於當前專案的 AST 節點。
  - OpenDocuments workspace UUID：每次向量 / Storage 查詢都作為硬性 Filter（`doc_meta.workspace_uuid == <uuid>`），確保檢索只回傳當前 workspace 的文件。
- **混合檢索 (Hybrid Retrieval)**：業務意圖查詢回傳向量證據 + 受影響程式碼的 `.toon` 壓縮子圖；symbol 查詢反向取得相關的非結構化文件脈絡。
- **與 plugin 生態對齊**：各 plugin（handoff, opendoc, review…）以 Graphify 注入的 `workspace_key` 對齊 — 各自不 walk-up、不分歧 root 定位。

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

無需獨立伺服器設定。Graphify Core 依賴此 crate 並在啟動時載入為 plugin，GraphifyMCP 自動註冊檢索工具。設定採用完全動態與相對路徑設計 — 無環境層級機密、無硬編碼路徑。

## 📐 架構設計與細節

原始規格草案見 `SPEC.md`；更詳細的需求書、系統設計以及 OpenSpec 規範文件，請參閱本專案的 `openspec/` 目錄。Graphify plugin 契約（`GraphifyPlugin` trait、`WorkspaceContext`）由 Graphify Core 定義，並與 GraphifyRust 專案協調。

## 📄 授權條款

MIT
