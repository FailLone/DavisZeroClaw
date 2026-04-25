//! Search provider abstraction. Brave is the only real impl in MVP;
//! mock is available via `#[cfg(test)]`.

pub mod mock;

use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub url: String,
    pub title: String,
    pub snippet: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("provider '{0}' unavailable: {1}")]
    Unavailable(&'static str, String),
    #[error("rate limited")]
    RateLimited,
    #[error("auth error: {0}")]
    Auth(String),
    #[error("other: {0}")]
    Other(#[from] anyhow::Error),
}

#[async_trait]
pub trait SearchProvider: Send + Sync {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, SearchError>;
}
