# Design — opendoc-native-plugin

> 本文件為實作依據。`SPEC.md` 為歷史草案，已標註 superseded。

## 1. 為什麼是 Plugin（不是純 MCP）

純 MCP 只能讓 Agent 分別呼叫 `mcp_search_doc("auth")` 和 `mcp_get_ast("auth.rs")`，
兩者在 Agent 腦中是孤立資訊，需消耗大量 Context Window 拼接。

Plugin 具備 MCP 做不到的三個跨維度能力：

### 1.1 跨域圖譜綁定（Cross-Domain Subgraph Binding）

Plugin 將 Markdown 的 Header / Section 變成 AST 上的「Spec 節點」，透過
`implements_spec` 跨域邊連接到原始碼圖譜：

```
[RFC-0004 Sec 3.2 (Spec Node)]
       │
       │  implements_spec  ← OpenDoc 建立的跨域邊（query-time virtual edge）
       ▼
[crate::auth::verify_token (AST Node)]
       │
       │  calls
       ▼
[crate::db::get_user (AST Node)]
```

Agent 拿 .toon 拓撲時，直接拿到「包含 Spec 業務邏輯 + 程式碼依賴」的
完整微型聯集圖譜。

**實作方式**：`implements_spec` 邊不持久化進 Graphify Core graph（Q1 決策 (a)）。
Plugin 自有 SQLite registry 儲存 spec↔symbol 鏈結；查詢時從 Core 取得
AST 子圖，在 query-time merge spec 節點與 `implements_spec` 邊，組成聯集
圖譜後以 .toon 回傳。

### 1.2 雙向 Drift 偵測（Bidirectional Drift Detection）

**程式碼改了，文檔過期了嗎？**

Core 偵測到 `verify_token` 的 AST signature 改變時，Plugin 比對 registry
中的 spec block signature，觸發警告：「verify_token 程式碼已修訂，但關聯的
RFC-0004 Sec 3.2 規格文件自 N 天前未更新」。

**文檔改了，程式碼沒實作？**

使用者在 docs/ 修改了 API 規範，Plugin 透過 symbol link 反查 Core graph，
發現沒有對應的 AST 節點，提示：「Spec 已新增 endpoint /v2/refresh，但代碼
拓撲中尚未存在對應 handler」。

### 1.3 雙軌儲存與離線確定性檢索（Deterministic & Hybrid Search）

**硬性鏈結（Hard Spec Link）**：代碼註解中的 `@spec docs/auth.md#sec-2` 或
Markdown 中的 `# Symbol: crate::auth::verify_token`，達成 100% 確定性匹配
（0ms，不走向量）。

**軟性檢索（Soft Hybrid Search）**：沒有硬鏈結時，才落回向量 + 全文混合搜尋
（Layer 2，透過 REST API 直連 OpenDocuments 伺服器）。

## 2. 架構總覽

`graphify-plugin-opendoc` 為 Graphify 內嵌型 crate，實作
`graphify-core::plugin::GraphifyPlugin` trait，直接編譯併入 Graphify 核心。
不經過外掛進程、Stdio 或 JSON-RPC。

兩層架構：

```
Layer 1（plugin 自有領域，零外部依賴，現可實作）
  pulldown-cmark 解析 Markdown AST → spec block
  硬鏈結（@spec / # Symbol:）→ 100% 確定性 match（0ms，無向量）
  SQLite link registry（doc ↔ code symbol + workspace mapping）
  雙向查詢 API：doc → code、code → doc（硬鏈結優先）
  雙向 drift audit：doc-side（signature 比對）+ code-side（graph 查詢）
  sync_toon：跨 session 交換鏈結索引

Layer 2（向量軟搜尋，抽象為 trait，現行 NoOp）
  SpecSearchBackend trait（純 Rust 介面）
  NoOpBackend（現行 fallback，回空）
  RestBackend：plugin 內建實作，以 ureq 直連 OD REST API（Layer 2 啟用時）
  workspace mapping：手動設定，存 plugin SQLite
```

### 為什麼分兩層

Layer 1 的硬鏈結是核心價值：文件中以 `# Symbol: <name>` 或 `@spec:<path>`
明確標記的區塊，與程式碼 symbol 的對應是 100% 確定性文字 match，不需要向量
搜尋，0ms，零誤判。這層現在就能完整實作與測試。

Layer 2 的向量軟搜尋是 fallback：當硬鏈結不存在時（文件未標記 symbol），
才需要向量近似搜尋。OpenDocuments 的 R1-R5（search endpoint / index path /
workspace 隔離 / TEXT id）已驗證通過，plugin 以 ureq 直連 OD REST API
（`POST /api/v1/search`）實作 `RestBackend`；未設定 OD base URL 時回退
`NoOpBackend`。

## 3. 契約基準（以 graphify-core v1 為準）

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
    fn bind(&mut self, ctx: WorkspaceContext);  // by value
    fn get_workspace_key(&self) -> &str;
    fn sync_toon(&mut self, opt_toon: Option<Vec<u8>>) -> Vec<u8>;
    fn on_graph_updated(&mut self, event: &GraphUpdateEvent) {}  // 預設 no-op
}
```

- `bind` 取 `WorkspaceContext` **by value**（非 by reference）。
- `WorkspaceContext` 只有 `workspace_key / workspace_name / root_path / timestamp`，
  無 `repo_paths`、無 `workspace_uuid`。
- relay root = `ctx.root_path`，bind 時定位一次，後續複用記憶體快取。
- trait 無 `perform_handoff` 方法；業務 API 為 plugin 的公開函式，非 trait 方法。
- `sync_toon` 的 .toon 封包錯誤以 metadata `error` 回報，不得 panic。

> **備註**：`SPEC.md` 草案的 `OpenDocNativePlugin` trait / `async_trait` /
> `WorkspaceKey` / `workspace_uuid` / `GraphifyError` / `GraphOutput` 均為早期
> 命名，已驗證與實際 core 契約不符。以本文件為準。

## 4. 插件業務 API（公開同步函式，非 trait 方法）

```rust
/// 索引文件：解析 spec block、抽取硬鏈結、寫入 registry。回傳鏈結數。
pub fn index_docs(&mut self, doc_paths: &[PathBuf]) -> Result<usize, Error>;

/// 文件 → code symbols（硬鏈結查詢，0ms 確定性）。
pub fn trace_doc_to_code(&self, doc_path: &str) -> Result<Vec<LinkRow>, Error>;

/// symbol → 對應 spec 區塊（硬鏈結查詢）。
/// MCP 工具名：opendoc_get_context(node_id)
pub fn fetch_code_to_doc_context(&self, symbol: &str) -> Result<Vec<LinkRow>, Error>;

/// 雙向 drift 稽核：doc-side（signature 比對）+ code-side（graph 查詢）。
/// MCP 工具名：opendoc_audit_drift()
pub fn audit_drift(&self) -> Result<Vec<DriftItem>, Error>;

/// 設定 workspace mapping（Layer 2 用，手動設定）。
pub fn set_workspace_mapping(&self, od_workspace_id: &str) -> Result<(), Error>;
```

業務 API 為**同步**（與 `GraphifyPlugin` trait 一致）。Layer 2 的 ureq
呼叫為同步阻塞；若 OD 端未來改 async API，在 RestBackend 內處理，不影響
plugin 本體的同步介面。

### 資料結構

```rust
/// 一條硬鏈結（registry 一列）。
pub struct LinkRow {
    pub workspace_key: String,
    pub doc_path: String,      // workspace root 相對路徑
    pub spec_id: String,       // "<doc_path>#<slug(title)>"
    pub symbol: String,        // 對應的 code symbol
    pub signature: String,     // spec block content 的 sha1 hex（drift 基準）
}

/// drift 稽核項目。
pub struct DriftItem {
    pub spec_id: String,
    pub doc_path: String,
    pub symbol: String,
    pub status: DriftStatus,   // UpToDate / DocChanged / DocMissing / CodeMissing
}

pub enum DriftStatus {
    UpToDate,    // 文件與索引一致，且 symbol 存在於 code graph
    DocChanged,  // 文件內容已變更（signature 不符）
    DocMissing,  // 文件已不存在
    CodeMissing, // spec 宣告的 symbol 在 code graph 中找不到（doc→code 缺實作）
}
```

## 5. Layer 1 詳細設計

### 5.1 Markdown spec block 解析

使用 `pulldown-cmark`（純 Rust Markdown parser，無系統依賴）解析 Markdown AST：

- ATX heading 切出 spec 區塊；標題以下的內容為區塊 content。
- 區塊內的 `# Symbol: <name>` 行宣告對應的 code symbol（硬鏈結目標）。
- `spec_id = "<doc_path>#<slug(title)>"`（可穩定再定位）。
- `block_signature` = 區塊 content 的 sha1 hex（drift audit 比對基準）。

### 5.2 硬鏈結 registry

SQLite（透過 `graphify-registry` 的 `RegistryDb`，bundled rusqlite）：

```sql
CREATE TABLE IF NOT EXISTS opendoc_links (
    workspace_key TEXT NOT NULL,
    doc_path      TEXT NOT NULL,
    spec_id       TEXT NOT NULL,
    symbol        TEXT NOT NULL,
    signature     TEXT NOT NULL,
    PRIMARY KEY (workspace_key, spec_id, symbol)
);

-- Layer 2 workspace mapping（手動設定，見 §6.3）
CREATE TABLE IF NOT EXISTS opendoc_workspace_mapping (
    workspace_key   TEXT PRIMARY KEY,  -- Graphify 端（SipHash hex）
    od_workspace_id TEXT NOT NULL       -- OpenDocuments 端（TEXT）
);
```

- `index_docs` 為 reindex 語意：先刪該 workspace 的舊鏈結、再寫入新抽取結果。
- 查詢 by `workspace_key + doc_path`（doc → code）或 `workspace_key + symbol`
  （code → doc）。
- `opendoc_workspace_mapping`：Layer 1 不寫入（不跟 OD 溝通），但 schema 先定義
  （forward-compatible、零成本）。Layer 2 搜尋時查這張表取得 `od_workspace_id`。

### 5.3 雙向 drift audit

**Doc-side drift**（Layer 1，不需 graph）：

對 registry 中每條鏈結：

1. 重讀 `root_path.join(doc_path)`。
2. 重新解析 spec block，找 `spec_id` 對應區塊。
3. 比對 `block_signature` 與 registry 的 `signature`。
4. 回報 `UpToDate` / `DocChanged` / `DocMissing`。

**Code-side drift**（需 graph 查詢，query-time merge）：

對 registry 中每條鏈結的 `symbol`：

1. 透過 `sync_toon` 取得 Graphify graph 資料。
2. 用 core `query_bfs` 查詢 symbol 是否存在於 AST graph。
3. 不存在 → 回報 `CodeMissing`（「Spec 宣告了 symbol，但代碼拓撲中找不到」）。

### 5.4 sync_toon 封包

- metadata MUST：`format_version: "1.0.0"` + `workspace_key`。
- plugin 狀態放 `metadata.plugin_data["opendoc"]`，承載 `links: Vec<LinkRow>`。
- 錯誤以 `metadata.error` 表達，不 panic。
- 版本政策：同 MAJOR 可互操作；MAJOR 不符以 error 封包拒絕。
- 轉義規則與 core `toon.rs` 一致。

## 6. Layer 2 詳細設計（向量軟搜尋）

### 6.1 介面

```rust
/// 向量軟搜尋介面（Layer 2）。現行 impl = NoOpBackend（回空）或 RestBackend（連 OD）。
pub trait SpecSearchBackend: Send + Sync {
    /// 依 query 搜尋文件 chunk，回傳最相關的 spec 區塊。
    fn search(&self, od_workspace_id: &str, query: &str) -> Vec<SearchHit>;
}

pub struct SearchHit {
    pub doc_path: String,
    pub spec_id: String,
    pub heading: Option<String>,  // OD R2：原始 heading 原文，plugin 算回內部 spec_id
    pub score: f64,
}

/// 現行 fallback：永遠回空（硬鏈結優先，無硬鏈結時無軟搜尋）。
pub struct NoOpBackend;

/// 真 backend：plugin 內建，以 ureq 直連 OD REST API（`POST /api/v1/search`）。
/// 不走 MCP 轉發，也不 path-dep `opendoc-storage`（libsqlite3-sys 衝突，見 §6.4）。
pub struct RestBackend {
    base_url: String,   // 例如 http://127.0.0.1:3006
}
```

Plugin 透過 `with_backend()` builder 注入 backend：

```rust
pub fn with_backend(mut self, backend: Box<dyn SpecSearchBackend>) -> Self {
    self.backend = backend;
    self
}
```

- 預設 `NoOpBackend`（Layer 1 獨立運作，不需 OD）。
- 設定 OD base URL（CLI `--od-url` / MCP 注入）時以 `RestBackend` 啟用 Layer 2。

### 6.2 傳輸方式：REST API 直連（ureq）

Layer 2 不直接 path-dep `opendoc-storage`（libsqlite3-sys 衝突，見 §6.4），
改以同步 HTTP client（ureq，無 tokio，無 async runtime）直連 OD 的
REST API：

```
Plugin (in-process)
  → SpecSearchBackend trait
  → RestBackend (plugin 內建, ureq)
  → OpenDocuments server REST API: POST /api/v1/search
```

- ureq 為同步、無 async runtime、無額外 native 依賴（除 rustls）。
- `search(od_workspace_id, query)`：
  `POST {base_url}/api/v1/search`，header `X-Workspace: <od_workspace_id>`，
  body `{"query": <query>, "top_k": 5}` → 解析 `{hits:[{doc_path, spec_id,
  heading, score, snippet}]}` → 轉 `SearchHit`。
- OD 端驗收依 `docs/OpenDocuments-Requirements.md` 8.1 測試程序（T1-T7 已過）。
- 網路錯誤 / 非 200 / 解析失敗 → 回空（Layer 1 不受影響，永不因 Layer 2 失敗
  而 panic）。
- Qdrant / 向量資料庫在 OD 端，不在 plugin 端。Plugin 不 bundle 向量資料庫。

### 6.3 Workspace Mapping（手動設定）

Graphify 的 `workspace_key`（SipHash hex）與 OpenDocuments 的 `workspace_id`
（TEXT）不同。對映由使用者手動設定：

```rust
/// 設定 workspace mapping（存 plugin SQLite）。
pub fn set_workspace_mapping(&self, od_workspace_id: &str) -> Result<(), Error>;
```

- 使用者透過 CLI 或 MCP tool 明確設定對映。
- 存 `opendoc_workspace_mapping` 表。
- Layer 2 搜尋時：查表取得 `od_workspace_id` → 傳給 RestBackend →
  RestBackend 帶 `X-Workspace` header 呼叫 OD `/api/v1/search`。
- 未設定 mapping 時，Layer 2 搜尋回空（不猜測、不自動建立）。

### 6.4 為什麼不能 path-dep `opendoc-storage`

`opendoc-storage` 依賴 `sqlx 0.7` → `sqlx-sqlite` → `libsqlite3-sys ^0.26`。
本 plugin 透過 `graphify-registry` 使用 `rusqlite 0.32` → `libsqlite3-sys 0.30`。
Cargo 規則：同一 dependency graph 只允許一個 `links = "sqlite3"` 的 package。
兩個 `libsqlite3-sys` 版本無法共存。

因此 Layer 2 以 ureq 直連 OD REST API，不把 `opendoc-storage` 的重型依賴塞進 plugin。

## 7. MCP 效率層

Plugin 核心引擎負責解析、硬鏈結計算、registry、drift audit。
MCP 效率層只曝露 2~3 個極簡 API 讓 Agent 發起查詢或觸發 sync：

| MCP 工具 | 對應 plugin API | 說明 |
|----------|-----------------|------|
| `opendoc_get_context` | `fetch_code_to_doc_context` | 一鍵回傳該代碼節點最精準的 spec 區塊 |
| `opendoc_audit_drift` | `audit_drift` | 檢查全專案 Spec 與 Code 的失真度 |
| `opendoc_index` | `index_docs` | 索引文件（抽取硬鏈結寫入 registry） |

MCP 工具由 graphify-mcp 在啟動時自動註冊（與 handoff plugin 相同模式）。

## 8. Graphify graph 邊界（Q1 決策）

Plugin 不直接持有 Petgraph，不修改 Graphify Core graph。

採用 **plugin 自有 registry + query-time merge**：

1. Plugin 自有 SQLite registry 儲存 spec↔symbol 鏈結。
2. 查詢時透過 `sync_toon` 取得 Graphify graph 資料 + core `query_bfs`。
3. 在 query-time merge spec 節點與 `implements_spec` 邊，組成聯集圖譜。
4. 不持久化進 Graphify Core graph。

這符合 `GraphifyPlugin` v1 無直接 graph handle 的契約，也避免修改 Graphify Core。

## 9. 相依與部署

- 插件作為 `GraphifyPlugins/graphify-plugin-opendoc` 獨立 crate。
- GraphifyRust 以 path dependency 嵌入（與 handoff 相同模式）。
- 依賴：`graphify-core`（path）、`graphify-registry`（path）、`rusqlite`（bundled）、
  `serde` / `serde_json`、`sha1`、`pulldown-cmark`、`thiserror`。dev: `tempfile`。
- **不**依賴 `opendoc-storage` / `opendoc-types`（libsqlite3-sys 衝突，見 §6.4）。
- **不**依賴 `async-trait`（業務 API 為同步）。
- **不**bundle Qdrant / 向量資料庫（Layer 2 直連 OD REST API，向量在 OD 端）。

## 10. 安全與開源去識別

- 遵循 AGENTS.md：禁止私有主機名、本地 IP、本機絕對路徑進入版本控制。
- 不使用真實金鑰值；使用環境變數或本地 gitignored 檔案。
- 所有配置為動態或相對路徑。

## 11. 效能預算

- 硬鏈結查詢：0ms（確定性文字 match + SQLite index）。
- drift audit：線性於 registry 鏈結數，每條一次 file read + sha1（doc-side）；
  code-side 需 graph 查詢（透過 sync_toon + query_bfs）。
- 16ms BFS Trace 為 Graphify Core 的效能目標，非本 plugin 的 SLA。