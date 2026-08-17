//! sync_toon 封包（.toon）— 依 `openspec/specs/sync-toon-packet/spec.md`。
//!
//! - metadata MUST：`format_version`（`"1.0.0"`）＋ `workspace_key`。
//! - relay 狀態放 `metadata.plugin_data["handoff"]`（core 契約保留容器）。
//! - 錯誤以 `metadata.error` 表達，實作不得 panic。
//! - 版本政策：同 MAJOR 可互操作；MAJOR 不符以 `error` 封包拒絕。
//! - 轉義規則與 core `toon.rs` 一致（同系列格式）。

/// 封包契約版本。
pub const FORMAT_VERSION: &str = "1.0.0";

/// 解析出的封包 metadata。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PacketMeta {
    pub format_version: Option<String>,
    pub workspace_key: Option<String>,
    pub error: Option<String>,
}

/// TOON 字串轉義（與 core `toon.rs::escape_string` 同規則，非 tabular 模式）。
fn escape_string(s: &str) -> String {
    let needs_quoting = s.is_empty()
        || s == "null"
        || s == "true"
        || s == "false"
        || s.chars().any(|c| {
            c.is_whitespace()
                || c == ':'
                || c == '['
                || c == ']'
                || c == '{'
                || c == '}'
                || c == '-'
                || c == '\\'
                || c == '"'
        })
        || s.starts_with('-')
        || s.chars().next().is_some_and(|c| c.is_ascii_digit());

    if needs_quoting {
        let mut escaped = String::from("\"");
        for c in s.chars() {
            match c {
                '"' => escaped.push_str("\\\""),
                '\\' => escaped.push_str("\\\\"),
                '\n' => escaped.push_str("\\n"),
                '\r' => escaped.push_str("\\r"),
                '\t' => escaped.push_str("\\t"),
                _ => escaped.push(c),
            }
        }
        escaped.push('"');
        escaped
    } else {
        s.to_string()
    }
}

/// TOON 字串反轉義。
fn unescape_string(s: &str) -> String {
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let mut unescaped = String::new();
        let chars: Vec<char> = s[1..s.len() - 1].chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '\\' && i + 1 < chars.len() {
                match chars[i + 1] {
                    '"' => unescaped.push('"'),
                    '\\' => unescaped.push('\\'),
                    'n' => unescaped.push('\n'),
                    'r' => unescaped.push('\r'),
                    't' => unescaped.push('\t'),
                    c => {
                        unescaped.push('\\');
                        unescaped.push(c);
                    }
                }
                i += 2;
            } else {
                unescaped.push(chars[i]);
                i += 1;
            }
        }
        unescaped
    } else {
        s.to_string()
    }
}

/// 產出承載封包：metadata（format_version + workspace_key）+ plugin_data。
pub fn emit_packet(workspace_key: &str, plugin_data: &serde_json::Value) -> String {
    let mut out = String::new();
    out.push_str("metadata:\n");
    out.push_str(&format!("  format_version: {}\n", escape_string(FORMAT_VERSION)));
    out.push_str(&format!("  workspace_key: {}\n", escape_string(workspace_key)));
    out.push_str(&format!("  plugin_data: {}\n", escape_string(&plugin_data.to_string())));
    out
}

/// 產出錯誤封包：metadata（format_version + error）。
pub fn emit_error_packet(error: &str) -> String {
    let mut out = String::new();
    out.push_str("metadata:\n");
    out.push_str(&format!("  format_version: {}\n", escape_string(FORMAT_VERSION)));
    out.push_str(&format!("  error: {}\n", escape_string(error)));
    out
}

/// 掃描 .toon 的 metadata 區塊，取 format_version / workspace_key / error。
pub fn parse_meta(toon: &str) -> PacketMeta {
    let mut meta = PacketMeta::default();
    let mut in_metadata = false;
    for line in toon.lines() {
        let trimmed = line.trim();
        if in_metadata {
            if trimmed.is_empty() || !line.starts_with("  ") {
                break;
            }
            if let Some((k, v)) = trimmed.split_once(':') {
                let v = v.trim();
                match k.trim() {
                    "format_version" => meta.format_version = Some(unescape_string(v)),
                    "workspace_key" => meta.workspace_key = Some(unescape_string(v)),
                    "error" => meta.error = Some(unescape_string(v)),
                    _ => {}
                }
            }
        } else if trimmed == "metadata:" {
            in_metadata = true;
        }
    }
    meta
}

/// MAJOR 版本是否與本契約（v1）不符。
pub fn major_mismatch(format_version: &str) -> bool {
    format_version.split('.').next() != Some("1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_and_parse_packet_roundtrip() {
        let data = serde_json::json!({"handoff": {"schema_version": "1.0.0"}, "active_nodes": []});
        let packet = emit_packet("w-abc123", &data);
        let meta = parse_meta(&packet);
        assert_eq!(meta.format_version.as_deref(), Some("1.0.0"));
        assert_eq!(meta.workspace_key.as_deref(), Some("w-abc123"));
        assert!(meta.error.is_none());
    }

    #[test]
    fn error_packet_carries_error() {
        let packet = emit_error_packet("No relay.json found. Run relayInit first.");
        let meta = parse_meta(&packet);
        assert_eq!(
            meta.error.as_deref(),
            Some("No relay.json found. Run relayInit first.")
        );
        assert!(meta.workspace_key.is_none());
    }

    #[test]
    fn major_mismatch_detected() {
        assert!(!major_mismatch("1.0.0"));
        assert!(!major_mismatch("1.5.0"));
        assert!(major_mismatch("2.0.0"));
        assert!(major_mismatch("0.9.0"));
    }

    #[test]
    fn escape_roundtrip_with_hostile_strings() {
        let samples = [
            "plain",
            "with space",
            "comma, and quote \"q\"",
            "colon: bracket[ brace}",
            "-leading-hyphen",
            "123-starts-numeric",
            "多語系\u{1F680}emojis",
            "newline\nand\ttab",
        ];
        for s in samples {
            assert_eq!(unescape_string(&escape_string(s)), s);
        }
    }

    #[test]
    fn parse_meta_tolerates_missing_keys() {
        let meta = parse_meta("metadata:\n  plugin_data: {}\n");
        assert!(meta.format_version.is_none());
        assert!(meta.workspace_key.is_none());
    }

    #[test]
    fn parse_meta_ignores_other_sections() {
        let packet = "metadata:\n  format_version: \"1.0.0\"\n  workspace_key: \"wk\"\n\nnodes[0,]{...}\n";
        let meta = parse_meta(packet);
        assert_eq!(meta.format_version.as_deref(), Some("1.0.0"));
    }
}
