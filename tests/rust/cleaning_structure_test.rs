//! End-to-end test: a raw article with code blocks and cross-section short
//! repeats goes through `normalize_article_memory` and retains structure.
//!
//! Exercises the Phase 1 cleaning primitives (T1 normalize-preserving, T2
//! sliding dedup, T3 swap inside `cleaning_internals`) via the public
//! `normalize_article_memory` API — no internals are reached into. The
//! fixture is intentionally padded with 60 unique filler lines between the
//! two `示例说明:` markers so the second occurrence lands outside the
//! 50-line sliding dedup window and survives.

use crate::article_memory::{
    add_article_memory, normalize_article_memory, ArticleMemoryAddRequest,
    ArticleMemoryRecordStatus,
};
use crate::{init_article_memory, RuntimePaths};
use tempfile::TempDir;

fn test_paths() -> (TempDir, RuntimePaths) {
    let tmp = TempDir::new().expect("tempdir");
    let paths = RuntimePaths {
        repo_root: tmp.path().to_path_buf(),
        runtime_dir: tmp.path().join(".runtime").join("davis"),
    };
    std::fs::create_dir_all(paths.runtime_dir.join("state")).expect("create state dir");
    init_article_memory(&paths).expect("init");
    (tmp, paths)
}

#[tokio::test]
async fn normalize_preserves_fenced_code_and_cross_section_repeats() {
    let (_tmp, paths) = test_paths();

    // 60 unique, short-ish filler lines so the second `示例说明:` lands beyond the
    // 50-line sliding dedup window. Each line is distinct prose (<80 chars so
    // it does not hit the short-line bypass) to also verify the window logic
    // tracks recent-N rather than document-global uniqueness.
    let filler: String = (0..60)
        .map(|i| format!("Filler line number {i} has unique content here."))
        .collect::<Vec<_>>()
        .join("\n\n");

    let raw_md = format!(
        "# Example Article\n\
         \n\
         Example one.\n\
         \n\
         ```rust\n\
         \x20   let x = 1;\n\
         \x20   let y = 2;\n\
         ```\n\
         \n\
         More prose here with enough    extra   whitespace   to   trigger   folding.\n\
         \n\
         ## Section Two\n\
         \n\
         示例说明:\n\
         \n\
         Example two.\n\
         \n\
         {filler}\n\
         \n\
         ## Section Three\n\
         \n\
         示例说明:\n\
         \n\
         Example three."
    );

    let req = ArticleMemoryAddRequest {
        title: "Test".into(),
        url: Some("https://example.test/a".into()),
        source: "test".into(),
        language: None,
        tags: vec![],
        content: raw_md.clone(),
        summary: None,
        translation: None,
        status: ArticleMemoryRecordStatus::Candidate,
        value_score: None,
        notes: None,
    };
    let record = add_article_memory(&paths, req).expect("add");

    let resp = normalize_article_memory(&paths, None, None, &record.id)
        .await
        .expect("normalize");

    let normalized_path = std::path::Path::new(&resp.normalized_path);
    let contents = std::fs::read_to_string(normalized_path).expect("read normalized");

    // --- Fence preservation (T1 + T3): 4-space indent inside the ```rust``` block survives.
    assert!(
        contents.contains("    let x = 1;"),
        "code-block indent lost:\n{contents}"
    );
    assert!(
        contents.contains("    let y = 2;"),
        "code-block indent lost:\n{contents}"
    );

    // --- Cross-section short-repeat (T2 SlidingDedup window=50): both `示例说明:`
    // survive because filler lines push the first occurrence out of the window
    // by the time the second arrives.
    let count = contents.matches("示例说明:").count();
    assert!(
        count >= 2,
        "expected >=2 instances of 示例说明: across sections, got {count} in:\n{contents}"
    );

    // --- Whitespace folding on normal prose: the 3+ consecutive spaces in the
    // sample line get collapsed down by the normalizer.
    assert!(
        !contents.contains("enough    extra"),
        "expected whitespace folding on normal prose, but found 4+ consecutive spaces in:\n{contents}"
    );
}
