//! Test-only deterministic search provider.

use super::{SearchError, SearchHit, SearchProvider};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct MockSearch {
    map: Mutex<HashMap<String, Vec<SearchHit>>>,
}

impl MockSearch {
    pub fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }

    pub fn inject(&self, query: &str, hits: Vec<SearchHit>) {
        self.map.lock().unwrap().insert(query.to_string(), hits);
    }
}

#[async_trait]
impl SearchProvider for MockSearch {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, SearchError> {
        let out = self
            .map
            .lock()
            .unwrap()
            .get(query)
            .cloned()
            .unwrap_or_default();
        Ok(out.into_iter().take(limit).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_returns_injected_hits_respecting_limit() {
        let m = MockSearch::new();
        m.inject(
            "rust",
            vec![
                SearchHit {
                    url: "a".into(),
                    title: "A".into(),
                    snippet: "".into(),
                },
                SearchHit {
                    url: "b".into(),
                    title: "B".into(),
                    snippet: "".into(),
                },
            ],
        );
        let got = m.search("rust", 1).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].url, "a");
    }
}
