//! graphify-plugin-skeleton — Compact AST skeleton extraction for LLM context.
//!
//! Uses graphify-core's tree-sitter parsers (9 languages) to extract top-level
//! declarations and produce a compact text skeleton (~300 tokens) suitable for
//! `inspect_context`-style LLM context injection.
//!
//! This is a stateless plugin (no `GraphifyPlugin` trait) — it reads files and
//! returns text, with no workspace binding or graph state.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use graphify_core::extract::extract_file;

/// The kind categories we display in the skeleton, ordered by display priority.
const DISPLAY_ORDER: &[&str] = &[
    "struct", "enum", "union", "class", "interface", "type",
    "trait", "impl_block",
    "function", "method", "const", "static",
];

/// Produce a compact text skeleton for a source file.
///
/// Returns a formatted string like:
/// ```text
/// // ── Structs (2) ──
///   pub struct Config { ... }  // line 5
/// ```
pub fn extract_skeleton(path: &str) -> Result<String> {
    let file_path = Path::new(path);
    let content = fs::read_to_string(file_path)
        .with_context(|| format!("Failed to read {}", path))?;
    let lines: Vec<&str> = content.lines().collect();

    // Use graphify-core's tree-sitter parsers to extract AST nodes
    let result = extract_file(file_path)
        .with_context(|| format!("graphify extraction failed for {}", path))?;

    if result.nodes.is_empty() {
        return Ok(format!("// (empty — no declarations found in {})", path));
    }

    // Group nodes by kind, collecting (signature, line_number) pairs
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();

    for node in &result.nodes {
        let kind = &node.kind;

        // Skip internal/meta node kinds
        if matches!(kind.as_str(), "module" | "root" | "program") {
            continue;
        }

        // Only include top-level-like declaration kinds
        if !is_declaration_kind(kind) {
            continue;
        }

        let sig = extract_signature(&lines, node.start_line, node.end_line);
        groups
            .entry(kind.clone())
            .or_default()
            .push((sig, node.start_line));
    }

    // Build skeleton text
    let mut output = String::new();
    output.push_str(&format!("// {} — tokens: {} -> {}\n\n",
        file_path.file_name().unwrap_or_default().to_string_lossy(),
        count_tokens(&content),
        estimate_tokens(&groups),
    ));

    // Emit groups in display order, then any remaining kinds
    let mut seen = std::collections::HashSet::new();
    for kind in DISPLAY_ORDER {
        if let Some(entries) = groups.remove(*kind) {
            emit_group(&mut output, kind, &entries);
            seen.insert(kind.to_string());
        }
    }
    // Remaining kinds (not in DISPLAY_ORDER)
    for (kind, entries) in &groups {
        if !seen.contains(kind) {
            emit_group(&mut output, kind, entries);
        }
    }

    Ok(output)
}

/// Check if a node kind is a top-level declaration worth showing.
fn is_declaration_kind(kind: &str) -> bool {
    matches!(
        kind,
        "struct"
            | "enum"
            | "union"
            | "class"
            | "interface"
            | "type"
            | "trait"
            | "impl_block"
            | "function"
            | "method"
            | "const"
            | "static"
            | "variable"
    )
}

/// Extract the human-readable signature from source lines.
///
/// Concatenates lines from `start_line` until the body starts (`{` for C-family,
/// `:` for Python), then strips the body and returns just the declaration head.
fn extract_signature(lines: &[&str], start_line: usize, end_line: usize) -> String {
    if start_line == 0 || start_line > lines.len() {
        return String::new();
    }

    let mut sig = String::new();
    let idx0 = start_line.saturating_sub(1);

    for i in idx0..end_line.min(lines.len()) {
        let line = lines[i];
        let trimmed = line.trim();

        // Skip empty lines
        if trimmed.is_empty() {
            continue;
        }

        // Check for body-brace (C-family) or doc-comment separator
        if let Some(pos) = trimmed.find('{') {
            // Found body start — take everything before `{` + "..."
            let before = trimmed[..pos].trim_end();
            sig.push_str(before);
            sig.push_str(" { ... }");
            return sig;
        }

        sig.push_str(trimmed);
        sig.push(' ');

        // For Python/indent-based languages: if this line ends with `:`
        // and the next line has greater indent, the body starts here.
        if trimmed.ends_with(':') {
            if i + 1 < end_line.min(lines.len()) {
                let next = lines[i + 1].trim();
                if !next.is_empty() && !next.starts_with('#') {
                    // Check indent: next line is more indented → body starts
                    let cur_indent = lines[i].len() - lines[i].trim_start().len();
                    let next_indent = lines[i + 1].len() - lines[i + 1].trim_start().len();
                    if next_indent > cur_indent {
                        // Body starts after `:`, keep the signature as-is
                        break;
                    }
                }
            }
        }
    }

    let result = sig.trim().to_string();
    if result.is_empty() { result } else { result }
}

/// Write one group of declarations into the skeleton output.
fn emit_group(output: &mut String, kind: &str, entries: &[(String, usize)]) {
    if entries.is_empty() {
        return;
    }
    let header = pluralize_kind(kind);
    output.push_str(&format!("// ── {} ({}) ──\n", header, entries.len()));
    for (sig, line) in entries {
        output.push_str(&format!("  {}  // line {}\n", sig, line));
    }
    output.push('\n');
}

/// Pluralize a kind name for the section header.
fn pluralize_kind(kind: &str) -> &str {
    match kind {
        "class" => "Classes",
        "function" => "Functions",
        "struct" => "Structs",
        "enum" => "Enums",
        "trait" => "Traits",
        "impl_block" => "Impl Blocks",
        "interface" => "Interfaces",
        "type" => "Types",
        "method" => "Methods",
        "const" => "Constants",
        "static" => "Statics",
        "variable" => "Variables",
        "union" => "Unions",
        _ => {
            // Generic pluralize: add 's'
            // This is a static str, so we can't dynamically allocate
            kind // fallback to the kind itself
        }
    }
}

/// Count approximate tokens (whitespace-separated words + punctuation groups).
fn count_tokens(content: &str) -> usize {
    content.split_whitespace()
        .flat_map(|w| w.split(|c: char| !c.is_alphanumeric() && c != '_'))
        .filter(|s| !s.is_empty())
        .count()
}

/// Estimate output tokens from the skeleton structure.
fn estimate_tokens(groups: &std::collections::BTreeMap<String, Vec<(String, usize)>>) -> usize {
    let mut total = 0;
    for (kind, entries) in groups {
        // Header line
        total += 5 + kind.len() + 3; // "// ── Kind (N) ──\n"
        for (sig, _) in entries {
            total += sig.split_whitespace().count() + 3; // "  sig  // line N\n"
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_rust_skeleton() {
        let dir = tempfile::tempdir().unwrap();
        let rs_path = dir.path().join("test.rs");
        let content = r#"
use std::collections::HashMap;

/// A rate limiter implementation.
pub struct RateLimiter {
    max: u32,
    window: u64,
}

impl RateLimiter {
    pub fn new(max: u32, window: u64) -> Self {
        RateLimiter { max, window }
    }
}
"#;
        let mut f = fs::File::create(&rs_path).unwrap();
        f.write_all(content.as_bytes()).unwrap();

        let skeleton = extract_skeleton(rs_path.to_str().unwrap()).unwrap();
        println!("=== Rust skeleton ===\n{}", skeleton);

        assert!(skeleton.contains("Structs"));
        assert!(skeleton.contains("RateLimiter"));
        assert!(skeleton.contains("Functions"));
        assert!(skeleton.contains("new"));
    }

    #[test]
    fn test_python_skeleton() {
        let dir = tempfile::tempdir().unwrap();
        let py_path = dir.path().join("test.py");
        let content = r#"
import os
from typing import Optional

class Config:
    def __init__(self, name: str):
        self.name = name

def hello(name: str) -> str:
    return f"Hi {name}"
"#;
        let mut f = fs::File::create(&py_path).unwrap();
        f.write_all(content.as_bytes()).unwrap();

        let skeleton = extract_skeleton(py_path.to_str().unwrap()).unwrap();
        println!("=== Python skeleton ===\n{}", skeleton);

        assert!(skeleton.contains("Classes"));
        assert!(skeleton.contains("Config"));
        assert!(skeleton.contains("Functions"));
        assert!(skeleton.contains("hello"));
    }
}