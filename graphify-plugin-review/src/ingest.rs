//! code-review-graph 輸入協定（IngestPayload）— file-based import。
//!
//! 契約（openspec design.md §3.1）：
//! ```json
//! {
//!   "version": "1.0",
//!   "source": "code-review-graph",
//!   "workspace_key": "my-app-v1",
//!   "reviews": [ { "review_id": "...", "file_path": "src/auth.rs",
//!                  "line_number": 42, "severity": "high", "category": "security",
//!                  "comment": "...", "created_at": "..." } ]
//! }
//! ```
//!
//! Slice 0 走確定性 file-based import（零 CRG 介面風險）；CRG MCP 即時
//! analysis 接入（選項 A）留給 Slice 1/2 的 `crg_client`。

use serde::Deserialize;

/// 檔案匯入的頂層契約。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct IngestPayload {
    pub version: String,
    #[serde(default)]
    pub source: String,
    pub workspace_key: String,
    #[serde(default)]
    pub reviews: Vec<ReviewItem>,
}

/// 單筆 review 記錄（CRG 產出格式）。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ReviewItem {
    pub review_id: String,
    pub file_path: String,
    pub line_number: u32,
    pub severity: String,
    pub category: String,
    pub comment: String,
    pub created_at: String,
}

/// 解析 IngestPayload JSON。
///
/// # Errors
/// JSON 格式不符或欄位缺漏時回傳 [`serde_json::Error`]。
pub fn parse_payload(json: &str) -> Result<IngestPayload, serde_json::Error> {
    serde_json::from_str(json)
}

/// 讀取並解析一個 IngestPayload 檔。
///
/// # Errors
/// 檔案不存在 / 讀取失敗回傳 [`std::io::Error`]，格式不符回傳
/// [`serde_json::Error`]。
pub fn parse_payload_file(path: &std::path::Path) -> Result<IngestPayload, IngestError> {
    let raw = std::fs::read_to_string(path)?;
    Ok(parse_payload(&raw)?)
}

/// ingest 解析階段的錯誤。
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("db error: {0}")]
    Db(#[from] rusqlite::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_payload() {
        let json = r#"{
            "version": "1.0",
            "source": "code-review-graph",
            "workspace_key": "my-app-v1",
            "reviews": [
                {
                    "review_id": "crg-sec-001",
                    "file_path": "src/auth.rs",
                    "line_number": 42,
                    "severity": "high",
                    "category": "security",
                    "comment": "Potential timing attack on HMAC token comparison.",
                    "created_at": "2026-08-10T00:00:00Z"
                }
            ]
        }"#;
        let payload = parse_payload(json).unwrap();
        assert_eq!(payload.version, "1.0");
        assert_eq!(payload.workspace_key, "my-app-v1");
        assert_eq!(payload.reviews.len(), 1);
        assert_eq!(payload.reviews[0].review_id, "crg-sec-001");
        assert_eq!(payload.reviews[0].line_number, 42);
    }

    #[test]
    fn empty_reviews_allowed() {
        let payload = parse_payload(r#"{"version":"1.0","workspace_key":"w"}"#).unwrap();
        assert!(payload.reviews.is_empty());
    }

    #[test]
    fn missing_workspace_key_fails() {
        assert!(parse_payload(r#"{"version":"1.0"}"#).is_err());
    }

    #[test]
    fn parse_payload_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reviews.json");
        std::fs::write(&path, r#"{"version":"1.0","workspace_key":"w","reviews":[]}"#).unwrap();
        let payload = parse_payload_file(&path).unwrap();
        assert_eq!(payload.workspace_key, "w");
    }

    #[test]
    fn parse_missing_file_errors() {
        let err = parse_payload_file(std::path::Path::new("/nonexistent/x.json")).unwrap_err();
        assert!(matches!(err, IngestError::Io(_)));
    }
}
