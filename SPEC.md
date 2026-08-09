# graphify-plugin-opendoc 原生 Rust 內部插件規格文件

> **狀態：歷史草案（superseded）**
>
> 本文件為專案初期的規格草案，保留逐字不修改，作為設計演進的歷史記錄。
> 其中 `OpenDocNativePlugin` trait、`async_trait`、`WorkspaceContext.workspace_uuid`、
> `GraphOutput`、`GraphifyError`、`crates/graphify-plugin-opendoc` 路徑等命名
> 已驗證與 Graphify Core v1 實際契約不符。
>
> **實作依據請以 `openspec/changes/opendoc-native-plugin/design.md` 為準。**

## 1. 系統架構與定位 (Native Architecture)

本插件作為 Graphify Monorepo 中的 Native Rust Crate（crates/graphify-plugin-opendoc），直接編譯併入 Graphify 核心，不經過任何外掛進程或 Stdio 通訊。

數據流向：OpenDocuments Vector Client (Rust SDK) $\rightarrow$ Entity Resolver $\rightarrow$ Graphify Core Memory Graph (Petgraph) $\rightarrow$ .toon 序列化。

零 Mock 原則：不允許任何模擬或偽造數據。向量端點直連 OpenDocuments 真實的 Rust SDK / Storage Layer，AST 端點直連 Graphify 記憶體內的真實語意樹。

## 2. 原生 Trait 介面定義 (Native Trait Spec)

插件需實作 Graphify Core 的原生插件 Trait，於編譯期完成類型安全檢查：

```rust
// crates/graphify-plugin-opendoc/src/lib.rs

use async_trait::async_trait;
use graphify_core::{
    Graph::GraphOutput,
    Types::{WorkspaceKey, ToonPayload, GraphifyError}
};

/// 雙鍵識別結構體
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceContext {
    /// Graphify 本地 AST 靜態圖譜鍵值
    pub workspace_key: WorkspaceKey,
    /// OpenDocuments 向量資料庫強對應 UUID
    pub workspace_uuid: String,
}

/// 真實 Vector 檢索結果 (非 Mock)
#[derive(Debug, Clone)]
pub struct VectorChunk {
    pub chunk_id: String,
    pub source_file: String,
    pub content: String,
    pub linked_symbols: Vec<String>,
}

/// 混合檢索最終產出
#[derive(Debug)]
pub struct HybridQueryResult {
    pub vector_evidence: Vec<VectorChunk>,
    pub toon_subgraph: ToonPayload,
}

#[async_trait]
pub trait OpenDocNativePlugin: Send + Sync {
    /// 直連 OpenDocuments 真實向量庫，根據業務意圖檢索並發射 Graphify 16ms Trace
    async fn trace_doc_to_code(
        &self,
        ctx: &WorkspaceContext,
        query: &str,
        graph: &GraphOutput,
    ) -> Result<HybridQueryResult, GraphifyError>;

    /// 給定真實 Symbol 名稱，反向檢索 OpenDocuments 真實非結構化文檔 (xlsx/pdf/doc)
    async fn fetch_code_to_doc_context(
        &self,
        ctx: &WorkspaceContext,
        symbol_name: &str,
    ) -> Result<Vec<VectorChunk>, GraphifyError>;
}
```

## 3. 雙鍵隔離與記憶體對齊 (Dual-Key Alignment)

插件直接於 Rust 記憶體層級進行約束與校驗：

- `workspace_key`：直接映射至 Graphify Core 的 Petgraph 記憶體實體，確保發射 BFS Trace 時只針對當前專案的 AST 節點。
- `workspace_uuid`：於呼叫 OpenDocuments 的 Rust Client 時，作為底層 Storage / Vector Engine Query 的硬性 Filter 條件（例如 `filter: doc_meta.workspace_uuid == ctx.workspace_uuid`）。

## 4. 給 Code Agent 的開發指令 (Agent Directive)

可以將以下指示直接發送給 Code Agent 進行開發：

```
Task Directive for Code Agent:

Create Native Rust Crate:
- Create a new crate under crates/graphify-plugin-opendoc.
- Implement it as a pure Rust (.rs) internal plugin.
- Do NOT write any JSON-RPC, Stdio wrappers, or external IPC handlers.

NO MOCKING ALLOWED:
- Interact directly with the real OpenDocuments Rust SDK / vector store interface.
- All document chunk retrieval must execute real vector queries filtered by workspace_uuid.

Memory Graph Alignment:
- Accept &GraphOutput directly from graphify-core memory without serialization overhead.
- Extract linked_symbols from real vector chunks, look up matching Node IDs in the active
  Petgraph instance, and execute 16ms BFS Impact Trace.
- Compress the resulting subgraph using Graphify's .toon encoder before returning.

Export Rust API:
- Expose standard Rust async methods: trace_doc_to_code and fetch_code_to_doc_context.
```
