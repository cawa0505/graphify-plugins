//! Draco MCP client — 對「以 System User 運行的可觀測性 Draco MCP
//! Server」（Prometheus / OTEL / Jaeger / pprof scraper）發起 MCP-over-HTTP
//! 呼叫。
//!
//! Slice 0 只落 framing 骨架（initialize handshake + tools/call）；Slice 1
//! 實作 `fetch_top_hotspots`（一鍵同步 Top 熱點）。
//!
//! Draco 的 MCP-over-HTTP handshake 與 CRG 同構（`initialize` → session id →
//! `tools/call`），framing 沿用 code-review-graph client 的實測格式。
//!
//! **契約 v1（本 plugin 定義）**：public 不存在 observability Draco
//! （唯一 Draco 為網頁 scraper，見 2026-08-10 librarian 調查）——
//! `fetch_top_hotspots` 回應採 server-side 聚合（traceloop envelope +
//! pprof top-N 語意），形狀如下，註記於 openspec spec.md：
//!
//! ```json
//! {
//!   "window": { "start": "2026-08-10T00:00:00Z", "end": "2026-08-10T01:00:00Z" },
//!   "count": 2,
//!   "hotspots": [
//!     { "file_path": "src/db/query.rs", "function_name": "query_users",
//!       "p99_latency_ms": 1250.0, "call_count": 5000, "alloc_bytes": 10485760,
//!       "environment": "production" }
//!   ]
//! }
//! ```

use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

/// Draco 可觀測性 MCP tools — §5.2 確認的 `fetch_top_hotspots`；
/// 其餘工具待 Draco 端實際存在後 probe 補齊（不臆測）。
pub const DRACO_TOOLS: [&str; 1] = ["fetch_top_hotspots"];

/// 預設 Draco base url（`DRACO_BASE_URL` 環境變數覆寫）。
///
/// 注意：port 未經 probe 確認（CRG 的 9877 不保證適用）；待真正 Draco
/// MCP server 部署後修正。
#[must_use]
pub fn default_draco_url() -> String {
    std::env::var("DRACO_BASE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:9876/mcp".to_string())
}

/// Draco `fetch_top_hotspots` 單一熱點（契約 v1）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DracoHotspot {
    pub file_path: String,
    pub function_name: String,
    pub p99_latency_ms: f64,
    #[serde(default)]
    pub call_count: u64,
    #[serde(default)]
    pub alloc_bytes: i64,
    #[serde(default = "default_environment")]
    pub environment: String,
}

fn default_environment() -> String {
    "production".to_string()
}

/// `fetch_top_hotspots` 回應 envelope（`{window, count, hotspots}`）。
#[derive(Debug, Clone, Deserialize)]
pub struct DracoHotspotsResponse {
    #[serde(default)]
    pub window: Option<Value>,
    #[serde(default)]
    pub count: Option<usize>,
    #[serde(default)]
    pub hotspots: Vec<DracoHotspot>,
}

/// Draco MCP client（streamable HTTP transport）。
#[derive(Debug, Clone)]
pub struct DracoMcpClient {
    base_url: String,
    /// initialize 拿到的 session id（`Mcp-Session-Id` header）。
    session_id: Option<String>,
}

impl DracoMcpClient {
    /// 建立 client（`base_url` 如 `http://127.0.0.1:9876/mcp`）。
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
                "clientInfo": { "name": "graphify-plugin-telemetry", "version": "0.1.0" }
            }
        })
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

    /// 執行 `initialize` handshake，記下 `Mcp-Session-Id` header。
    ///
    /// # Errors
    /// 網路/HTTP 失敗回傳 [`ureq::Error`]（轉為 [`DracoError::Ureq`]）。
    pub fn initialize(&mut self) -> Result<(), DracoError> {
        let resp = ureq::post(&self.base_url)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream")
            .timeout(Duration::from_secs(10))
            .send_json(Self::initialize_request())?;
        self.session_id = resp.header("Mcp-Session-Id").map(ToString::to_string);
        Ok(())
    }

    /// 一鍵拉取 Draco 的 Top 熱點（server-side 聚合；`limit` 為 `None`
    /// 時交 Draco 預設，通常 Top 10）。
    ///
    /// 自動完成 `initialize` handshake（若尚未初始化）。回傳熱點清單，
    /// 依 server 排序（p99 降冪）。
    ///
    /// # Errors
    /// 網路 / MCP framing / 契約解析失敗回傳 [`DracoError`]。
    pub fn fetch_top_hotspots(
        &mut self,
        limit: Option<usize>,
    ) -> Result<Vec<DracoHotspot>, DracoError> {
        if self.session_id.is_none() {
            self.initialize()?;
        }
        let args = match limit {
            Some(n) => json!({ "limit": n }),
            None => json!({}),
        };
        let raw = self.call_tool("fetch_top_hotspots", &args)?;
        Self::parse_hotspots(&raw)
    }

    /// 從 Draco `tools/call` 回傳的 text（已是契約 JSON，`call_tool`
    /// 已剝掉 MCP envelope）解析 `fetch_top_hotspots` 回應。
    ///
    /// # Errors
    /// text 非契約 JSON 回傳 [`DracoError::Parse`]。
    pub fn parse_hotspots(raw: &str) -> Result<Vec<DracoHotspot>, DracoError> {
        let resp: DracoHotspotsResponse =
            serde_json::from_str(raw).map_err(|e| DracoError::Parse(e.to_string()))?;
        Ok(resp.hotspots)
    }

    /// 執行 `tools/call`（streamable HTTP POST）。回傳合併後的 text 內容。
    ///
    /// # Errors
    /// 網路/HTTP 失敗回傳 [`ureq::Error`]；`session_id` 未初始化回傳
    /// [`DracoError::NotInitialized`]。
    pub fn call_tool(&mut self, name: &str, args: &Value) -> Result<String, DracoError> {
        let session = self
            .session_id
            .clone()
            .ok_or(DracoError::NotInitialized)?;
        let body = Self::call_tool_request(2, name, args);
        let resp = ureq::post(&self.base_url)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream")
            .set("Mcp-Session-Id", &session)
            .timeout(Duration::from_secs(10))
            .send_json(body)?;
        let raw = resp.into_string()?;
        Self::extract_result_content(&raw).ok_or(DracoError::EmptyResult)
    }
}

/// Draco client 錯誤。
#[derive(Debug, thiserror::Error)]
pub enum DracoError {
    #[error("ureq error: {0}")]
    Ureq(Box<ureq::Error>),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not initialized: call initialize() first")]
    NotInitialized,
    #[error("empty result from Draco")]
    EmptyResult,
    #[error("parse error: {0}")]
    Parse(String),
}

impl From<ureq::Error> for DracoError {
    fn from(e: ureq::Error) -> Self {
        Self::Ureq(Box::new(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_request_has_expected_shape() {
        let v = DracoMcpClient::initialize_request();
        assert_eq!(v["method"], "initialize");
        assert_eq!(v["params"]["protocolVersion"], "2025-03-26");
        assert_eq!(v["params"]["clientInfo"]["name"], "graphify-plugin-telemetry");
    }

    #[test]
    fn call_tool_request_has_expected_shape() {
        let v = DracoMcpClient::call_tool_request(
            2,
            "fetch_top_hotspots",
            &json!({"workspace_key": "my-app-v1"}),
        );
        assert_eq!(v["method"], "tools/call");
        assert_eq!(v["params"]["name"], "fetch_top_hotspots");
        assert_eq!(v["params"]["arguments"]["workspace_key"], "my-app-v1");
    }

    #[test]
    fn extracts_text_from_plain_json_response() {
        let raw = r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"hello"},{"type":"text","text":"world"}]}}"#;
        assert_eq!(
            DracoMcpClient::extract_result_content(raw).as_deref(),
            Some("hello\nworld")
        );
    }

    #[test]
    fn extracts_text_from_sse_response() {
        let raw = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"sse hit\"}]}}\n\n";
        assert_eq!(
            DracoMcpClient::extract_result_content(raw).as_deref(),
            Some("sse hit")
        );
    }

    #[test]
    fn empty_result_returns_none() {
        let raw = r#"{"jsonrpc":"2.0","id":2,"result":{"content":[]}}"#;
        assert!(DracoMcpClient::extract_result_content(raw).is_none());
    }

    #[test]
    fn not_initialized_call_fails() {
        let mut c = DracoMcpClient::new("http://127.0.0.1:1/mcp");
        let err = c.call_tool("fetch_top_hotspots", &json!({})).unwrap_err();
        assert!(matches!(err, DracoError::NotInitialized));
    }

    #[test]
    fn default_url_respects_env() {
        assert!(default_draco_url().starts_with("http://"));
    }

    #[test]
    fn parses_hotspots_contract() {
        let raw = r#"{
            "window": { "start": "2026-08-10T00:00:00Z", "end": "2026-08-10T01:00:00Z" },
            "count": 2,
            "hotspots": [
                { "file_path": "src/db/query.rs", "function_name": "query_users",
                  "p99_latency_ms": 1250.0, "call_count": 5000, "alloc_bytes": 10485760 },
                { "file_path": "src/payment/stripe.rs", "function_name": "process_payment",
                  "p99_latency_ms": 2450.0, "environment": "staging" }
            ]
        }"#;
        let resp: DracoHotspotsResponse =
            serde_json::from_str(raw).expect("contract JSON must parse");
        assert_eq!(resp.count, Some(2));
        assert_eq!(resp.hotspots.len(), 2);
        // environment 缺省 = production
        assert_eq!(resp.hotspots[0].environment, "production");
        assert_eq!(resp.hotspots[1].environment, "staging");
        // 缺省數值為 0（call_count / alloc_bytes 可選）
        assert_eq!(resp.hotspots[1].call_count, 0);
    }

    #[test]
    fn parse_hotspots_tolerates_missing_hotspots() {
        // 契約 lenient：缺 hotspots 欄位 = 無熱點，非錯誤。
        let raw = r#"{"count": 1}"#;
        let hotspots = DracoMcpClient::parse_hotspots(raw).unwrap();
        assert!(hotspots.is_empty());
    }

    /// 用臨時 TcpListener 起一個迷你 Draco（initialize + tools/call 兩段
    /// handshake），驗證 `fetch_top_hotspots` 完整 HTTP 旅程。這是真連線
    /// 測試（非 mock 實作），只是 Draco 端用 fixture 回應。
    #[test]
    fn fetch_top_hotspots_e2e() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            // 兩段 handshake：initialize → tools/call
            let hotspot_json = r#"{"window":{"start":"2026-08-10T00:00:00Z","end":"2026-08-10T01:00:00Z"},"count":1,"hotspots":[{"file_path":"src/db/query.rs","function_name":"query_users","p99_latency_ms":1250.0,"call_count":5000,"alloc_bytes":10485760}]}"#;
            for (i, stream) in listener.incoming().enumerate() {
                let mut stream = stream.unwrap();
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).unwrap();
                let body = if i == 0 {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake-draco","version":"0.0.0"}}}"#
                } else {
                    &format!(
                        r#"{{"jsonrpc":"2.0","id":2,"result":{{"content":[{{"type":"text","text":{}}}]}}}}"#,
                        serde_json::to_string(hotspot_json).unwrap()
                    )
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: ses-e2e\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(resp.as_bytes()).unwrap();
            }
        });

        let mut client =
            DracoMcpClient::new(format!("http://{addr}/mcp"));
        let hotspots = client.fetch_top_hotspots(Some(10)).unwrap();
        assert_eq!(hotspots.len(), 1);
        assert_eq!(hotspots[0].function_name, "query_users");
        assert_eq!(hotspots[0].p99_latency_ms, 1250.0);
        assert_eq!(hotspots[0].call_count, 5000);
        assert_eq!(hotspots[0].environment, "production");
    }
}
