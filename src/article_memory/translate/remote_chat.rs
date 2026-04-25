//! HTTP client for zeroclaw's /api/chat.
//!
//! Private to the translate module. Do NOT export. Do NOT reuse from other
//! workers. If a second non-hot-path consumer emerges, extract then — driven
//! by real second-consumer requirements, not speculation.
//! See docs/superpowers/plans/2026-04-25-topic-crawl-mvp.md §"Anchor decisions" A1/A3.

use crate::app_config::TranslateConfig;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub(super) enum RemoteChatError {
    #[error("zeroclaw unreachable: {0}")]
    Unreachable(String),
    #[error("budget exceeded (scope={scope}): {message}")]
    BudgetExceeded { scope: String, message: String },
    #[error("zeroclaw remote error: http {status}: {body}")]
    Remote { status: u16, body: String },
    #[error("zeroclaw response decode: {0}")]
    Decode(String),
    #[error("empty content")]
    Empty,
}

pub(super) struct RemoteChat {
    http: reqwest::Client,
    endpoint: String,
    api_key: Option<String>,
    budget_scope: String,
}

#[derive(Serialize)]
struct ChatReq<'a> {
    messages: Vec<ChatMsg<'a>>,
    hint: &'a str,
    classification: &'a str,
    budget_scope: &'a str,
    temperature: f32,
    max_tokens: usize,
}

#[derive(Serialize)]
struct ChatMsg<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResp {
    content: Option<String>,
}

#[derive(Deserialize)]
struct BudgetBody {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

impl RemoteChat {
    pub(super) fn new(cfg: &TranslateConfig, http: reqwest::Client) -> Self {
        let endpoint = format!("{}/api/chat", cfg.zeroclaw_base_url.trim_end_matches('/'));
        let api_key = cfg
            .api_key_env
            .as_deref()
            .and_then(|n| std::env::var(n).ok())
            .filter(|k| !k.is_empty());
        Self {
            http,
            endpoint,
            api_key,
            budget_scope: cfg.budget_scope.clone(),
        }
    }

    pub(super) async fn translate_to_zh(
        &self,
        system: &str,
        user: &str,
    ) -> Result<String, RemoteChatError> {
        let body = ChatReq {
            messages: vec![
                ChatMsg {
                    role: "system",
                    content: system,
                },
                ChatMsg {
                    role: "user",
                    content: user,
                },
            ],
            hint: "cheapest",
            classification: "translation",
            budget_scope: &self.budget_scope,
            temperature: 0.2,
            max_tokens: 4000,
        };
        let mut req = self
            .http
            .post(&self.endpoint)
            .json(&body)
            .timeout(Duration::from_secs(120));
        if let Some(k) = self.api_key.as_deref() {
            req = req.bearer_auth(k);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| RemoteChatError::Unreachable(e.to_string()))?;
        match resp.status().as_u16() {
            200 => {
                let parsed: ChatResp = resp
                    .json()
                    .await
                    .map_err(|e| RemoteChatError::Decode(e.to_string()))?;
                parsed
                    .content
                    .filter(|c| !c.trim().is_empty())
                    .ok_or(RemoteChatError::Empty)
            }
            402 => {
                let body: BudgetBody = resp.json().await.unwrap_or(BudgetBody {
                    scope: None,
                    message: None,
                });
                Err(RemoteChatError::BudgetExceeded {
                    scope: body.scope.unwrap_or_else(|| self.budget_scope.clone()),
                    message: body.message.unwrap_or_else(|| "budget exceeded".into()),
                })
            }
            status => {
                let text = resp.text().await.unwrap_or_default();
                Err(RemoteChatError::Remote { status, body: text })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    type MockState = Arc<Mutex<MockReply>>;

    #[derive(Clone, Default)]
    struct MockReply {
        status: u16,
        body: Value,
    }

    async fn handler(
        State(s): State<MockState>,
        Json(_req): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        let g = s.lock().unwrap().clone();
        (StatusCode::from_u16(g.status).unwrap(), Json(g.body))
    }

    async fn mock_server(reply: MockReply) -> (String, MockState) {
        let state = Arc::new(Mutex::new(reply));
        let app = Router::new()
            .route("/api/chat", post(handler))
            .with_state(state.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (format!("http://{addr}"), state)
    }

    fn cfg_with(base: &str) -> TranslateConfig {
        TranslateConfig {
            enabled: true,
            zeroclaw_base_url: base.into(),
            ..TranslateConfig::default()
        }
    }

    #[tokio::test]
    async fn success_200_returns_content() {
        let (base, _s) = mock_server(MockReply {
            status: 200,
            body: json!({"content": "hello 你好"}),
        })
        .await;
        let rc = RemoteChat::new(&cfg_with(&base), reqwest::Client::new());
        let got = rc.translate_to_zh("sys", "user").await.unwrap();
        assert_eq!(got, "hello 你好");
    }

    #[tokio::test]
    async fn status_402_returns_budget_exceeded() {
        let (base, _s) = mock_server(MockReply {
            status: 402,
            body: json!({"scope": "translation:monthly", "message": "over"}),
        })
        .await;
        let rc = RemoteChat::new(&cfg_with(&base), reqwest::Client::new());
        let err = rc.translate_to_zh("sys", "user").await.unwrap_err();
        assert!(
            matches!(err, RemoteChatError::BudgetExceeded { .. }),
            "{err}"
        );
    }

    #[tokio::test]
    async fn status_500_returns_remote() {
        let (base, _s) = mock_server(MockReply {
            status: 500,
            body: json!({"err":"boom"}),
        })
        .await;
        let rc = RemoteChat::new(&cfg_with(&base), reqwest::Client::new());
        let err = rc.translate_to_zh("sys", "user").await.unwrap_err();
        assert!(
            matches!(err, RemoteChatError::Remote { status: 500, .. }),
            "{err}"
        );
    }

    #[tokio::test]
    async fn unreachable_when_daemon_not_running() {
        let cfg = cfg_with("http://127.0.0.1:1"); // port 1 always refuses
        let rc = RemoteChat::new(&cfg, reqwest::Client::new());
        let err = rc.translate_to_zh("sys", "user").await.unwrap_err();
        assert!(matches!(err, RemoteChatError::Unreachable(_)), "{err}");
    }

    #[tokio::test]
    async fn empty_content_is_error() {
        let (base, _s) = mock_server(MockReply {
            status: 200,
            body: json!({"content": ""}),
        })
        .await;
        let rc = RemoteChat::new(&cfg_with(&base), reqwest::Client::new());
        let err = rc.translate_to_zh("sys", "user").await.unwrap_err();
        assert!(matches!(err, RemoteChatError::Empty), "{err}");
    }
}
