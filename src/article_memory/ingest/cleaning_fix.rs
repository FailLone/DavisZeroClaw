//! Structure-preserving markdown normalization.
//!
//! Replaces the T1-era `normalize_line` which unconditionally ran
//! `split_whitespace()` and destroyed fenced code blocks and list indentation.

#![allow(dead_code)]

/// Normalize a single line of markdown, preserving fenced-code content and
/// list indentation, while folding internal runs of whitespace into single
/// spaces.
///
/// When `in_code_fence` is true, the line is returned unchanged (callers
/// track fence state themselves — see `normalize_markdown_preserving_structure`).
pub fn normalize_line_preserving(line: &str, in_code_fence: bool) -> String {
    if in_code_fence {
        return line.to_string();
    }
    // Detect and preserve leading whitespace for list / indented content.
    let indent_len: usize = line
        .chars()
        .take_while(|c| c.is_whitespace() && *c != '\n')
        .count();
    let indent: String = line.chars().take(indent_len).collect();
    let body: String = line.chars().skip(indent_len).collect();
    let folded = body
        .replace('\u{00a0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if folded.is_empty() {
        indent.trim_end().to_string()
    } else {
        format!("{indent}{folded}")
    }
}

/// Normalize a multi-line markdown string while respecting fenced code blocks.
pub fn normalize_markdown_preserving_structure(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push(line.to_string());
            continue;
        }
        out.push(normalize_line_preserving(line, in_fence));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_line_collapses_internal_whitespace() {
        let r = normalize_line_preserving("hello    world  \t  again", false);
        assert_eq!(r, "hello world again");
    }

    #[test]
    fn list_indent_preserved() {
        let r = normalize_line_preserving("  - nested    item", false);
        assert_eq!(r, "  - nested item");
    }

    #[test]
    fn code_fence_line_unchanged() {
        let r = normalize_line_preserving("    let x = 1;  // indent matters", true);
        assert_eq!(r, "    let x = 1;  // indent matters");
    }

    #[test]
    fn full_document_preserves_fenced_block() {
        let md = "Para one with    extra  spaces.\n\n```\n    let x = 1;\n    let y = 2;\n```\n\nPara two\twith\ttabs.";
        let out = normalize_markdown_preserving_structure(md);
        assert!(out.contains("Para one with extra spaces."));
        assert!(out.contains("    let x = 1;")); // 4-space indent kept
        assert!(out.contains("    let y = 2;"));
        assert!(out.contains("Para two with tabs."));
    }

    #[test]
    fn nbsp_becomes_space_outside_fence() {
        let r = normalize_line_preserving("a\u{00a0}b\u{00a0}c", false);
        assert_eq!(r, "a b c");
    }

    #[test]
    fn tilde_fences_toggled_too() {
        let md = "~~~\n  indented\n~~~\nfollowing";
        let out = normalize_markdown_preserving_structure(md);
        assert!(out.contains("  indented"));
    }
}
