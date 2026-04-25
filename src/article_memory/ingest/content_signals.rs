//! Content-level statistical signals computed from extracted markdown.
//!
//! Phase 1 uses these only inside the quality gate. Phase 2 will also feed
//! them into the deterministic value score.

// Consumers (quality gate, value scorer) land in later tasks; until then
// these items are exercised only by the unit tests in this file.
#![allow(dead_code)]

#[derive(Debug, Clone, PartialEq)]
pub struct ContentSignals {
    pub total_chars: usize,
    pub code_density: f32,
    pub link_density: f32,
    pub heading_depth: usize,
    pub heading_count: usize,
    pub paragraph_count: usize,
    pub list_ratio: f32,
    pub avg_paragraph_chars: usize,
    pub alpha_ratio: f32,
    pub symbol_ratio: f32,
}

pub fn compute_signals(markdown: &str) -> ContentSignals {
    let total_chars = markdown.chars().count();
    if total_chars == 0 {
        return ContentSignals {
            total_chars: 0,
            code_density: 0.0,
            link_density: 0.0,
            heading_depth: 0,
            heading_count: 0,
            paragraph_count: 0,
            list_ratio: 0.0,
            avg_paragraph_chars: 0,
            alpha_ratio: 0.0,
            symbol_ratio: 0.0,
        };
    }

    // Code density: fenced code block chars + inline-code chars
    let mut code_chars: usize = 0;
    let mut in_fence = false;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            code_chars += line.chars().count() + 1; // +1 for newline
            continue;
        }
        if in_fence {
            code_chars += line.chars().count() + 1;
        }
    }
    // Inline code (simple backtick counter; excludes lines already in fences).
    let inline_code_chars = count_inline_code_chars(markdown);
    code_chars += inline_code_chars;

    let code_density = (code_chars as f32 / total_chars as f32).clamp(0.0, 1.0);

    // Heading stats
    let mut heading_depth = 0usize;
    let mut heading_count = 0usize;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') {
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes)
            && trimmed
                .chars()
                .nth(hashes)
                .map(|c| c == ' ')
                .unwrap_or(false)
        {
            heading_count += 1;
            if hashes > heading_depth {
                heading_depth = hashes;
            }
        }
    }

    // Paragraphs: non-empty blocks separated by blank lines.
    let paragraphs: Vec<&str> = markdown
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let paragraph_count = paragraphs.len();
    let paragraph_chars_total: usize = paragraphs.iter().map(|p| p.chars().count()).sum();
    let avg_paragraph_chars = paragraph_chars_total
        .checked_div(paragraph_count)
        .unwrap_or(0);

    // Link density: number of [..](..) matches / word count
    let link_count = count_markdown_links(markdown);
    let word_count = markdown.split_whitespace().count().max(1);
    let link_density = (link_count as f32 / word_count as f32).clamp(0.0, 1.0);

    // List ratio
    let lines: Vec<&str> = markdown.lines().collect();
    let total_lines = lines.len().max(1);
    let list_lines = lines
        .iter()
        .filter(|line| {
            let t = line.trim_start();
            t.starts_with("- ")
                || t.starts_with("* ")
                || t.starts_with("+ ")
                || starts_with_numbered_list(t)
        })
        .count();
    let list_ratio = list_lines as f32 / total_lines as f32;

    // Char-class ratios
    let mut alpha = 0usize;
    let mut symbol = 0usize;
    for ch in markdown.chars() {
        if ch.is_alphabetic() {
            alpha += 1;
        } else if "!@#$%^&*()[]{}|\\/<>~`".contains(ch) {
            symbol += 1;
        }
    }
    let alpha_ratio = alpha as f32 / total_chars as f32;
    let symbol_ratio = symbol as f32 / total_chars as f32;

    ContentSignals {
        total_chars,
        code_density,
        link_density,
        heading_depth,
        heading_count,
        paragraph_count,
        list_ratio,
        avg_paragraph_chars,
        alpha_ratio,
        symbol_ratio,
    }
}

fn count_inline_code_chars(markdown: &str) -> usize {
    let mut total = 0usize;
    let mut chars = markdown.chars().peekable();
    let mut in_fence = false;
    let mut fence_buf = String::new();
    while let Some(c) = chars.next() {
        fence_buf.push(c);
        if fence_buf.ends_with("```") {
            in_fence = !in_fence;
            fence_buf.clear();
        }
        if fence_buf.len() > 3 {
            fence_buf.drain(..1);
        }
        if in_fence {
            continue;
        }
        if c == '`' {
            // capture until next `
            let mut run = 0usize;
            for nc in chars.by_ref() {
                if nc == '`' {
                    break;
                }
                run += 1;
            }
            total += run;
        }
    }
    total
}

fn count_markdown_links(markdown: &str) -> usize {
    // Naive: count "](" occurrences. Good enough for density estimation.
    markdown.matches("](").count()
}

fn starts_with_numbered_list(line: &str) -> bool {
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    digits > 0
        && line.chars().nth(digits).map(|c| c == '.').unwrap_or(false)
        && line
            .chars()
            .nth(digits + 1)
            .map(|c| c == ' ')
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_markdown_yields_zero_signals() {
        let s = compute_signals("");
        assert_eq!(s.total_chars, 0);
        assert_eq!(s.paragraph_count, 0);
        assert_eq!(s.heading_depth, 0);
    }

    #[test]
    fn paragraph_count_splits_on_blank_lines() {
        let md = "first para\n\nsecond para\n\nthird para";
        let s = compute_signals(md);
        assert_eq!(s.paragraph_count, 3);
        assert!(s.avg_paragraph_chars >= 9);
    }

    #[test]
    fn heading_depth_tracks_max_level() {
        let md = "# H1\n\n## H2\n\n#### H4\n\nbody";
        let s = compute_signals(md);
        assert_eq!(s.heading_depth, 4);
        assert_eq!(s.heading_count, 3);
    }

    #[test]
    fn code_density_counts_fenced_and_inline() {
        let md = "prose prose prose\n\n```\nprint(x)\n```\n\nmore `code` here";
        let s = compute_signals(md);
        assert!(s.code_density > 0.0);
        assert!(s.code_density < 1.0);
    }

    #[test]
    fn link_density_counts_markdown_links() {
        let md = "one [link](http://a) two [link](http://b) words words";
        let s = compute_signals(md);
        assert!(s.link_density > 0.0);
    }

    #[test]
    fn list_ratio_measures_list_lines() {
        let md = "- a\n- b\n- c\n\ntext";
        let s = compute_signals(md);
        assert!(s.list_ratio > 0.5);
    }

    #[test]
    fn alpha_ratio_detects_symbol_heavy() {
        let md = "############!!!!!!!!########";
        let s = compute_signals(md);
        assert!(s.alpha_ratio < 0.1);
        assert!(s.symbol_ratio > 0.1);
    }
}
