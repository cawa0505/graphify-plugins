//! Line-to-Symbol Resolver — 將 `file_path + line_number` 升維綁定到 Graphify
//! canonical node id。
//!
//! Graphify extract 產出的 `NodeId` 為 `{file_path}:{kind}:{name}`（如
//! `src/auth.rs:function:verify_token`）——人可讀且跨 re-parse 穩定（只要
//! symbol 存在，id 不變；行號位移不影響）。綁定以 id 為鍵，滿足 R1
//! 「canonical_node_id（穩定 Symbol 外鍵）」。
//!
//! 解析策略：在 GraphOutput 節點中找 `source_file` 相符且
//! `start_line <= line <= end_line` 的節點；多個重疊時取 span 最小者
//! （最內層 symbol，如 function 而非其所在的 class/module）。

use graphify_core::types::{GraphOutput, NodeId};

/// 一次解析結果：命中節點 + 其 canonical id。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub node_id: NodeId,
}

/// 對 `graph` 做線掃，將 `(file_path, line)` 解析為最內層節點的 canonical id。
///
/// `file_path` 以 workspace-root 相對路徑表達（與 IngestPayload 一致）；
/// 比對時先試精確比對，再試「以 / 分隔路徑的 suffix 相符」以容忍前綴差異。
/// 回傳 `None` = 檔案不存在於 graph 或行號超出所有節點（orphan line）。
#[must_use]
pub fn resolve_line(
    graph: &GraphOutput,
    file_path: &str,
    line: u32,
) -> Option<Resolved> {
    let line = usize::try_from(line).unwrap_or(usize::MAX);
    let mut best: Option<&graphify_core::types::Node> = None;
    let mut best_span = usize::MAX;

    for node in &graph.nodes {
        if !file_matches(&node.source_file, file_path) {
            continue;
        }
        if node.start_line <= line && line <= node.end_line {
            let span = node.end_line - node.start_line;
            if span < best_span {
                best_span = span;
                best = Some(node);
            }
        }
    }

    best.map(|n| Resolved {
        node_id: n.id.clone(),
    })
}

/// `node_path` 是否代表 `want`（workspace-root 相對路徑）。
/// 精確相等，或兩者都以 `/` 分隔且 node_path 以 `want` 結尾，
/// 或 want 以 node_path 結尾（反向匹配，處理 ingest payload 給絕對路徑而 graph 給相對路徑的情況）。
#[must_use]
pub fn file_matches(node_path: &str, want: &str) -> bool {
    if node_path == want {
        return true;
    }
    if want.is_empty() {
        return false;
    }

    // 正向：node_path 以 want 結尾（graph 路徑長，payload 路徑短）
    if want.contains('/') {
        let n = node_path.strip_suffix(want);
        if let Some(prefix) = n {
            // 前綴必須是空字串或結尾為 '/'，避免 "foosrc/auth.rs" 誤配
            return prefix.is_empty() || prefix.ends_with('/');
        }
    }

    // 反向：want 以 node_path 結尾（payload 路徑長，graph 路徑短）
    // 去掉 graph 路徑的 `./` 前綴再比對
    let clean_path = node_path.strip_prefix("./").unwrap_or(node_path);
    if clean_path.contains('/') {
        let n = want.strip_suffix(clean_path);
        if let Some(prefix) = n {
            return prefix.is_empty() || prefix.ends_with('/');
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphify_core::types::{FileType, Node};

    fn node(
        id: &str,
        source_file: &str,
        start: usize,
        end: usize,
    ) -> Node {
        Node {
            id: NodeId(id.to_string()),
            label: id.rsplit(':').next().unwrap_or(id).to_string(),
            file_type: FileType::Code,
            kind: "function".to_string(),
            language: "rust".to_string(),
            source_file: source_file.to_string(),
            start_line: start,
            end_line: end,
            doc_comment: None,
            description: None,
            metadata: None,
        }
    }

    fn graph_with(nodes: Vec<Node>) -> GraphOutput {
        GraphOutput {
            nodes,
            edges: Vec::new(),
            metadata: Default::default(),
        }
    }

    #[test]
    fn resolves_innermost_symbol() {
        let g = graph_with(vec![
            node("src/auth.rs:module", "src/auth.rs", 1, 100),
            node("src/auth.rs:class:Auth", "src/auth.rs", 10, 90),
            node("src/auth.rs:function:verify_token", "src/auth.rs", 30, 60),
        ]);
        let r = resolve_line(&g, "src/auth.rs", 42).unwrap();
        assert_eq!(
            r.node_id.0,
            "src/auth.rs:function:verify_token",
            "innermost (smallest span) wins"
        );
    }

    #[test]
    fn resolves_outer_when_no_inner() {
        let g = graph_with(vec![node("src/auth.rs:module", "src/auth.rs", 1, 100)]);
        let r = resolve_line(&g, "src/auth.rs", 99).unwrap();
        assert_eq!(r.node_id.0, "src/auth.rs:module");
    }

    #[test]
    fn boundary_lines_inclusive() {
        let g = graph_with(vec![node("src/a.rs:function:f", "src/a.rs", 10, 20)]);
        assert!(resolve_line(&g, "src/a.rs", 10).is_some());
        assert!(resolve_line(&g, "src/a.rs", 20).is_some());
        assert!(resolve_line(&g, "src/a.rs", 21).is_none());
    }

    #[test]
    fn orphan_line_returns_none() {
        let g = graph_with(vec![node("src/a.rs:function:f", "src/a.rs", 10, 20)]);
        assert!(resolve_line(&g, "src/a.rs", 5).is_none());
        assert!(resolve_line(&g, "src/b.rs", 15).is_none());
    }

    #[test]
    fn suffix_path_matches() {
        let g = graph_with(vec![node("f:function:f", "/repo/src/a.rs", 1, 5)]);
        // 精確
        assert!(resolve_line(&g, "/repo/src/a.rs", 3).is_some());
        // suffix 相符（容忍前綴差異）
        assert!(resolve_line(&g, "src/a.rs", 3).is_some());
        // 不含 '/' 的 want（純檔名）不做 suffix 匹配
        assert!(resolve_line(&g, "a.rs", 3).is_none());
    }

    #[test]
    fn file_matches_rules() {
        assert!(file_matches("src/auth.rs", "src/auth.rs"));
        assert!(file_matches("/repo/src/auth.rs", "src/auth.rs"));
        assert!(file_matches("src/auth/verify.rs", "auth/verify.rs")); // suffix w/ '/' prefix
        assert!(!file_matches("src/auth.rs", "auth.rs")); // 純檔名不做 suffix
        assert!(!file_matches("src/foosrc/auth.rs", "src/auth.rs"));
        assert!(file_matches("src/foosrc/auth.rs", "src/foosrc/auth.rs"));
    }

    #[test]
    fn file_matches_bidirectional() {
        // 正向匹配（graph 路徑長，payload 路徑短）
        assert!(file_matches("./src/auth.rs", "src/auth.rs")); // ./ 前綴被去除
        assert!(file_matches("src/auth.rs", "src/auth.rs"));
        assert!(file_matches("/repo/src/auth.rs", "src/auth.rs"));

        // 反向匹配（payload 路徑長，graph 路徑短）
        assert!(file_matches("src/auth.rs", "/repo/src/auth.rs")); // 絕對路徑匹配相對路徑
        assert!(file_matches("./src/auth.rs", "/repo/src/auth.rs")); // 反向匹配，./ 被去除

        // 負向匹配
        assert!(!file_matches("./src/other.rs", "/repo/src/auth.rs"));
        assert!(!file_matches("src/auth.rs", "auth.rs")); // 純檔名不做 suffix
        assert!(!file_matches("src/foosrc/auth.rs", "src/auth.rs"));
    }
}
