# Design — opendoc-native-plugin

## 1. 架構總覽

`graphify-plugin-opendoc` 為 Graphify 內嵌型 crate，直接編譯併入 Graphify 核心。不經過任何外掛進程、Stdio 或 JSON-RPC。

```
OpenDocuments Vector Client (Rust SDK)
        │  真實向量檢索（零 Mock，workspace UUID 硬性 Filter）
        ▼
Entity Resolver（symbol ↔ Graph Node 對齊）
        ▼
Graphify Core Memory Graph（Petgraph BFS Trace，受限於 workspace_key）
        ▼
.toon 序列化（HybridQueryResult）
```

## 2. 契約基準（以 graphify-core v1 為準）

插件實作 `graphify-core::plugin::GraphifyPlugin`：

```rust
// graphify-core/src/plugin.rs（實際契約）
pub struct WorkspaceContext {
    pub workspace_key: String,   // SipHash hex，非 UUID
    pub workspace_name: String,
    pub root_path: String,
    pub timestamp: i64,
}

pub trait GraphifyPlugin {
    fn get_id(&self) -> &str;
    fn bind(&mut self, ctx: WorkspaceContext);
    fn get_workspace_key(&self) -> &str;
    fn sync_toon(&mut self, opt_toon: Option<Vec<u8>>) -> Vec<u8>;
    fn on_graph_updated(&mut self, event: &GraphUpdateEvent) {}  // 預設 no-op
}
```

> **備註**：SPEC.md 草案中的 `WorkspaceKey` / `workspace_uuid` / `async_trait` / `GraphifyError` 為早期命名，已驗證與實際 core 契約不符，以本文件為準。`workspace_key` 即跨 plugin 硬對齊鍵。

## 3. 插件業務 API（公開函式，非 trait 方法）

插件除實作 `GraphifyPlugin` 外，另暴露兩個業務方法供 GraphifyMCP 註冊為工具（是否 async 待與 GraphifyRust 對齊）：

```rust
/// 業務意圖 → 程式碼：向量檢索 + BFS Trace 混合結果
async fn trace_doc_to_code(
    ctx: &WorkspaceContext,
    query: &str,
    graph: &GraphOutput,
) -> Result<HybridQueryResult, PluginError>;

/// Symbol → 非結構化文檔脈絡：反向檢索
async fn fetch_code_to_doc_context(
    ctx: &WorkspaceContext,
    symbol_name: &str,
) -> Result<Vec<VectorChunk>, PluginError>;
```

### 資料結構

```rust
pub struct VectorChunk {
    pub chunk_id: String,
    pub source_file: String,
    pub content: String,
    pub linked_symbols: Vec<String>,
}

pub struct HybridQueryResult {
    pub vector_evidence: Vec<VectorChunk>,
    pub toon_subgraph: ToonPayload,
}
```

## 4. 雙鍵隔離與記憶體對齊

- **`workspace_key`**：直接映射至 Graphify Core 的 Petgraph 記憶體實體。BFS Trace 只針對當前 workspace 的 AST 節點。
- **OpenDocuments workspace UUID**：呼叫 OpenDocuments Rust Client 時作為底層 Storage / Vector Engine Query 的硬性 Filter（`filter: doc_meta.workspace_uuid == ctx.uuid`）。
- 兩個鍵的對映規則由誰負責產生與持久化 → [待討論]（見 proposal §5 Open Questions）。

## 5. 效能預算（非硬性 SLA）

- 16ms BFS Trace 標記為效能預算目標，非硬性 SLA。需定義 benchmark 基準（樣本數、硬體、query 複雜度）後才能驗證。

## 6. 相依與部署

- 插件作為 `GraphifyPlugins/graphify-plugin-opendoc` 獨立 crate；GraphifyRust 以 path/git dependency 嵌入（與 handoff 相同模式）。
- 零 Mock：不允許 stub 向量資料；所有檢索必須執行真實向量查詢。

## 7. 安全與開源去識別

- 遵循 AGENTS.md：禁止私有主機名、本地 IP、本機絕對路徑進入版本控制。
- 不使用真實金鑰值；使用環境變數或本地 gitignored 檔案。
