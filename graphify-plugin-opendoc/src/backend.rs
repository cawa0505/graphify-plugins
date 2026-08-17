//! Layer 2 soft search backend — `SpecSearchBackend` trait + `NoOpBackend` + `RestBackend`。
//!
//! Layer 1（硬鏈結）以 0ms 確定性匹配為主軸，Layer 2 只在硬鏈結缺席時才
//! 向 OD 的向量檢索 fallback。`RestBackend` 以 ureq 直連 OD REST API
//! （`POST /api/v1/search`，`X-Workspace` header）；未設定 OD base URL 時
//! 回退 `NoOpBackend`。網路/解析錯誤一律回空，Layer 1 不受影響、永不 panic。

use serde::Deserialize;

/// 搜尋命中結果。
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub doc_path: String,
    pub spec_id: String,
    /// OD R2：原始 heading 原文（非 slug）。plugin 據此算回內部
    /// `sha1(doc_path + heading)[0..12]` 與 `LinkRow.spec_id` 對映。
    pub heading: Option<String>,
    pub score: f64,
}

/// Layer 2 搜尋後端介面。
///
/// `od_workspace_id` 由 plugin 的 registry 查 `opendoc_workspace_mapping` 取得後傳入。
pub trait SpecSearchBackend: Send + Sync {
    /// 向 OD 搜尋 query，回傳命中結果（已排序、已 workspace 過濾）。
    fn search(&self, od_workspace_id: &str, query: &str) -> Vec<SearchHit>;
}

/// 預設 backend — 不搜尋，永遠回傳空。
///
/// plugin 在未設定 OD base URL 時使用此實作。Layer 1（硬鏈結）永遠可用。
pub struct NoOpBackend;

impl SpecSearchBackend for NoOpBackend {
    fn search(&self, _od_workspace_id: &str, _query: &str) -> Vec<SearchHit> {
        Vec::new()
    }
}

/// 真 backend — 以 ureq 直連 OD REST API（Layer 2）。
///
/// `POST {base_url}/api/v1/search`，header `X-Workspace: <od_workspace_id>`，
/// body `{"query": <query>, "top_k": 5}`，解析 `{hits:[{doc_path, spec_id,
/// heading, score, snippet}]}`。任何錯誤（網路、狀態碼、JSON 解析）→ 回空，
/// 不 panic。`snippet` 不消費（OD 端保留），serde 預設忽略多餘欄位。
pub struct RestBackend {
    pub base_url: String,
}

impl RestBackend {
    pub fn new(base_url: impl Into<String>) -> Self {
        let mut url = base_url.into();
        while url.ends_with('/') {
            url.pop();
        }
        Self { base_url: url }
    }
}

#[derive(Deserialize)]
struct OdSearchResponse {
    hits: Vec<OdHit>,
}

#[derive(Deserialize)]
struct OdHit {
    doc_path: String,
    spec_id: String,
    heading: Option<String>,
    score: f64,
}

const SEARCH_TOP_K: u32 = 5;

impl SpecSearchBackend for RestBackend {
    fn search(&self, od_workspace_id: &str, query: &str) -> Vec<SearchHit> {
        let url = format!("{}/api/v1/search", self.base_url);
        let body = serde_json::json!({"query": query, "top_k": SEARCH_TOP_K});
        let resp = ureq::post(&url)
            .set("X-Workspace", od_workspace_id)
            .send_json(body);

        let Ok(resp) = resp else { return Vec::new() };
        let Ok(parsed): std::result::Result<OdSearchResponse, _> = resp.into_json() else {
            return Vec::new();
        };
        parsed
            .hits
            .into_iter()
            .map(|h| SearchHit {
                doc_path: h.doc_path,
                spec_id: h.spec_id,
                heading: h.heading,
                score: h.score,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_returns_empty() {
        let backend = NoOpBackend;
        assert!(backend.search("any_ws", "any query").is_empty());
    }

    #[test]
    fn rest_backend_strips_trailing_slash() {
        let b = RestBackend::new("http://127.0.0.1:8080/");
        assert_eq!(b.base_url, "http://127.0.0.1:8080");
    }

    #[test]
    fn rest_backend_unreachable_returns_empty() {
        // ponytail: 用一個保證不存在的 port 測網路錯誤路徑。
        // 127.0.0.1:1 在 Linux 一定拒絕連線（權限/未綁定）。
        let backend = RestBackend::new("http://127.0.0.1:1");
        let hits = backend.search("any_ws", "any query");
        assert!(hits.is_empty(), "unreachable OD should yield empty hits, not panic");
    }

    #[test]
    fn rest_backend_parses_well_formed_response() {
        // ponytail: 不依賴 live OD，以手工 JSON 驗解析契約。
        let json = r#"{"hits":[{"doc_path":"docs/auth.md","spec_id":"slug-1","heading":"登入流程","score":0.83,"snippet":"..."}]}"#;
        let parsed: OdSearchResponse = serde_json::from_str(json).expect("valid shape");
        assert_eq!(parsed.hits.len(), 1);
        assert_eq!(parsed.hits[0].doc_path, "docs/auth.md");
        assert_eq!(parsed.hits[0].heading.as_deref(), Some("登入流程"));
        assert!((parsed.hits[0].score - 0.83).abs() < 1e-9);
    }

    #[test]
    fn rest_backend_parses_empty_hits() {
        let json = r#"{"hits":[]}"#;
        let parsed: OdSearchResponse = serde_json::from_str(json).expect("empty shape");
        assert!(parsed.hits.is_empty());
    }

    #[test]
    fn rest_backend_parses_missing_heading_as_none() {
        let json = r#"{"hits":[{"doc_path":"docs/x.md","spec_id":"s","score":0.5,"snippet":"..."}]}"#;
        let parsed: OdSearchResponse = serde_json::from_str(json).expect("missing heading");
        assert_eq!(parsed.hits[0].heading, None);
    }

    #[test]
    fn rest_backend_parses_multiple_hits_preserves_order() {
        let json = r#"{"hits":[
            {"doc_path":"a.md","spec_id":"a","heading":"A","score":0.9,"snippet":""},
            {"doc_path":"b.md","spec_id":"b","heading":"B","score":0.7,"snippet":""},
            {"doc_path":"c.md","spec_id":"c","heading":"C","score":0.5,"snippet":""}
        ]}"#;
        let parsed: OdSearchResponse = serde_json::from_str(json).expect("multi shape");
        assert_eq!(parsed.hits.len(), 3);
        // OD 已排序，plugin 原樣保留順位
        assert_eq!(parsed.hits[0].doc_path, "a.md");
        assert_eq!(parsed.hits[2].doc_path, "c.md");
    }
}