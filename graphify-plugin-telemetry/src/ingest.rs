//! Draco 輸入協定（TelemetryIngestPayload）— file-based import。
//!
//! 契約（§3.1）：
//! ```json
//! {
//!   "version": "1.0",
//!   "source": "draco-mcp",
//!   "workspace_key": "my-app-v1",
//!   "metrics": [
//!     { "metric_id": "tel-p99-001", "file_path": "src/db/query.rs",
//!       "function_name": "query_users", "line_number": 88,
//!       "p50_ms": 12.5, "p99_ms": 1250.0,
//!       "alloc_bytes_per_req": 10485760, "call_count_per_min": 5000,
//!       "environment": "production", "recorded_at": "2026-08-10T06:00:00Z" }
//!   ]
//! }
//! ```
//!
//! Slice 0 走確定性 file-based import（零 Draco 介面風險）；Draco MCP 即時
//! 抓取（`fetch_top_hotspots` 等）留給 Slice 1/2 的 `draco_client`。

use serde::Deserialize;

/// 檔案匯入的頂層契約。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TelemetryIngestPayload {
    pub version: String,
    #[serde(default)]
    pub source: String,
    pub workspace_key: String,
    #[serde(default)]
    pub metrics: Vec<Metric>,
}

/// 單筆效能指標記錄（Draco 產出格式；欄位對齊 §3.1）。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Metric {
    pub metric_id: String,
    pub file_path: String,
    pub function_name: String,
    pub line_number: u32,
    pub p50_ms: f64,
    pub p99_ms: f64,
    #[serde(default)]
    pub alloc_bytes_per_req: i64,
    #[serde(default)]
    pub call_count_per_min: i64,
    #[serde(default = "default_environment")]
    pub environment: String,
    pub recorded_at: String,
}

fn default_environment() -> String {
    "production".to_string()
}

/// 解析 TelemetryIngestPayload JSON。
///
/// # Errors
/// JSON 格式不符或欄位缺漏時回傳 [`serde_json::Error`]。
pub fn parse_payload(json: &str) -> Result<TelemetryIngestPayload, serde_json::Error> {
    serde_json::from_str(json)
}

/// 讀取並解析一個 TelemetryIngestPayload 檔。
///
/// # Errors
/// 檔案不存在 / 讀取失敗回傳 [`std::io::Error`]，格式不符回傳
/// [`serde_json::Error`]。
pub fn parse_payload_file(path: &std::path::Path) -> Result<TelemetryIngestPayload, IngestError> {
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
    #[error("draco error: {0}")]
    Draco(#[from] crate::draco_client::DracoError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_payload() {
        let json = r#"{
            "version": "1.0",
            "source": "draco-mcp",
            "workspace_key": "my-app-v1",
            "metrics": [
                {
                    "metric_id": "tel-p99-001",
                    "file_path": "src/db/query.rs",
                    "function_name": "query_users",
                    "line_number": 88,
                    "p50_ms": 12.5,
                    "p99_ms": 1250.0,
                    "alloc_bytes_per_req": 10485760,
                    "call_count_per_min": 5000,
                    "environment": "production",
                    "recorded_at": "2026-08-10T06:00:00Z"
                }
            ]
        }"#;
        let payload = parse_payload(json).unwrap();
        assert_eq!(payload.version, "1.0");
        assert_eq!(payload.source, "draco-mcp");
        assert_eq!(payload.workspace_key, "my-app-v1");
        assert_eq!(payload.metrics.len(), 1);
        let m = &payload.metrics[0];
        assert_eq!(m.metric_id, "tel-p99-001");
        assert_eq!(m.line_number, 88);
        assert_eq!(m.p99_ms, 1250.0);
        assert_eq!(m.alloc_bytes_per_req, 10485760);
    }

    #[test]
    fn optional_fields_default() {
        let payload =
            parse_payload(r#"{"version":"1.0","workspace_key":"w","metrics":[{"metric_id":"m1","file_path":"src/a.rs","function_name":"f","line_number":1,"p50_ms":0.0,"p99_ms":0.0,"recorded_at":"2026-08-10T06:00:00Z"}]}"#)
                .unwrap();
        let m = &payload.metrics[0];
        assert_eq!(m.alloc_bytes_per_req, 0);
        assert_eq!(m.call_count_per_min, 0);
        assert_eq!(m.environment, "production");
    }

    #[test]
    fn empty_metrics_allowed() {
        let payload = parse_payload(r#"{"version":"1.0","workspace_key":"w"}"#).unwrap();
        assert!(payload.metrics.is_empty());
    }

    #[test]
    fn missing_workspace_key_fails() {
        assert!(parse_payload(r#"{"version":"1.0"}"#).is_err());
    }

    #[test]
    fn parse_payload_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.json");
        std::fs::write(&path, r#"{"version":"1.0","workspace_key":"w","metrics":[]}"#).unwrap();
        let payload = parse_payload_file(&path).unwrap();
        assert_eq!(payload.workspace_key, "w");
    }

    #[test]
    fn parse_missing_file_errors() {
        let err = parse_payload_file(std::path::Path::new("/nonexistent/x.json")).unwrap_err();
        assert!(matches!(err, IngestError::Io(_)));
    }
}
