# Design: Embedded Plugin Architecture — graphify-plugin-handoff

## 1. 架構定位：單一 Crate 作為 Graphify 內嵌型 Plugin

- **`graphify-plugin-handoff`**：單一 Cargo crate（`lib.rs` 為主，`src/bin/cli.rs` 為選用 CLI）
- **無獨立 MCP Server**：不透過 stdio JSON-RPC 與 AI client 直接溝通。
- **Plugin 介面契約**：實作 `graphify-core` 定義的 `GraphifyPlugin` trait（v1，已定案，`graphify-core/src/plugin.rs`）。
  - `get_id(&self) -> &str`：唯一標識插件（如 `"graphify-plugin-handoff"`）
  - `bind(&mut self, ctx: WorkspaceContext)`：由 Graphify 注入單一工作區上下文（**by value**；`WorkspaceContext { workspace_key, workspace_name, root_path, timestamp }`）
  - `get_workspace_key(&self) -> &str`：傳回當前 workspace 路由鍵（SipHash hex，v1 契約定義）；未 bind 回傳空字串
  - `sync_toon(&mut self, opt_toon: Option<Vec<u8>>) -> Vec<u8>`：.toon 封包同步（sync-toon-packet 規格，見 §5）
  - `on_graph_updated(&mut self, event: &GraphUpdateEvent)`：圖更新通知（預設 no-op，可覆寫）
  - relay* 操作（init/save/close/switch/resume/status/add）為 crate **公開 API 函式**，非 trait 方法；由 GraphifyMCP 對映為 MCP tools。

## 2. Root 解析模型：Graphify 的 Workspace Context 注入

- **解析規則**：v1 `WorkspaceContext` 承載單一工作區（`workspace_key` / `workspace_name` / `root_path` / `timestamp`），**不含** repo 清單。relay root 於 `bind(ctx)` 時由 `ctx.root_path` 一次定位並常駐記憶體快取（不重複 walk-up）。
- **具名 repo 操作**（relaySave/Close/Resume/Switch with `repo`）：一律走 state 內 `repos[name].path` registry 查表（root-relative），零 walk-up。
- **無 repo 操作**（relayStatus、relayResume 不帶 repo）：使用啟動時快取的 root。
- **relayInit**：唯一允許的寫入式 walk-up — 從 cwd 向上找第一個含 `relay.json` 的目錄；找到即拒絕，否則在 cwd 建立新 root 並快取。
- **Fail-fast**：任何非 init 工具在無快取 root 且無 `repo` 參數時 → 回傳 `No relay.json found. Run relayInit first.`；`repo` 不存在於 registry → 回傳 `RepoNotFound`，不做無界搜尋。

## 3. 寫入同步與安全 (Lazy-Write & File Lock)

- **Lazy-Write**：僅在 `save`、`close` 造成實質狀態變更時，將變更刷入磁碟。
- **Atomic & Lock**：透過 `fs2` 取得排他鎖，採用「寫入臨時檔 + OS rename」的 Atomic 寫入法，確保資料 100% 安全。
- **狀態檔案位置**：`relay.json`（schema 1.0.0，protocol 語意權威，向後相容既有工具）＋ `.relay/relay.toon`（TOON 序列化鏡像，短程記憶路徑）；寫入路徑雙寫。sync_toon 交換時，relay 狀態放入 .toon 的 `metadata.plugin_data.<plugin_id>` 容器（core 契約保留容器），**不得**新增頂層 metadata 欄位。

## 4. TOON 狀態模型（由 Graphify 插件共用）

`RelayState`（serde 可序列化）：
```toml
session_id           # 當前 relay session
phase                # init | active | saved | closed
volatile_state       # 進行中的工作內容
open_threads         # 待處理的任務清單
specs_hash           # spec 檔案的 hash，用於偵測變更
confidence           # 0.0~1.0，表示狀態完整度
```

## 5. Plugin SDK 擴展點

- **GraphifyMCP 自動註冊**：`graphify-mcp`（GraphifyRust 側）啟動時掃描載入的 plugin，依公開 API 自動註冊 `relayInit` / `relaySave` / `relayResume` / `relayStatus` / `relaySwitch` / `relayAdd` / `relayClose` 為 MCP tools。**此 repo 只提供公開 API 與工具語意（PROTOCOL.md），MCP 對映屬 GraphifyRust 整合範圍，不在此 crate 實作。**
- **sync_toon 封包契約**（`openspec/specs/sync-toon-packet/spec.md`）：
  - 封包為 .toon 文件；metadata MUST 含 `format_version`（`"1.0.0"`）與 `workspace_key`（與 `get_workspace_key()` 一致）。
  - 主動同步（`None`）以綁定上下文自產輸出，不得 panic；無法產出時回傳 metadata 含 `error` 的 .toon。
  - 可選承載：`symbol_nodes` / `graph_topology`；解析端必須容忍缺席。
  - 版本政策：同 MAJOR 可互操作，MAJOR 提升可拒絕並以 `error` metadata 回應。
- **跨 Plugin 間協作**：透過 `workspace_key` 將此 plugin 的 relay state 與 `graphify-plugin-opendoc`、`graphify-plugin-review` 等串連。
- **檔案位置**：plugin 僅負責邏輯；檔案讀寫與鎖機制在此 crate 內完成，GraphifyCore 不依賴此 crate 的檔案 I/O。

## 6. 依賴清單

```toml
[dependencies]
graphify-core = { path = "../../GraphifyRust/graphify-core" }  # GraphifyPlugin trait（v1）
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"
fs2 = "0.4"     # 檔案鎖（寫入安全）
sha1 = "0.10"   # spec hash（sha1-12，相容既有 relay.json）
chrono = "0.4"  # ISO-8601 UTC 時間戳（ms）
```

> 備註：`graphify-core` 為 path dependency（`../../GraphifyRust/graphify-core`），契約由 GraphifyRust 維護；本 repo 只消費其公開 API，不修改 core。

## 7. 開發指令

```bash
cargo check                 # 編譯檢查
cargo test --doc            # 單元測試
cargo clippy --all-targets  # 程式碼檢查
```