# Change Proposal — opendoc-native-plugin

## 1. Problem

Graphify 目前的記憶體知識圖譜只涵蓋程式碼（AST 語意樹），非結構化文件（PDF / DOCX / XLSX / 網頁等）不在圖譜內。工程師在處理跨 code + document 的任務時，需要手動在 Graphify 與 OpenDocuments（RAG 平台）之間切換，兩邊的查詢結果無法互相關聯：

- 無法從「業務意圖查詢」直接取得對應的程式碼影響範圍（doc → code）。
- 無法從「已知 symbol」反向取得相關的非結構化文件脈絡（code → doc）。

## 2. Proposed Solution

在 Graphify 生態加入原生 Rust 內部插件 `graphify-plugin-opendoc`，直接編譯併入 Graphify 核心（無外掛進程、無 Stdio、無 JSON-RPC），作為程式碼圖譜與 OpenDocuments 向量庫之間的橋接層：

```
OpenDocuments Vector Client (Rust SDK)
        ↓ 真實向量檢索（零 Mock）
Entity Resolver（symbol ↔ Graph 節點對齊）
        ↓
Graphify Core Memory Graph（Petgraph BFS Trace）
        ↓
.toon 序列化（混合檢索最終產出）
```

### 雙鍵隔離（Dual-Key Alignment）

- **`workspace_key`**：Graphify 本地 AST 圖譜的硬性路由鍵（`graphify-core` v1 契約定義，`derive_workspace_key` 產生），確保 BFS Trace 只作用於當前專案的 AST 節點。
- **OpenDocuments workspace UUID**：呼叫 OpenDocuments Rust Client 時作為 Storage / Vector Engine Query 的硬性 Filter（`doc_meta.workspace_uuid == <uuid>`），確保向量檢索只回傳當前 workspace 的文件。

## 3. Key Decisions

| 項目 | 決策 | 依據 |
|------|------|------|
| 實作語言 | Rust（原生 crate） | Graphify core 為 Rust；零 Mock 直連 Rust SDK |
| 通訊方式 | 無 — 直接編譯併入 core（in-process） | 避免 JSON-RPC / Stdio / IPC 開銷 |
| Trait 契約 | 以 `graphify-core` v1 `GraphifyPlugin` 為準 | `workspace_key` 為跨 plugin 硬對齊鍵（SPEC.md 的 `WorkspaceKey`/`workspace_uuid` 命名為草案，以實際 core 契約為準） |
| 文件檢索 | 真實 OpenDocuments Rust SDK / Storage Layer | 零 Mock 原則：禁止模擬數據 |
| 效能預算 | 16ms BFS Trace 標記為效能預算，非硬性 SLA | 待 GraphifyRust 確認是否為硬性要求 |

## 4. Out of Scope（待討論）

- **Qdrant / 長期語意記憶**：與 handoff plugin 相同，RAG 供給邊界未定 — Graphify llm layer vs plugin sidecar。opendoc 插件不自行 bundle 向量資料庫，直連 OpenDocuments 現有 storage。
- **`async_trait` vs 同步 trait**：SPEC.md 草案使用 `async_trait`，但 graphify-core v1 `GraphifyPlugin` 為同步介面。待 GraphifyRust 對齊後決定插件業務 API（`trace_doc_to_code` / `fetch_code_to_doc_context`）的 async 與否。
- **16ms Trace 的實際量測方法**：需定義 benchmark 基準（樣本數、硬體、query 複雜度）後才能驗證。

## 5. Open Questions

- [ ] `workspace_uuid`（OpenDocuments 端）與 `workspace_key`（Graphify 端）的對映規則由誰負責產生與持久化？是否沿用 handoff plugin 的 workspace identity 機制？
- [ ] OpenDocuments Rust SDK 的 crate 對外暴露形式（published crate vs path/git dep）？
- [ ] 插件業務 API（trace_doc_to_code / fetch_code_to_doc_context）是給 GraphifyMCP 註冊成 MCP tools，還是只作為內部函式庫？
