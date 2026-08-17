//! Markdown spec block 解析（pulldown-cmark 事件流）。
//!
//! 解析 Markdown 文件，將每個 heading（非 `Symbol:` 宣告）定義為一個
//! [`SpecBlock`]。`# Symbol: <name>` heading 為符號宣告，歸屬前一個 block。
//!
//! 硬連結來源：
//! - `# Symbol: <name>` — 一般 heading 事件，text 前綴為 `Symbol:`
//! - 純 heading（非 Symbol）= 新 block；符號在 block 內
//!
//! 不含 code fence 內的 `#` — pulldown-cmark 不將其解析為 heading。

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use sha1::{Digest, Sha1};

/// 一個 Markdown spec block（由 heading 定義）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecBlock {
    pub doc_path: String,
    pub heading: String,
    pub spec_id: String,
    pub content: String,
    pub symbols: Vec<String>,
    pub block_signature: String,
}

/// 從 raw Markdown text 萃取目錄內所有的 spec blocks。
///
/// `# Symbol: <name>` 事件被當作該 block 的符號宣告，不另起 block。
/// Heading 之外的內容（前言）不構成 block。
#[must_use]
pub fn extract_blocks(markdown: &str, doc_path: &str) -> Vec<SpecBlock> {
    let parser = Parser::new(markdown);
    let mut blocks: Vec<SpecBlock> = Vec::new();
    let mut current: Option<SpecBlock> = None;
    let mut heading_text = String::new();
    let mut in_heading = false;
    let mut content_buf = String::new();

    for event in parser {
        match event {
            Event::Start(Tag::Heading { .. }) => {
                in_heading = true;
                heading_text.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                in_heading = false;
                let heading = heading_text.trim().to_string();
                if let Some(rest) = heading.strip_prefix("Symbol:") {
                    let symbol = rest.trim().to_string();
                    if !symbol.is_empty() {
                        if let Some(block) = &mut current {
                            if !block.symbols.contains(&symbol) {
                                block.symbols.push(symbol);
                            }
                        }
                    }
                } else {
                    if let Some(mut block) = current.take() {
                        block.content = std::mem::take(&mut content_buf);
                        block.block_signature = sha1_hex(&block.content);
                        blocks.push(block);
                    }
                    current = Some(SpecBlock {
                        doc_path: doc_path.to_string(),
                        heading: heading.clone(),
                        spec_id: spec_id_for(doc_path, &heading),
                        content: String::new(),
                        symbols: Vec::new(),
                        block_signature: String::new(),
                    });
                }
            }
            Event::Text(text) => {
                if in_heading {
                    heading_text.push_str(&text);
                } else if current.is_some() {
                    content_buf.push_str(&text);
                    content_buf.push('\n');
                }
            }
            Event::Code(text) => {
                if in_heading {
                    heading_text.push_str(&text);
                } else if current.is_some() {
                    content_buf.push_str(&text);
                    content_buf.push('\n');
                }
            }
            _ => {}
        }
    }

    if let Some(mut block) = current.take() {
        block.content = std::mem::take(&mut content_buf);
        block.block_signature = sha1_hex(&block.content);
        blocks.push(block);
    }
    blocks
}

/// 檢測 `# Symbol:` 一行的 heading level（用於 `# Symbol:` headingLv0 偵測）。
///
/// 返回此 heading 是否為 `Symbol:` 宣告（供 caller 區分新 block 與 symbol 宣告）。
#[must_use]
pub fn is_symbol_heading(heading_text: &str) -> bool {
    heading_text.trim().to_lowercase().starts_with("symbol:")
}

/// 從符號取得 HeadingLevel（H1~H6），非 heading 為 `None`。
#[must_use]
pub fn heading_level_of(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// spec_id = sha1(doc_path + heading) 的前 12 字 hex。
pub fn spec_id_for(doc_path: &str, heading: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(doc_path.as_bytes());
    hasher.update(heading.as_bytes());
    let hash = hasher.finalize();
    hex_lower(&hash)[..12].to_string()
}

/// sha1 hex (40 chars).
fn sha1_hex(content: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(content.as_bytes());
    hex_lower(&hasher.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "docs/auth.md";

    #[test]
    fn empty_markdown_yields_no_blocks() {
        assert!(extract_blocks("", DOC).is_empty());
    }

    #[test]
    fn single_heading_yields_one_block() {
        let md = "## verify_token\n\nValidates JWT.\n";
        let blocks = extract_blocks(md, DOC);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].heading, "verify_token");
        assert_eq!(blocks[0].doc_path, DOC);
        assert!(blocks[0].content.contains("Validates JWT."));
        assert!(blocks[0].symbols.is_empty());
        assert!(blocks[0].block_signature.len() == 40);
    }

    #[test]
    fn multiple_headings_split_into_blocks() {
        let md = "## verify_token\n\nToken validation.\n\n## get_user\n\nUser lookup.\n";
        let blocks = extract_blocks(md, DOC);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].heading, "verify_token");
        assert_eq!(blocks[1].heading, "get_user");
        assert!(blocks[0].content.contains("Token validation."));
        assert!(!blocks[0].content.contains("User lookup."));
        assert!(blocks[1].content.contains("User lookup."));
    }

    #[test]
    fn symbol_declaration_attaches_to_current_block() {
        let md = "## verify_token\n\nValidates tokens.\n\n# Symbol: crate::auth::verify_token\n\nMore content.\n";
        let blocks = extract_blocks(md, DOC);
        assert_eq!(blocks.len(), 1, "Symbol heading should not split block");
        assert_eq!(blocks[0].symbols, vec!["crate::auth::verify_token"]);
        assert!(blocks[0].content.contains("Validates tokens."));
        assert!(blocks[0].content.contains("More content."));
    }

    #[test]
    fn multiple_symbols_in_one_block() {
        let md = "## auth\n\nAuth module.\n\n# Symbol: crate::auth::login\n\n# Symbol: crate::auth::logout\n";
        let blocks = extract_blocks(md, DOC);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].symbols, vec!["crate::auth::login", "crate::auth::logout"]);
    }

    #[test]
    fn symbol_before_any_block_is_dropped() {
        let md = "# Symbol: orphan\n\n## verify_token\n\ncontent\n";
        let blocks = extract_blocks(md, DOC);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].symbols.is_empty(), "orphan symbol should be dropped");
    }

    #[test]
    fn symbol_inside_code_fence_is_not_captured() {
        let md = "## verify_token\n\nExample:\n\n```\n# Symbol: fake\n```\n\nReal.\n";
        let blocks = extract_blocks(md, DOC);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].symbols.is_empty(), "code-fence # Symbol: is not a heading");
    }

    #[test]
    fn spec_id_is_deterministic() {
        let md = "## verify_token\n\ncontent\n";
        let b1 = extract_blocks(md, DOC);
        let b2 = extract_blocks(md, DOC);
        assert_eq!(b1[0].spec_id, b2[0].spec_id);
        assert_eq!(b1[0].spec_id.len(), 12);
    }

    #[test]
    fn spec_id_differs_by_doc_path() {
        let md = "## verify_token\n\ncontent\n";
        let b1 = extract_blocks(md, "docs/a.md");
        let b2 = extract_blocks(md, "docs/b.md");
        assert_ne!(b1[0].spec_id, b2[0].spec_id);
    }

    #[test]
    fn block_signature_changes_with_content() {
        let md1 = "## verify_token\n\nOriginal.\n";
        let md2 = "## verify_token\n\nModified.\n";
        let b1 = extract_blocks(md1, DOC);
        let b2 = extract_blocks(md2, DOC);
        assert_eq!(b1[0].spec_id, b2[0].spec_id, "spec_id is by heading, not content");
        assert_ne!(b1[0].block_signature, b2[0].block_signature);
    }

    #[test]
    fn h1_title_then_h2_blocks() {
        let md = "# Auth Module\n\nOverview.\n\n## verify_token\n\nContent.\n\n## get_user\n\nMore.\n";
        let blocks = extract_blocks(md, DOC);
        assert_eq!(blocks.len(), 3, "H1 title is also a block");
        assert_eq!(blocks[0].heading, "Auth Module");
        assert_eq!(blocks[1].heading, "verify_token");
        assert_eq!(blocks[2].heading, "get_user");
    }

    #[test]
    fn heading_with_inline_code() {
        // pulldown-cmark strips inline-code delimiters: `` `verify_token` `` → "verify_token".
        let md = "## `verify_token` function\n\nContent.\n\n# Symbol: crate::auth::verify_token\n";
        let blocks = extract_blocks(md, DOC);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].heading, "verify_token function");
        assert_eq!(blocks[0].symbols, vec!["crate::auth::verify_token"]);
    }

    #[test]
    fn duplicate_symbol_deduplicated() {
        let md = "## auth\n\nContent.\n\n# Symbol: crate::auth::login\n\n# Symbol: crate::auth::login\n";
        let blocks = extract_blocks(md, DOC);
        assert_eq!(blocks[0].symbols, vec!["crate::auth::login"]);
    }
}