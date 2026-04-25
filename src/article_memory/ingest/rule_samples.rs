//! Per-host accumulator for HTML samples awaiting a rule-learning round.

#![allow(dead_code)]

use super::rule_types::RuleSample;
use crate::RuntimePaths;
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

pub struct SampleStore {
    root: PathBuf,
}

impl SampleStore {
    pub fn new(paths: &RuntimePaths) -> Self {
        let root = paths.article_memory_dir().join("rule_samples");
        Self { root }
    }

    fn host_dir(&self, host: &str) -> PathBuf {
        // Sanitize for filesystem: replace '/' and other oddities.
        let safe = host.replace(['/', '\\'], "_");
        self.root.join(safe)
    }

    /// Persist an HTML sample for `host`. Writes the HTML body to a
    /// `.html` file and a sidecar `.json` with metadata.
    pub fn push(
        &self,
        host: &str,
        job_id: &str,
        url: &str,
        html: &str,
        markdown_from_engine: &str,
        failure_reason: &str,
        failure_details: Vec<String>,
    ) -> Result<()> {
        let dir = self.host_dir(host);
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;

        let timestamp = crate::support::isoformat(crate::support::now_utc());
        // Filesystem-safe timestamp: replace ':' with '-' for macOS/Win.
        let ts_safe = timestamp.replace(':', "-");
        let base = format!("{ts_safe}-{job_id}");
        let html_path = dir.join(format!("{base}.html"));
        let json_path = dir.join(format!("{base}.json"));

        let html_rel = pathdiff::diff_paths(&html_path, self.root.parent().unwrap_or(&self.root))
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| html_path.display().to_string());

        let sample = RuleSample {
            url: url.to_string(),
            job_id: job_id.to_string(),
            captured_at: timestamp,
            html_snapshot_path: html_rel,
            markdown_from_engine: markdown_from_engine.to_string(),
            failure_reason: failure_reason.to_string(),
            failure_details,
        };

        fs::write(&html_path, html)?;
        fs::write(&json_path, serde_json::to_string_pretty(&sample)?)?;
        Ok(())
    }

    /// Return the list of hosts whose sample count meets `threshold`.
    pub fn ready_hosts(&self, threshold: usize) -> Vec<String> {
        let mut ready = Vec::new();
        let Ok(entries) = fs::read_dir(&self.root) else {
            return ready;
        };
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let json_count = entry
                .path()
                .read_dir()
                .map(|iter| {
                    iter.flatten()
                        .filter(|e| {
                            e.path()
                                .extension()
                                .and_then(|e| e.to_str())
                                .map(|s| s == "json")
                                .unwrap_or(false)
                        })
                        .count()
                })
                .unwrap_or(0);
            if json_count >= threshold {
                if let Some(name) = entry.file_name().to_str() {
                    ready.push(name.to_string());
                }
            }
        }
        ready
    }

    /// Load up to `limit` most-recent samples for a host.
    pub fn load_samples(&self, host: &str, limit: usize) -> Result<Vec<(RuleSample, String)>> {
        let dir = self.host_dir(host);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut entries: Vec<_> = fs::read_dir(&dir)?
            .flatten()
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s == "json")
                    .unwrap_or(false)
            })
            .collect();
        entries.sort_by_key(|e| e.file_name());
        entries.reverse();

        let mut out = Vec::new();
        for entry in entries.into_iter().take(limit) {
            let json_path = entry.path();
            let html_path = json_path.with_extension("html");
            let sample: RuleSample = serde_json::from_str(&fs::read_to_string(&json_path)?)?;
            let html = fs::read_to_string(&html_path)?;
            out.push((sample, html));
        }
        Ok(out)
    }

    pub fn clear(&self, host: &str) -> Result<()> {
        let dir = self.host_dir(host);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_creates_html_and_json() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths {
            repo_root: temp.path().to_path_buf(),
            runtime_dir: temp.path().join("runtime"),
        };
        std::fs::create_dir_all(paths.article_memory_dir()).unwrap();
        let store = SampleStore::new(&paths);
        store
            .push(
                "example.com",
                "job1",
                "https://example.com/a",
                "<html></html>",
                "# md",
                "hard_fail",
                vec!["markdown_too_short".into()],
            )
            .unwrap();
        let loaded = store.load_samples("example.com", 10).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0.failure_reason, "hard_fail");
        assert_eq!(loaded[0].1, "<html></html>");
    }

    #[test]
    fn ready_hosts_respects_threshold() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths {
            repo_root: temp.path().to_path_buf(),
            runtime_dir: temp.path().join("runtime"),
        };
        std::fs::create_dir_all(paths.article_memory_dir()).unwrap();
        let store = SampleStore::new(&paths);
        store
            .push("a.com", "j1", "u", "h", "m", "hard_fail", vec![])
            .unwrap();
        store
            .push("a.com", "j2", "u", "h", "m", "hard_fail", vec![])
            .unwrap();
        assert_eq!(store.ready_hosts(3), Vec::<String>::new());
        store
            .push("a.com", "j3", "u", "h", "m", "hard_fail", vec![])
            .unwrap();
        assert_eq!(store.ready_hosts(3), vec!["a.com".to_string()]);
    }

    #[test]
    fn clear_removes_all_samples() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths {
            repo_root: temp.path().to_path_buf(),
            runtime_dir: temp.path().join("runtime"),
        };
        std::fs::create_dir_all(paths.article_memory_dir()).unwrap();
        let store = SampleStore::new(&paths);
        store
            .push("a.com", "j1", "u", "h", "m", "hard_fail", vec![])
            .unwrap();
        store.clear("a.com").unwrap();
        assert!(store.load_samples("a.com", 10).unwrap().is_empty());
    }
}
