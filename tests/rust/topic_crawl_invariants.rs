//! These invariants encode architectural decisions that would re-open debate
//! every time a new LLM caller is added. See
//! docs/superpowers/plans/2026-04-25-topic-crawl-mvp.md §"Anchor decisions" A1-A3.

use std::fs;
use std::path::Path;

#[test]
fn remote_chat_is_not_imported_outside_translate_module() {
    let src = Path::new("src");
    for entry in walkdir::WalkDir::new(src).into_iter().flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        // Allow translate module to own the import.
        if p.components().any(|c| c.as_os_str() == "translate") {
            continue;
        }
        let body = fs::read_to_string(p).unwrap();
        assert!(
            !body.contains("translate::remote_chat"),
            "{p:?} imports translate::remote_chat; remote_chat must stay private to translate"
        );
        assert!(
            !body.contains("RemoteChat"),
            "{p:?} references RemoteChat; stay inside translate"
        );
    }
}

#[test]
fn no_zeroclaw_crate_in_cargo_toml() {
    let body = fs::read_to_string("Cargo.toml").unwrap();
    for (i, line) in body.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("zeroclaw") && trimmed.contains('=') {
            panic!("Cargo.toml line {i}: {line}\n— zeroclaw must not be a Cargo dep");
        }
    }
}
