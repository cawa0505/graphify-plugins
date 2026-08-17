//! CRG MCP client 骨架 — 對「以 System User 運行的 code-review-graph (CRG)
//! MCP Server」發起 MCP-over-HTTP 呼叫（選項 A：即時 analysis 接入）。
//!
//! Slice 0 只落 framing 骨架（initialize handshake + tools/call），不做真呼叫
//! —— ingest 主路徑是 file-based import（選項 B，零 CRG 介面風險）。
//! Slice 1/2 接 CRG 4 tools 時在此擴充。probe 已驗證 CRG 的 MCP-over-HTTP
//! handshake 可通（`initialize` → session id → `tools/call`）。

use serde_json::{json, Value};
use std::time::Duration;

/// CRG 現役 MCP tools（2026-08 probe 實測，與 LOCAL-NOTES 的
/// `--tools` 參數一致）。
pub const CRG_TOOLS: [&str; 4] = [
    "get_minimal_context_tool",
    "query_graph_tool",
    "get_review_context_tool",
    "detect_changes_tool",
];

/// CRG MCP client（streamable HTTP transport）。
#[derive(Debug, Clone)]
pub struct CrgMcpClient {
    base_url: String,
    /// initialize 拿到的 session id（`Mcp-Session-Id` header）。
    session_id: Option<String>,
}

impl CrgMcpClient {
    /// 建立 client（`base_url` 如 `http://127.0.0.1:8080/mcp`）。
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            session_id: None,
        }
    }

    /// 是否已完成 initialize handshake。
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.session_id.is_some()
    }

    /// 產出 `initialize` 請求 body（framing 測試用；真呼叫見 Slice 1/2）。
    #[must_use]
    pub fn initialize_request() -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "graphify-plugin-review", "version": "0.1.0" }
            }
        })
    }

    /// 執行 MCP `initialize` handshake（streamable HTTP POST）。
    ///
    /// 從回應 header `Mcp-Session-Id` 快取 session id；重複呼叫為
    /// no-op（回傳既有 session）。CRG 不可達 / 無 session header → Err。
    ///
    /// # Errors
    /// 網路/HTTP 失敗回傳 [`ureq::Error`]；回應缺 session id 回傳
    /// [`CrgError::NoSessionId`]。
    pub fn initialize(&mut self) -> Result<String, CrgError> {
        if let Some(session) = &self.session_id {
            return Ok(session.clone());
        }
        let resp = ureq::post(&self.base_url)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream")
            .timeout(Duration::from_secs(10))
            .send_json(Self::initialize_request())?;
        // MCP Streamable HTTP：session id 由 initialize 回應 header 提供。
        let session = resp
            .header("Mcp-Session-Id")
            .ok_or(CrgError::NoSessionId)?
            .to_string();
        self.session_id = Some(session.clone());
        // 讀完 body 讓連線可重用（header 已取，body 不需要）。
        let _ = resp.into_string();
        Ok(session)
    }

    /// 產出 `tools/call` 請求 body（framing 測試用）。
    #[must_use]
    pub fn call_tool_request(id: u64, name: &str, args: &Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": args }
        })
    }

    /// 從 MCP 回應 body 取出 `result.content`（text block 串接）。
    ///
    /// 回應可為純 JSON（`{"result":{"content":[...]}}`）或 SSE
    /// （`event: message\ndata: {...}`）——先剝 SSE 前綴再取 JSON。
    #[must_use]
    pub fn extract_result_content(raw: &str) -> Option<String> {
        let json_str = raw
            .lines()
            .find_map(|l| l.strip_prefix("data: "))
            .unwrap_or(raw);
        let v: Value = serde_json::from_str(json_str).ok()?;
        let content = v.get("result")?.get("content")?.as_array()?;
        let text: Vec<&str> = content
            .iter()
            .filter_map(|c| c.get("text").and_then(Value::as_str))
            .collect();
        if text.is_empty() {
            None
        } else {
            Some(text.join("\n"))
        }
    }

    /// 執行 `tools/call`（streamable HTTP POST）。回傳合併後的 text 內容。
    ///
    /// # Errors
    /// 網路/HTTP 失敗回傳 [`ureq::Error`]；`session_id` 未初始化回傳
    /// [`CrgError::NotInitialized`]。
    pub fn call_tool(
        &mut self,
        name: &str,
        args: &Value,
    ) -> Result<String, CrgError> {
        let session = self
            .session_id
            .clone()
            .ok_or(CrgError::NotInitialized)?;
        let body = Self::call_tool_request(2, name, args);
        let resp = ureq::post(&self.base_url)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream")
            .set("Mcp-Session-Id", &session)
            .timeout(Duration::from_secs(10))
            .send_json(body)?;
        let raw = resp.into_string()?;
        Self::extract_result_content(&raw).ok_or(CrgError::EmptyResult)
    }

    /// 呼叫 `detect_changes_tool`（git diff 風險審查），解析
    /// `review_priorities`（top-10 風險節點）。
    ///
    /// 依契約（crg-requirements.md §4）：`repo_root` 為必帶參數，
    /// `detail_level` 用 `"standard"`（minimal 只回 name strings，
    /// 無法取得 file_path/line 對映）。
    ///
    /// # Errors
    /// handshake / 網路 / parse 失敗依情況回傳 [`CrgError`]。
    pub fn detect_changes(
        &mut self,
        repo_root: &str,
        base: Option<&str>,
    ) -> Result<Vec<CrgPriority>, CrgError> {
        self.initialize()?;
        let mut args = json!({ "repo_root": repo_root, "detail_level": "standard" });
        if let Some(b) = base {
            args["base"] = json!(b);
        }
        let text = self.call_tool("detect_changes_tool", &args)?;
        // text content 是 JSON 字串（實測：structuredContent 與 text 同源）。
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| CrgError::Parse(e.to_string()))?;
        let priorities = v
            .get("review_priorities")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|p| serde_json::from_value::<CrgPriority>(p.clone()).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(priorities)
    }
}

/// `detect_changes_tool` 的 `review_priorities` 單筆（依 CRG 源碼
/// `node_to_dict` shape；只取 plugin 需要的欄位，忽略其餘）。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CrgPriority {
    pub name: Option<String>,
    pub qualified_name: Option<String>,
    pub file_path: Option<String>,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub risk_score: Option<f64>,
    pub kind: Option<String>,
}

/// CRG client 錯誤。
#[derive(Debug, thiserror::Error)]
pub enum CrgError {
    #[error("ureq error: {0}")]
    Ureq(Box<ureq::Error>),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not initialized: call initialize() first")]
    NotInitialized,
    #[error("no Mcp-Session-Id in initialize response")]
    NoSessionId,
    #[error("empty result from CRG")]
    EmptyResult,
    #[error("parse error: {0}")]
    Parse(String),
}

impl From<ureq::Error> for CrgError {
    fn from(e: ureq::Error) -> Self {
        Self::Ureq(Box::new(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_request_has_expected_shape() {
        let v = CrgMcpClient::initialize_request();
        assert_eq!(v["method"], "initialize");
        assert_eq!(v["params"]["protocolVersion"], "2025-03-26");
    }

    #[test]
    fn call_tool_request_has_expected_shape() {
        let v = CrgMcpClient::call_tool_request(
            2,
            "query_graph_tool",
            &json!({"pattern": "callers_of", "target": "crate::auth::verify"}),
        );
        assert_eq!(v["method"], "tools/call");
        assert_eq!(v["params"]["name"], "query_graph_tool");
        assert_eq!(v["params"]["arguments"]["pattern"], "callers_of");
    }

    #[test]
    fn extracts_text_from_plain_json_response() {
        let raw = r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"hello"},{"type":"text","text":"world"}]}}"#;
        assert_eq!(
            CrgMcpClient::extract_result_content(raw).as_deref(),
            Some("hello\nworld")
        );
    }

    #[test]
    fn extracts_text_from_sse_response() {
        let raw = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"sse hit\"}]}}\n\n";
        assert_eq!(
            CrgMcpClient::extract_result_content(raw).as_deref(),
            Some("sse hit")
        );
    }

    #[test]
    fn empty_result_returns_none() {
        let raw = r#"{"jsonrpc":"2.0","id":2,"result":{"content":[]}}"#;
        assert!(CrgMcpClient::extract_result_content(raw).is_none());
    }

    #[test]
    fn not_initialized_call_fails() {
        let mut c = CrgMcpClient::new("http://127.0.0.1:1/mcp");
        let err = c.call_tool("query_graph_tool", &json!({})).unwrap_err();
        assert!(matches!(err, CrgError::NotInitialized));
    }

    #[test]
    fn tool_list_matches_crg() {
        assert_eq!(CRG_TOOLS.len(), 4);
        assert!(CRG_TOOLS.contains(&"detect_changes_tool"));
    }

    #[test]
    fn initialize_is_noop_when_already_initialized() {
        let mut c = CrgMcpClient::new("http://127.0.0.1:1/mcp");
        c.session_id = Some("ses-test".to_string());
        // 已初始化 → 不回網路，直接回傳快取 session。
        assert_eq!(c.initialize().unwrap(), "ses-test");
        assert_eq!(c.session_id.as_deref(), Some("ses-test"));
    }

    #[test]
    fn initialize_unreachable_server_fails() {
        let mut c = CrgMcpClient::new("http://127.0.0.1:1/mcp");
        // 127.0.0.1:1 幾乎必然拒連 — 回 Ureq，不 panic。
        let err = c.initialize().unwrap_err();
        assert!(matches!(err, CrgError::Ureq(_)));
        assert!(c.session_id.is_none());
    }
}
