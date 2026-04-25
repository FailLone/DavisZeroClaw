//! Parse RSS/Atom/sitemap bytes into candidate URLs.
//!
//! `feed-rs` handles RSS 2.0 and Atom natively. Sitemap xml uses a simpler
//! `urlset` schema — we parse it via a hand-written scraper to avoid dragging
//! in a second xml dep.

use anyhow::{Context, Result};
use scraper::{Html, Selector};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateLink {
    pub url: String,
    pub title: Option<String>,
    pub summary: Option<String>,
}

pub fn parse_feed(bytes: &[u8]) -> Result<Vec<CandidateLink>> {
    let feed = feed_rs::parser::parse(bytes).context("feed-rs parse failed")?;
    let mut out = Vec::with_capacity(feed.entries.len());
    for entry in feed.entries {
        let url = entry.links.first().map(|l| l.href.clone());
        let Some(url) = url else { continue };
        out.push(CandidateLink {
            url,
            title: entry.title.map(|t| t.content),
            summary: entry.summary.map(|t| t.content),
        });
    }
    Ok(out)
}

pub fn parse_sitemap(bytes: &[u8]) -> Result<Vec<CandidateLink>> {
    let text = std::str::from_utf8(bytes).context("sitemap not utf-8")?;
    let doc = Html::parse_document(text);
    let sel = Selector::parse("loc").expect("static selector");
    let out = doc
        .select(&sel)
        .map(|node| CandidateLink {
            url: node.text().collect::<String>().trim().to_string(),
            title: None,
            summary: None,
        })
        .filter(|c| !c.url.is_empty())
        .collect();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rss_items() {
        let bytes = std::fs::read("tests/fixtures/discovery/rss_sample.xml").unwrap();
        let links = parse_feed(&bytes).unwrap();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].url, "https://example.com/async-rust-patterns");
        assert_eq!(links[0].title.as_deref(), Some("Async Rust patterns"));
    }

    #[test]
    fn parses_atom_entries() {
        let bytes = std::fs::read("tests/fixtures/discovery/atom_sample.xml").unwrap();
        let links = parse_feed(&bytes).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "https://example.com/io-uring");
    }

    #[test]
    fn parses_sitemap_locs() {
        let bytes = std::fs::read("tests/fixtures/discovery/sitemap_sample.xml").unwrap();
        let links = parse_sitemap(&bytes).unwrap();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].url, "https://example.com/docs/tokio");
    }

    #[test]
    fn invalid_feed_returns_err() {
        let err = parse_feed(b"<not xml").unwrap_err().to_string();
        assert!(err.contains("parse"), "{err}");
    }
}
