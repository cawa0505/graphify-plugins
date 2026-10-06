## ADDED Requirements

### Requirement: Workspace Context Injection (Single Walk-up by Graphify)
plugin MUST 取得 workspace 根路径與 repo 注册表，*唯* 透過 `GraphifyPlugin::bind` 注入的 `WorkspaceContext`：
- `workspace_key`：跨 session 揮發的唯一鑑別碼
- `repo_paths: HashMap<String, PathBuf>`：repo_name → repo_root
- `primary_repo: Option<String>`：當前活躍 repo

#### Scenario: Sub-directory execution without I/O penalty
- **WHEN** Graphify 啟動時完成 workspace 定位與 `WorkspaceContext` 注入
- **THEN** plugin 內部的所有路徑操作皆應在 `< 1ms` 內，直接使用記憶體中的 `repo_paths` 查表回應，且 MUST NOT 在硬碟上重複進行 walk-up。

### Requirement: Registry Lookup for Named Repo Operations
具名 repo 操作（`relaySave`、`relayClose`、`relayResume`、`relaySwitch` 帶 `repo`）MUST 直接以 `repo_paths[name]` 從 `WorkspaceContext` 查表解析，且 MUST NOT 進行任何 walk-up。

#### Scenario: Cross-repo switch without directory search
- **WHEN** plugin 在任一目錄呼叫帶 `repo` 的具名操作
- **THEN** 系統僅以 registry 查表解析路徑並在 <1ms 內回應，不存取檔案系統進行搜尋

### Requirement: Fail-fast on Missing Root
當 `repo` 不在 `repo_paths` 內時，plugin MUST 立即回傳明確錯誤（`RepoNotFound`），且 MUST NOT 向上無界搜尋或猜測 root。

#### Scenario: Fresh directory without relay root
- **WHEN** plugin 在尚未初始化 relay 的 repo 呼叫無 `repo` 操作
- **THEN** 系統回傳「未找到 repo，請確認 `WorkspaceContext` 包含正確的 repo 注册表」錯誤，且不進行額外磁碟搜尋