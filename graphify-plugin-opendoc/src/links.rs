//! Hard link extraction — 從 Markdown spec blocks 產出 [`LinkRow`] 集。
//!
//! `index_docs` 遍歷 doc 目錄下的 Markdown 文件，萃取 spec blocks
//! 與其 declared symbols，組成 registry 的一筆筆連結列。

use std::path::Path;

use crate::spec::{self, SpecBlock};

/// 一筆硬鏈結（registry 列）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRow {
    pub workspace_key: String,
    pub doc_path: String,
    pub spec_id: String,
    pub symbol: String,
    pub signature: String,
}

/// 從 `root` 下的指定 doc paths 萃取硬鏈結列表。
///
/// 只處理 `.md` 文件，忽略不存在的路徑。`doc_path` 以 root 相對路徑表示
/// （POSIX `/` 分隔）。每個 block 的每個 symbol 產出一筆 [`LinkRow`]；
/// 無 symbol 的 block 不產生列。
///
/// # 性價比
/// I/O 一次讀檔；每個 block 的 hash 為 O(file_size)。對數百 doc 的
/// 專案仍遠快於向量檢索。
pub fn index_docs(
    root: &Path,
    doc_paths: &[String],
    workspace_key: &str,
) -> Vec<LinkRow> {
    let mut rows = Vec::new();
    for rel in doc_paths {
        let full = root.join(rel);
        let Ok(md) = std::fs::read_to_string(&full) else {
            continue;
        };
        let blocks = spec::extract_blocks(&md, rel);
        for block in blocks {
            push_rows(&mut rows, workspace_key, &block);
        }
    }
    rows
}

fn push_rows(rows: &mut Vec<LinkRow>, workspace_key: &str, block: &SpecBlock) {
    for symbol in &block.symbols {
        rows.push(LinkRow {
            workspace_key: workspace_key.to_string(),
            doc_path: block.doc_path.clone(),
            spec_id: block.spec_id.clone(),
            symbol: symbol.clone(),
            signature: block.block_signature.clone(),
        });
    }
}

/// 列舉 `root` 目錄下所有 `.md` / `.markdown` 文件（相對路徑字串）。
///
/// 不包含 hidden（`.` 開頭）目錄或檔案。
pub fn discover_doc_paths(root: &Path) -> Vec<String> {
    let mut results = Vec::new();
    walk(root, root, &mut results);
    results.sort();
    results
}

fn walk(root: &Path, dir: &Path, results: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            dirs.push(path);
        } else if is_markdown(&name_str) {
            if let Ok(rel) = path.strip_prefix(root) {
                results.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    for d in dirs {
        walk(root, &d, results);
    }
}

fn is_markdown(name: &str) -> bool {
    name.ends_with(".md") || name.ends_with(".markdown")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn index_yields_links_only_for_blocks_with_symbols() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("auth.md"),
            "## verify_token\n\nValidates.\n\n# Symbol: crate::auth::verify_token\n",
        )
        .unwrap();
        let rows = index_docs(dir.path(), &["auth.md".to_string()], "ws-1");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].workspace_key, "ws-1");
        assert_eq!(rows[0].doc_path, "auth.md");
        assert_eq!(rows[0].symbol, "crate::auth::verify_token");
        assert!(rows[0].signature.len() == 40);
    }

    #[test]
    fn block_without_symbol_produces_no_row() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("doc.md"), "## intro\n\nNo symbols.\n").unwrap();
        let rows = index_docs(dir.path(), &["doc.md".to_string()], "ws-1");
        assert!(rows.is_empty());
    }

    #[test]
    fn nonexistent_file_skipped() {
        let dir = tempdir().unwrap();
        let rows = index_docs(dir.path(), &["missing.md".to_string()], "ws-1");
        assert!(rows.is_empty());
    }

    #[test]
    fn discover_finds_markdown_recursively() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("root.md"), "# R\n").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub").join("a.md"), "# A\n").unwrap();
        fs::write(dir.path().join("sub").join("b.md"), "# B\n").unwrap();
        fs::write(dir.path().join("notmd.txt"), "no\n").unwrap();

        let paths = discover_doc_paths(dir.path());
        assert!(paths.contains(&"root.md".to_string()));
        assert!(paths.contains(&"sub/a.md".to_string()));
        assert!(paths.contains(&"sub/b.md".to_string()));
        assert!(!paths.iter().any(|p| p.ends_with(".txt")));
    }

    #[test]
    fn discover_ignores_hidden_dirs() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("visible.md"), "V\n").unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git").join("hidden.md"), "H\n").unwrap();

        let paths = discover_doc_paths(dir.path());
        assert!(paths.contains(&"visible.md".to_string()));
        assert!(!paths.iter().any(|p| p.contains(".git")));
    }
}