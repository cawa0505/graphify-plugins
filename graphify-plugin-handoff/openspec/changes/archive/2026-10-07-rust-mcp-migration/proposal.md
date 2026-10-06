# Change: Embedded Plugin Architecture & Stateful Caching

## Why
1. 原版 Node.js 插件每次執行工具或 Compacting 鉤子時，皆為無狀態（Stateless）進程，必須重複往上層目錄搜尋（walk-up）定位 `relay.json`；且 skill 指令「習慣性往上找」，把 root 解析外包給 LLM，造成重複 I/O、nested repo 誤判與不確定性。
2. node_modules 依賴與 Node.js 執行期體積龐大，不利於輕量跨平台部署。
3. **新需求**：所有 Graphify 插件（review, handoff, opendoc）應以 **Graphify 內嵌型 crate** 形式存在，透過 `GraphifyPlugin` trait 與 core 結合，避免額外進程開銷與序列化延遲。

## What Changes
1. 將核心功能重構為 **Graphify 內嵌型 crate**（`graphify-plugin-handoff` lib.rs），擺脫 Node.js / node_modules，同時避免額外 MCP 伺服器開銷。
2. 導入 **Graphify Plugin Trait Contract**（v1，`graphify-core/src/plugin.rs`，已定案）：
   - `get_id(&self) -> &str`
   - `bind(&mut self, ctx: WorkspaceContext)`（by value；`WorkspaceContext { workspace_key, workspace_name, root_path, timestamp }`）
   - `get_workspace_key(&self) -> &str`
   - `sync_toon(&mut self, opt_toon: Option<Vec<u8>>) -> Vec<u8>`（.toon 封包交換，sync-toon-packet 規格）
   - `on_graph_updated(&mut self, event: &GraphUpdateEvent)`（預設 no-op）
   - relay* 操作（init/save/close/switch/resume/status/add）以 crate 公開 API 函式提供，非 trait 方法。
3. 維持 **長駐型 Workspace Root 快取機制**（透過 Graphify 注入的 `WorkspaceContext`），僅在啟動時執行一次磁碟定位；具名 repo 操作改走 `repos[name].path` registry 查表；後續請求完全在記憶體中（Stateful Memory Cache）直接響應。
4. 更新 **Slim Code-Relay Skill**（單一 `SKILL.md`）：
   - 開場：呼叫 `relayResume` 讀取 active repo 交接
   - 收尾：`relaySave`（進行中）或 `relayClose`（完成）
   - 跨 repo：`relayStatus` → `relaySwitch`
   - 明令：relay 狀態一律經由 Graphify Plugin 介面存取，禁止自行 walk-up。
5. `INTEGRATION.md` 仍適用：描述與 `opendoc-mcp`（文件 RAG）與 `graphify`（程式碼知識圖）的整合 use case。

## Impact
- **效能提升**：查詢延遲降至 <1ms（workspace context 注入僅啟動一次，之後為記憶體操作）。
- **開發簡化**：單一 crate 結構，消除 Cargo workspace 複雜度。
- **完全相容**：**[待討論]** 是否保留 Slim 版 Node.js 包裝器以維持舊版 `npx opencode-code-relay-plugin` 向後相容，視實際使用需求而定。
