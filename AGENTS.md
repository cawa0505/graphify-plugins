# AGENTS.md — Graphify Plugin Workspace

本目錄是 Graphify Plugin 生態系的 workspace。所有外掛遵循 **Pure Symbol Bridge**：Graphify Core 負責確定性的 AST 拓撲與衝擊半徑；Plugin 只負責外部領域資料與 `canonical_ast_node_id` 之間的 Ingest、Resolve、Bind 與 Context 合成，不重造外部工具的執行引擎。

## Plugin 文件流程

每個 Plugin 依序完成以下文件與驗證流程：

1. **研究**：確認真實資料源、現有工具、協定與不可跨越的架構邊界；研究結果保存為可追溯文件，不以假 server、假工具名稱或 mock 能力代替事實。
2. **規格**：先定義 In-Scope、Out-of-Scope、Ingest payload、binding schema、canonical symbol mapping、MCP tools 與驗收條件，再開始實作。
3. **實作**：沿用既有 Plugin pattern，以最少模組與依賴完成 Pure Bridge；不得把領域執行引擎、AST 圖譜或 speculative abstraction 塞進 Plugin。
4. **Plugin README**：功能、限制、設定、工具介面與驗證結果必須符合目前已實作能力；若 Plugin 維護雙語 README，兩份內容同步更新。
5. **驗證**：執行該 Plugin 的測試、lint/typecheck，以及必要的 `graphify-mcp` 整合檢查；未實測的外部 connector 必須明確標記為未驗證。
6. **更新生態索引**：研究、規格、實作或重大架構里程碑完成後，必須在同一工作階段回頭更新根目錄 `index.md`，同步：
   - Plugin 名稱與定位
   - 外部資料源與 Bridge/Binding 語意
   - 已完成能力與目前限制
   - 對 Agent Context / `.toon` 的實際賦能
   - Grand Matrix 中的對應項目

`index.md` 是生態系的現況索引，不是願望清單。規劃中的能力必須標示為規劃中；只有通過驗證的能力才可描述為已完成。

## 完成條件

Plugin 里程碑只有在以下條件全部成立時才算完成：

- 實作與規格一致，沒有越過 Pure Symbol Bridge 邊界。
- 專案指定的測試與檢查通過。
- README 與實際能力一致。
- 根目錄 `index.md` 已同步更新。
