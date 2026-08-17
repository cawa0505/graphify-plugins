//! sync_toon 封包（.toon）— 依 core 契約（format_version 1.0.0 + workspace_key）。
//!
//! - plugin 狀態放 `metadata.plugin_data["review"]`（core 契約保留容器）。
//! - 錯誤以 `metadata.error` 表達，實作不得 panic。
//! - 版本政策：同 MAJOR 可互操作；MAJOR 不符以 `error` 封包拒絕。

/// 封包契約版本。
pub const FORMAT_VERSION: &str = "1.0.0";

/// 解析出的封包 metadata。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PacketMeta {
    pub format_version: Option<String>,
    pub workspace_key: Option<String>,
    pub error: Option<String>,
}

/// TOON 字串轉義（與 core `toon.rs::escape_string` 同規則）。
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

/// 目前時間的 RFC3339 字串（UTC；供 updated_at 使用）。
#[must_use]
pub fn now_rfc3339() -> String {
    // 不引入 chrono：以 Unix epoch 秒組 UTC 字串（YYYY-MM-DDTHH:MM:SSZ）。
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // 1970-01-01 為週四；算出日期
    let (y, mo, d) = civil_from_days(i64::try_from(days).unwrap_or(0));
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// 自 days（自 1970-01-01 起算）轉民用日期（Howard Hinnant 演算法）。
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
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
    fn emit_and_parse_roundtrip() {
        let data = serde_json::json!({"review": {"bindings": 7}});
        let packet = emit_packet("w-abc", &data);
        let meta = parse_meta(&packet);
        assert_eq!(meta.format_version.as_deref(), Some("1.0.0"));
        assert_eq!(meta.workspace_key.as_deref(), Some("w-abc"));
        assert!(meta.error.is_none());
    }

    #[test]
    fn error_packet_carries_error() {
        let packet = emit_error_packet("No bindings yet.");
        let meta = parse_meta(&packet);
        assert_eq!(meta.error.as_deref(), Some("No bindings yet."));
    }

    #[test]
    fn major_mismatch_detected() {
        assert!(!major_mismatch("1.0.0"));
        assert!(!major_mismatch("1.5.0"));
        assert!(major_mismatch("2.0.0"));
        assert!(major_mismatch("0.9.0"));
    }

    #[test]
    fn escape_roundtrip() {
        let samples = ["plain", "with space", ".quote\"q\".", "colon: bracket[", "多語系\u{1F680}"];
        for s in samples {
            assert_eq!(unescape_string(&escape_string(s)), s);
        }
    }
}
