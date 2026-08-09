//! Layer 2 soft search backend — `SpecSearchBackend` trait + `NoOpBackend`。
//!
//! Layer 1（硬鏈結）以 0ms 確定性匹配為主軸，Layer 2 只在硬鏈結缺席時才
//! 向 OD 的向量檢索 fallback。真 backend（MCP 轉發）由 graphify-mcp 在啟動
//! 時注入；plugin 本體零 OD 依賴。

/// 搜尋命中結果。
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub doc_path: String,
    pub spec_id: String,
    pub score: f64,
}

/// Layer 2 搜尋後端介面。真實作由 graphify-mcp 的 MCP-to-MCP 轉發注入。
///
/// `od_workspace_id` 由 plugin 的 registry 查 `opendoc_workspace_mapping` 取得後傳入；
/// backend 本身持有此值。
pub trait SpecSearchBackend: Send + Sync {
    /// 向 OD 搜尋 query，回傳命中結果（已排序、已 workspace 過濾）。
    fn search(&self, od_workspace_id: &str, query: &str) -> Vec<SearchHit>;
}

/// 預設 backend — 不搜尋，永遠回傳空。
///
/// plugin 在未注入真 backend 時使用此實作。Layer 1（硬鏈結）永遠可用。
pub struct NoOpBackend;

impl SpecSearchBackend for NoOpBackend {
    fn search(&self, _od_workspace_id: &str, _query: &str) -> Vec<SearchHit> {
        Vec::new()
    }
}