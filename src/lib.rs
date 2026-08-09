//! graphify-plugin-opendoc — 文件↔程式碼雙向追蹤與 drift 偵測。
//!
//! 三大跨域能力：
//! 1. 跨域子圖綁定（query-time `implements_spec` virtual edge）
//! 2. 雙向 drift 偵測（DocChanged / DocMissing / CodeMissing）
//! 3. 確定性硬連結 + 軟性向量檢索 fallback
//!
//! Layer 1（硬連結）零 OD 依賴；Layer 2 透過 `SpecSearchBackend`
//! trait，真 backend 由 graphify-mcp 在啟動時注入，plugin 本體不含 OD 源碼。

use std::path::{Path, PathBuf};

use graphify_core::plugin::{
    GraphUpdateEvent, GraphifyPlugin, WorkspaceContext,
};
use thiserror::Error;

pub mod backend;
pub mod links;
pub mod registry;
pub mod skill_install;
pub mod spec;
pub mod sync;

pub use backend::{NoOpBackend, SearchHit, SpecSearchBackend};
pub use links::{LinkRow, discover_doc_paths, index_docs as extract_link_rows};
pub use registry::LinkDb;
pub use spec::SpecBlock;

/// Plugin 業務 API 的錯誤型別。
#[derive(Debug, Error)]
pub enum Error {
    /// 呼叫業務 API 前 plugin 尚未 `bind`。
    #[error("plugin not bound to a workspace context")]
    NotBound,
    /// 讀取/寫入檔案失敗（doc 不存在、權限…）。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// SQLite 錯誤（連線、查詢…）。
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Drift 判定結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftStatus {
    /// 文件未變更，block signature 與索引時一致。
    UpToDate,
    /// 文件已修改，block signature 不同。
    DocChanged,
    /// 文件不存在。
    DocMissing,
    /// 文件中的符號在程式碼 graph 中找不到（doc→code 方向）。
    CodeMissing,
}

/// 一筆 drift 報告項。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftItem {
    pub spec_id: String,
    pub doc_path: String,
    pub symbol: String,
    pub status: DriftStatus,
}

/// 廠列插件實例。
#[derive(Default)]
pub struct OpendocPlugin {
    ctx: Option<WorkspaceContext>,
    registry_path: Option<PathBuf>,
    backend: Option<Box<dyn SpecSearchBackend>>,
}

impl OpendocPlugin {
    /// 預設建構；以 [`registry_db_path`] 與 [`NoOpBackend`] 起始。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注入 registry 路徑（覆寫 XDG 預設）。graphify-mcp 注入全域 graphify.db。
    #[must_use]
    pub fn with_registry_path(mut self, path: PathBuf) -> Self {
        self.registry_path = Some(path);
        self
    }

    /// 注入 Layer 2 backend。預設為 [`NoOpBackend`]。
    #[must_use]
    pub fn with_backend(mut self, backend: Box<dyn SpecSearchBackend>) -> Self {
        self.backend = Some(backend);
        self
    }

    /// CLI 測試輔助：以 cwd 合成 `WorkspaceContext` 直接 `bind`。
    #[must_use]
    pub fn bind_for_cli(mut self, cwd: impl AsRef<Path>) -> Self {
        let cwd_ref = cwd.as_ref();
        let workspace_key = graphify_core::plugin::derive_workspace_key(cwd_ref);
        let name = cwd_ref
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "workspace".to_string());
        let ctx = WorkspaceContext::new(workspace_key, name, cwd_ref.to_string_lossy().into_owned());
        self.bind(ctx);
        self
    }

    fn require_ctx(&self) -> Result<&WorkspaceContext, Error> {
        self.ctx.as_ref().ok_or(Error::NotBound)
    }

    fn registry_path(&self) -> PathBuf {
        self.registry_path
            .clone()
            .unwrap_or_else(graphify_registry::registry_db_path)
    }

    fn backend(&self) -> &dyn SpecSearchBackend {
        self.backend
            .as_deref()
            .unwrap_or(&NOOP_BEACON_REF)
    }

    // ── 業務 API ──────────────────────────────────────────────────────────

    /// 全量索引：掃描 `ctx.root_path` 下所有 `.md` 文件，重建硬連結 registry。
    ///
    /// Layer 1 的「索引」不需要 OD 参与；只是解析 Markdown → spec block →
    /// symbols → registry 列。
    ///
    /// 返回建立的 [`LinkRow`] 數量。
    pub fn index_all_docs(&self) -> Result<usize, Error> {
        let ctx = self.require_ctx()?;
        let doc_paths = discover_doc_paths(Path::new(&ctx.root_path));
        let rows = extract_link_rows(Path::new(&ctx.root_path), &doc_paths, &ctx.workspace_key);
        let n = rows.len();
        let db = LinkDb::open(&self.registry_path())?;
        db.replace_links(&ctx.workspace_key, &rows)?;
        Ok(n)
    }

    /// 顯式指定 doc 路徑集的索引（CLI/測試可列出特定 doc）。doc_path 為
    /// root 相對路徑字串。
    pub fn index_doc_paths(&self, doc_paths: &[String]) -> Result<usize, Error> {
        let ctx = self.require_ctx()?;
        let rows = extract_link_rows(Path::new(&ctx.root_path), doc_paths, &ctx.workspace_key);
        let n = rows.len();
        let db = LinkDb::open(&self.registry_path())?;
        db.replace_links(&ctx.workspace_key, &rows)?;
        Ok(n)
    }

    /// doc → code 方向：「改了 docs/auth.md，哪些 symbols 受影響？」
    pub fn trace_doc_to_code(&self, doc_path: &str) -> Result<Vec<LinkRow>, Error> {
        let ctx = self.require_ctx()?;
        let db = LinkDb::open(&self.registry_path())?;
        Ok(db.query_by_doc(&ctx.workspace_key, doc_path)?)
    }

    /// code → doc 方向：「改了 `crate::auth::verify_token`，哪份 spec 描述它？」
    ///
    /// Layer 1 優先查硬連結；若沒有，且 plugin 已注入 Layer 2 backend，
    /// 才向 OD 查 `SearchHit`（回傳 [`LinkRow`] 空 signature）。
    pub fn fetch_code_to_doc_context(&self, symbol: &str) -> Result<Vec<LinkRow>, Error> {
        let ctx = self.require_ctx()?;
        let db = LinkDb::open(&self.registry_path())?;
        let hard = db.query_by_symbol(&ctx.workspace_key, symbol)?;
        if !hard.is_empty() {
            return Ok(hard);
        }
        // Layer 2 fallback
        let od_id = db.get_workspace_mapping(&ctx.workspace_key)?;
        if let Some(od_id) = od_id {
            let hits = self.backend().search(&od_id, symbol);
            return Ok(hits
                .into_iter()
                .map(|h| LinkRow {
                    workspace_key: ctx.workspace_key.clone(),
                    doc_path: h.doc_path,
                    spec_id: h.spec_id,
                    symbol: symbol.to_string(),
                    signature: String::new(),
                })
                .collect());
        }
        Ok(Vec::new())
    }

    /// doc-side drift：對每個 indexed link 重新讀檔、解析、比對 block signature。
    pub fn audit_drift(&self) -> Result<Vec<DriftItem>, Error> {
        let ctx = self.require_ctx()?;
        let db = LinkDb::open(&self.registry_path())?;
        let all = db.all_links(&ctx.workspace_key)?;
        let mut items = Vec::new();
        for row in &all {
            let full = Path::new(&ctx.root_path).join(&row.doc_path);
            let md = match std::fs::read_to_string(&full) {
                Ok(m) => m,
                Err(_) => {
                    items.push(DriftItem {
                        spec_id: row.spec_id.clone(),
                        doc_path: row.doc_path.clone(),
                        symbol: row.symbol.clone(),
                        status: DriftStatus::DocMissing,
                    });
                    continue;
                }
            };
            let blocks = spec::extract_blocks(&md, &row.doc_path);
            let status = match blocks.into_iter().find(|b| b.spec_id == row.spec_id) {
                None => DriftStatus::DocMissing,
                Some(b) if b.block_signature == row.signature => DriftStatus::UpToDate,
                Some(_) => DriftStatus::DocChanged,
            };
            items.push(DriftItem {
                spec_id: row.spec_id.clone(),
                doc_path: row.doc_path.clone(),
                symbol: row.symbol.clone(),
                status,
            });
        }
        Ok(items)
    }

    /// doc→code drift：spec block 宣告的 symbol 在 graph 中找不到。
    ///
    /// `known_symbols` 由 caller 透過 graphify-core 的 graph 查詢供給
    /// （plugin 本身不持有 graph handle——v1 契約）。
    pub fn audit_code_missing(
        &self,
        known_symbols: &[String],
    ) -> Result<Vec<DriftItem>, Error> {
        let ctx = self.require_ctx()?;
        let db = LinkDb::open(&self.registry_path())?;
        let all = db.all_links(&ctx.workspace_key)?;
        let known: std::collections::HashSet<&String> = known_symbols.iter().collect();
        Ok(all
            .into_iter()
            .filter(|r| !known.contains(&r.symbol))
            .map(|r| DriftItem {
                spec_id: r.spec_id,
                doc_path: r.doc_path,
                symbol: r.symbol,
                status: DriftStatus::CodeMissing,
            })
            .collect())
    }

    /// 設定 `workspace_key → od_workspace_id` 對映（Layer 2 用）。手動建立。
    pub fn set_workspace_mapping(&self, od_workspace_id: &str) -> Result<(), Error> {
        let ctx = self.require_ctx()?;
        let db = LinkDb::open(&self.registry_path())?;
        db.set_workspace_mapping(&ctx.workspace_key, od_workspace_id)?;
        Ok(())
    }

    /// 取得已設定的 `od_workspace_id`（除錯/查詢用）。
    pub fn get_workspace_mapping(&self) -> Result<Option<String>, Error> {
        let ctx = self.require_ctx()?;
        let db = LinkDb::open(&self.registry_path())?;
        Ok(db.get_workspace_mapping(&ctx.workspace_key)?)
    }
}

impl GraphifyPlugin for OpendocPlugin {
    fn get_id(&self) -> &str {
        "graphify-plugin-opendoc"
    }

    fn bind(&mut self, ctx: WorkspaceContext) {
        self.ctx = Some(ctx);
    }

    fn get_workspace_key(&self) -> &str {
        static EMPTY: String = String::new();
        match &self.ctx {
            Some(ctx) => &ctx.workspace_key,
            None => &EMPTY,
        }
    }

    fn sync_toon(&mut self, opt_toon: Option<Vec<u8>>) -> Vec<u8> {
        let workspace_key = self.get_workspace_key().to_string();
        let produce_state = || {
            let link_count = self
                .ctx
                .as_ref()
                .and_then(|_| {
                    LinkDb::open(&self.registry_path())
                        .ok()
                        .and_then(|db| db.all_links(&workspace_key).ok())
                })
                .map(|l| l.len())
                .unwrap_or(0);
            let mapping = self
                .ctx
                .as_ref()
                .and_then(|_| {
                    LinkDb::open(&self.registry_path())
                        .ok()
                        .and_then(|db| db.get_workspace_mapping(&workspace_key).ok().flatten())
                })
                .unwrap_or_default();
            serde_json::json!({
                "opendoc": {
                    "link_count": link_count,
                    "od_workspace_id": mapping,
                }
            })
        };
        match opt_toon {
            None => sync::emit_packet(&workspace_key, &produce_state()).into_bytes(),
            Some(bytes) => {
                let toon = String::from_utf8_lossy(&bytes);
                let meta = sync::parse_meta(&toon);
                if let Some(fv) = &meta.format_version {
                    if sync::major_mismatch(fv) {
                        return sync::emit_error_packet(&format!(
                            "Major version mismatch: plugin v1 cannot read v{}",
                            fv
                        ))
                        .into_bytes();
                    }
                }
                if let Some(e) = &meta.error {
                    return sync::emit_error_packet(e).into_bytes();
                }
                sync::emit_packet(&workspace_key, &produce_state()).into_bytes()
            }
        }
    }

    fn on_graph_updated(&mut self, _event: &GraphUpdateEvent) {
        // v1 no-op. Future: trigger audit_drift against the modified nodes
        // (graph_changed → doc stale check).
    }
}

// ponytail: static NoOp 節省一次 alloc，`backend()` 免 Box::new。一旦
// plugin 被注入真 backend，此 static 永遠碰不到。升級路徑 = `OnceCell` 或
// 動態注入；對單元測試與 MCP 啟動足夠。
static NOOP_BEACON_REF: NoOpBackend = NoOpBackend;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn make_plugin(dir: &tempfile::TempDir) -> OpendocPlugin {
        OpendocPlugin::new()
            .with_registry_path(dir.path().join("links.db"))
            .bind_for_cli(dir.path().to_string_lossy().into_owned())
    }

    fn write_doc(dir: &tempfile::TempDir, name: &str, content: &str) {
        fs::write(dir.path().join(name), content).unwrap();
    }

    // ── GraphifyPlugin trait ────────────────────────────────────────────

    #[test]
    fn plugin_id_is_stable() {
        let p = OpendocPlugin::new();
        assert_eq!(p.get_id(), "graphify-plugin-opendoc");
    }

    #[test]
    fn unbound_workspace_key_is_empty() {
        let p = OpendocPlugin::new();
        assert_eq!(p.get_workspace_key(), "");
        assert!(matches!(p.require_ctx(), Err(Error::NotBound)));
    }

    #[test]
    fn bind_roundtrips_workspace_key() {
        let mut p = OpendocPlugin::new();
        let ctx = WorkspaceContext::new("w-abc", "ws", "/tmp");
        p.bind(ctx);
        assert_eq!(p.get_workspace_key(), "w-abc");
    }

    #[test]
    fn sync_toon_proactive_emits_metadata() {
        let dir = tempdir().unwrap();
        let mut p = make_plugin(&dir);
        let bytes = p.sync_toon(None);
        let s = String::from_utf8(bytes).unwrap();
        let meta = sync::parse_meta(&s);
        assert_eq!(meta.format_version.as_deref(), Some("1.0.0"));
        assert!(meta.workspace_key.is_some());
    }

    #[test]
    fn sync_toon_passive_major_mismatch_returns_error() {
        let dir = tempdir().unwrap();
        let mut p = make_plugin(&dir);
        // FAKE 2.0.0 packet
        let bad_packet = "metadata:\n  format_version: \"2.0.0\"\n  workspace_key: \"w-x\"\n";
        let out = p.sync_toon(Some(bad_packet.as_bytes().to_vec()));
        let s = String::from_utf8(out).unwrap();
        let meta = sync::parse_meta(&s);
        assert!(meta.error.is_some(), "must return error for v2 packet");
        assert!(meta.error.unwrap().contains("Major version"));
    }

    // ── index_docs + trace_doc_to_code + fetch_code_to_doc_context ──────

    #[test]
    fn index_all_docs_persists_links() {
        let dir = tempdir().unwrap();
        write_doc(
            &dir,
            "auth.md",
            "## verify_token\n\nDesc.\n\n# Symbol: crate::auth::verify_token\n",
        );
        let p = make_plugin(&dir);
        let n = p.index_all_docs().unwrap();
        assert_eq!(n, 1, "one Symbol row from auth.md");
    }

    #[test]
    fn trace_doc_to_code_returns_links_for_doc() {
        let dir = tempdir().unwrap();
        write_doc(
            &dir,
            "auth.md",
            "## verify_token\n\nDesc.\n\n# Symbol: crate::auth::verify_token\n\n# Symbol: crate::auth::login\n",
        );
        let p = make_plugin(&dir);
        p.index_all_docs().unwrap();
        let rows = p.trace_doc_to_code("auth.md").unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.symbol == "crate::auth::verify_token"));
        assert!(rows.iter().any(|r| r.symbol == "crate::auth::login"));
    }

    #[test]
    fn fetch_code_to_doc_context_returns_links_for_symbol() {
        let dir = tempdir().unwrap();
        write_doc(
            &dir,
            "db.md",
            "## get_user\n\nUser lookup.\n\n# Symbol: crate::db::get_user\n",
        );
        let p = make_plugin(&dir);
        p.index_all_docs().unwrap();
        let rows = p.fetch_code_to_doc_context("crate::db::get_user").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].doc_path, "db.md");
    }

    #[test]
    fn fetch_code_to_doc_context_unknown_symbol_returns_empty() {
        let dir = tempdir().unwrap();
        write_doc(&dir, "x.md", "## x\n\n\n# Symbol: x\n");
        let p = make_plugin(&dir);
        p.index_all_docs().unwrap();
        let rows = p.fetch_code_to_doc_context("nonexistent").unwrap();
        assert!(rows.is_empty());
    }

    // ── audit_drift ─────────────────────────────────────────────────────

    #[test]
    fn audit_drift_up_to_date() {
        let dir = tempdir().unwrap();
        write_doc(
            &dir,
            "auth.md",
            "## verify_token\n\nOriginal.\n\n# Symbol: crate::auth::verify_token\n",
        );
        let p = make_plugin(&dir);
        p.index_all_docs().unwrap();
        let items = p.audit_drift().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].status, DriftStatus::UpToDate);
    }

    #[test]
    fn audit_drift_doc_changed() {
        let dir = tempdir().unwrap();
        write_doc(
            &dir,
            "auth.md",
            "## verify_token\n\nOriginal.\n\n# Symbol: crate::auth::verify_token\n",
        );
        let p = make_plugin(&dir);
        p.index_all_docs().unwrap();
        // Modify file content
        write_doc(
            &dir,
            "auth.md",
            "## verify_token\n\nModified content.\n\n# Symbol: crate::auth::verify_token\n",
        );
        let items = p.audit_drift().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].status, DriftStatus::DocChanged);
    }

    #[test]
    fn audit_drift_doc_missing() {
        let dir = tempdir().unwrap();
        write_doc(
            &dir,
            "auth.md",
            "## verify_token\n\nOriginal.\n\n# Symbol: crate::auth::verify_token\n",
        );
        let p = make_plugin(&dir);
        p.index_all_docs().unwrap();
        // Remove file
        fs::remove_file(dir.path().join("auth.md")).unwrap();
        let items = p.audit_drift().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].status, DriftStatus::DocMissing);
    }

    // ── audit_code_missing ──────────────────────────────────────────────

    #[test]
    fn audit_code_missing_finds_unimplemented_symbols() {
        let dir = tempdir().unwrap();
        write_doc(
            &dir,
            "auth.md",
            "## verify_token\n\nA.\n\n# Symbol: crate::auth::verify_token\n\n## login\n\nB.\n\n# Symbol: crate::auth::login\n",
        );
        let p = make_plugin(&dir);
        p.index_all_docs().unwrap();
        // Code graph only contains verify_token, not login
        let missing = p
            .audit_code_missing(&["crate::auth::verify_token".to_string()])
            .unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].symbol, "crate::auth::login");
        assert_eq!(missing[0].status, DriftStatus::CodeMissing);
    }

    #[test]
    fn audit_code_missing_empty_when_all_known() {
        let dir = tempdir().unwrap();
        write_doc(
            &dir,
            "auth.md",
            "## v\n\nA.\n\n# Symbol: crate::auth::verify_token\n",
        );
        let p = make_plugin(&dir);
        p.index_all_docs().unwrap();
        let missing = p.audit_code_missing(&["crate::auth::verify_token".to_string()]).unwrap();
        assert!(missing.is_empty());
    }

    // ── workspace mapping ───────────────────────────────────────────────

    #[test]
    fn workspace_mapping_roundtrip() {
        let dir = tempdir().unwrap();
        let p = make_plugin(&dir);
        assert!(p.get_workspace_mapping().unwrap().is_none());
        p.set_workspace_mapping("od-uuid-123").unwrap();
        assert_eq!(
            p.get_workspace_mapping().unwrap(),
            Some("od-uuid-123".to_string())
        );
    }

    // ── on_graph_updated no-op ──────────────────────────────────────────

    #[test]
    fn on_graph_updated_is_noop_without_panic() {
        let mut p = make_plugin(&tempdir().unwrap());
        let event = GraphUpdateEvent::new(
            p.get_workspace_key().to_string(),
            Vec::new(),
            graphify_core::plugin::GraphUpdateKind::Manual,
        );
        p.on_graph_updated(&event);
        // No assertion needed — must not panic.
    }

    // ── sync_toon_state reflects indexed links ───────────────────────────

    #[test]
    fn sync_toon_reflects_link_count_after_index() {
        let dir = tempdir().unwrap();
        write_doc(
            &dir,
            "auth.md",
            "## v\n\nA.\n\n# Symbol: crate::auth::verify_token\n",
        );
        let mut p = make_plugin(&dir);
        p.index_all_docs().unwrap();
        let bytes = p.sync_toon(None);
        let s = String::from_utf8(bytes).unwrap();
        // packet embeds JSON in YAML (escaped quotes), so check the unescaped keyword.
        assert!(s.contains("link_count"), "missing link_count in packet: {s}");
    }
}