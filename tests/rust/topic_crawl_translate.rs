//! Seeds an `ArticleMemoryIndex` with one English record and runs the
//! translate worker once against an axum mock impersonating zeroclaw's
//! `POST /api/chat`. Asserts the worker wrote the translation file,
//! stamped `translation_path` on the record, and reported the outcome.

use std::sync::Arc;

use axum::{routing::post, Json, Router};
use davis_zero_claw::mempalace_sink::testing::NoopSink;
use davis_zero_claw::{
    init_article_memory, load_article_index, run_one_cycle, save_article_index,
    ArticleMemoryRecord, ArticleMemoryRecordStatus, RuntimePaths, TranslateConfig,
    TranslateWorkerDeps,
};
use serde_json::{json, Value};

#[tokio::test]
async fn translates_single_english_article_end_to_end() {
    // 1. Spin up a mock zeroclaw `/api/chat` that always returns a
    //    Chinese-looking translation body. The shape mirrors the
    //    non-streaming branch of `remote_chat::RemoteChat::translate_to_zh`.
    let app = Router::new().route(
        "/api/chat",
        post(|Json(_): Json<Value>| async {
            Json(json!({"content": "译文内容\n\n一段翻译"}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base_url = format!("http://{addr}");

    // 2. Seed a runtime tempdir with a single saved, English, untranslated
    //    article. `init_article_memory` bootstraps an empty index on disk;
    //    we then load → push → save to plant the fixture record.
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = RuntimePaths {
        repo_root: tmp.path().to_path_buf(),
        runtime_dir: tmp.path().to_path_buf(),
    };
    init_article_memory(&paths).unwrap();

    let article_dir = paths.article_memory_dir().join("a1");
    std::fs::create_dir_all(&article_dir).unwrap();
    std::fs::write(article_dir.join("normalized.md"), "hello world").unwrap();

    let mut index = load_article_index(&paths).unwrap();
    index.articles.push(ArticleMemoryRecord {
        id: "a1".into(),
        title: "hello".into(),
        url: Some("https://ex.com/a".into()),
        source: "test".into(),
        language: Some("en".into()),
        tags: vec![],
        status: ArticleMemoryRecordStatus::Saved,
        value_score: Some(0.8),
        captured_at: "2026-04-01T00:00:00Z".into(),
        updated_at: "2026-04-01T00:00:00Z".into(),
        content_path: "a1/content.md".into(),
        raw_path: None,
        normalized_path: Some("a1/normalized.md".into()),
        summary_path: None,
        translation_path: None,
        notes: None,
        clean_status: Some("ok".into()),
        clean_profile: Some("default".into()),
    });
    save_article_index(&paths, &index).unwrap();

    // 3. Run one cycle of the translate worker against the mock. The
    //    `NoopSink` satisfies `TranslateWorkerDeps::mempalace_sink` without
    //    spawning the real MCP child process.
    let deps = TranslateWorkerDeps {
        config: Arc::new(TranslateConfig {
            enabled: true,
            zeroclaw_base_url: base_url,
            ..TranslateConfig::default()
        }),
        http: reqwest::Client::new(),
        paths: paths.clone(),
        mempalace_sink: Arc::new(NoopSink),
    };

    let report = run_one_cycle(&deps).await.unwrap();

    // 4. Report-level assertions: one success, zero failures.
    assert_eq!(report.translated, 1, "expected one translation success");
    assert_eq!(report.failed, 0, "expected zero failures");

    // 5. Record-level assertion: `translation_path` is stamped to the
    //    canonical `{id}/translation.md` location.
    let after = load_article_index(&paths).unwrap();
    let record = after
        .articles
        .iter()
        .find(|r| r.id == "a1")
        .expect("seeded record should still be in the index");
    assert_eq!(
        record.translation_path.as_deref(),
        Some("a1/translation.md"),
        "translation_path should point at the new translation file"
    );

    // 6. Filesystem-level assertion: the translation body actually lands on
    //    disk, and it contains the Chinese payload the mock returned.
    let translation_abs = paths
        .article_memory_dir()
        .join(record.translation_path.as_deref().unwrap());
    let written = std::fs::read_to_string(&translation_abs).unwrap();
    assert!(
        written.contains("译文"),
        "translation.md should contain the mock's Chinese body; got: {written}"
    );
}
