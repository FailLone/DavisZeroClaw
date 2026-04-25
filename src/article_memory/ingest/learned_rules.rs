//! Load / save / stale-track learned host rules. Also applies a hand-written
//! overrides file (`config/davis/article_memory_overrides.toml`) which takes
//! precedence over learned entries.

#![allow(dead_code)]

use super::rule_types::LearnedRule;
use crate::RuntimePaths;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Deserialize)]
struct OverridesFile {
    #[serde(default)]
    overrides: Vec<OverrideRule>,
}

#[derive(Debug, Deserialize)]
struct OverrideRule {
    host: String,
    #[serde(default)]
    content_selectors: Vec<String>,
    #[serde(default)]
    remove_selectors: Vec<String>,
    #[serde(default)]
    title_selector: Option<String>,
    #[serde(default)]
    start_markers: Vec<String>,
    #[serde(default)]
    end_markers: Vec<String>,
}

impl OverrideRule {
    fn into_learned(self) -> LearnedRule {
        LearnedRule {
            host: self.host,
            version: "override".to_string(),
            content_selectors: self.content_selectors,
            remove_selectors: self.remove_selectors,
            title_selector: self.title_selector,
            start_markers: self.start_markers,
            end_markers: self.end_markers,
            confidence: 1.0,
            reasoning: "hand-written override".to_string(),
            learned_from_sample_count: 0,
            stale: false,
        }
    }
}

#[derive(Clone)]
pub struct LearnedRuleStore {
    learned_path: PathBuf,
    inner: Arc<RwLock<BTreeMap<String, LearnedRule>>>,
    overrides: Arc<BTreeMap<String, LearnedRule>>,
}

impl LearnedRuleStore {
    /// Load learned_rules.json from disk and merge any overrides.toml.
    /// overrides.toml path is passed in (typically
    /// `config/davis/article_memory_overrides.toml` relative to repo_root).
    pub fn load(paths: &RuntimePaths, overrides_path: Option<&std::path::Path>) -> Result<Self> {
        let learned_path = paths.article_memory_dir().join("learned_rules.json");
        let learned: BTreeMap<String, LearnedRule> = if learned_path.exists() {
            let raw = fs::read_to_string(&learned_path)
                .with_context(|| format!("read {}", learned_path.display()))?;
            serde_json::from_str(&raw)
                .with_context(|| format!("parse {}", learned_path.display()))?
        } else {
            BTreeMap::new()
        };

        let mut overrides = BTreeMap::new();
        if let Some(op) = overrides_path {
            if op.exists() {
                let raw =
                    fs::read_to_string(op).with_context(|| format!("read {}", op.display()))?;
                let file: OverridesFile =
                    toml::from_str(&raw).with_context(|| format!("parse {}", op.display()))?;
                for rule in file.overrides {
                    overrides.insert(rule.host.clone(), rule.into_learned());
                }
            }
        }

        Ok(Self {
            learned_path,
            inner: Arc::new(RwLock::new(learned)),
            overrides: Arc::new(overrides),
        })
    }

    /// Look up the active rule for a host. Overrides win over learned entries
    /// and are always treated as non-stale.
    pub async fn get(&self, host: &str) -> Option<LearnedRule> {
        if let Some(r) = self.overrides.get(host) {
            return Some(r.clone());
        }
        let map = self.inner.read().await;
        map.get(host).cloned()
    }

    /// Store (or replace) a learned rule for a host. Persists atomically.
    pub async fn upsert(&self, rule: LearnedRule) -> Result<()> {
        {
            let mut map = self.inner.write().await;
            map.insert(rule.host.clone(), rule);
        }
        self.persist().await
    }

    /// Mark a host's learned rule stale. No-op if missing.
    pub async fn mark_stale(&self, host: &str, reason: &str) -> Result<()> {
        {
            let mut map = self.inner.write().await;
            if let Some(rule) = map.get_mut(host) {
                if rule.stale {
                    return Ok(());
                }
                rule.stale = true;
                tracing::info!(host = %host, reason = %reason, "marking learned rule stale");
            } else {
                return Ok(());
            }
        }
        self.persist().await
    }

    async fn persist(&self) -> Result<()> {
        let map = self.inner.read().await;
        let body = serde_json::to_string_pretty(&*map)?;
        let tmp = self.learned_path.with_extension("json.tmp");
        fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
        fs::rename(&tmp, &self.learned_path).with_context(|| {
            format!(
                "rename {} -> {}",
                tmp.display(),
                self.learned_path.display()
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(host: &str) -> LearnedRule {
        LearnedRule {
            host: host.to_string(),
            version: "v1".to_string(),
            content_selectors: vec!["article".to_string()],
            remove_selectors: vec![],
            title_selector: None,
            start_markers: vec![],
            end_markers: vec![],
            confidence: 0.8,
            reasoning: "test".to_string(),
            learned_from_sample_count: 3,
            stale: false,
        }
    }

    #[tokio::test]
    async fn upsert_and_get_roundtrips() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths {
            repo_root: temp.path().to_path_buf(),
            runtime_dir: temp.path().join("runtime"),
        };
        std::fs::create_dir_all(paths.article_memory_dir()).unwrap();
        let store = LearnedRuleStore::load(&paths, None).unwrap();
        store.upsert(rule("example.com")).await.unwrap();
        let got = store.get("example.com").await.unwrap();
        assert_eq!(got.host, "example.com");
    }

    #[tokio::test]
    async fn override_wins_over_learned() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths {
            repo_root: temp.path().to_path_buf(),
            runtime_dir: temp.path().join("runtime"),
        };
        std::fs::create_dir_all(paths.article_memory_dir()).unwrap();
        let overrides_path = temp.path().join("overrides.toml");
        std::fs::write(
            &overrides_path,
            r#"[[overrides]]
host = "example.com"
content_selectors = [".hand-written"]
"#,
        )
        .unwrap();
        let store = LearnedRuleStore::load(&paths, Some(&overrides_path)).unwrap();
        store.upsert(rule("example.com")).await.unwrap();
        let got = store.get("example.com").await.unwrap();
        assert_eq!(got.content_selectors, vec![".hand-written".to_string()]);
        assert_eq!(got.version, "override");
    }

    #[tokio::test]
    async fn mark_stale_sets_flag() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths {
            repo_root: temp.path().to_path_buf(),
            runtime_dir: temp.path().join("runtime"),
        };
        std::fs::create_dir_all(paths.article_memory_dir()).unwrap();
        let store = LearnedRuleStore::load(&paths, None).unwrap();
        store.upsert(rule("example.com")).await.unwrap();
        store.mark_stale("example.com", "test").await.unwrap();
        let got = store.get("example.com").await.unwrap();
        assert!(got.stale);
    }

    #[tokio::test]
    async fn persist_survives_reload() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths {
            repo_root: temp.path().to_path_buf(),
            runtime_dir: temp.path().join("runtime"),
        };
        std::fs::create_dir_all(paths.article_memory_dir()).unwrap();
        {
            let s1 = LearnedRuleStore::load(&paths, None).unwrap();
            s1.upsert(rule("example.com")).await.unwrap();
        }
        let s2 = LearnedRuleStore::load(&paths, None).unwrap();
        let got = s2.get("example.com").await.unwrap();
        assert_eq!(got.host, "example.com");
    }
}
