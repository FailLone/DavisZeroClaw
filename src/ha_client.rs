use crate::{HaState, USER_AGENT};
use reqwest::{Client, Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::net::{IpAddr, ToSocketAddrs};

#[derive(Debug, Clone)]
pub enum ProxyError {
    MissingCredentials,
    AuthFailed,
    Unreachable,
    Invalid(String),
}

#[derive(Clone)]
pub struct HaClient {
    client: Client,
    origin: String,
    token: String,
}

impl HaClient {
    pub fn from_env() -> std::result::Result<Self, ProxyError> {
        let ha_url = std::env::var("DAVIS_HA_URL").map_err(|_| ProxyError::MissingCredentials)?;
        let token = std::env::var("DAVIS_HA_TOKEN").map_err(|_| ProxyError::MissingCredentials)?;
        Self::from_credentials(&ha_url, &token)
    }

    pub fn from_credentials(ha_url: &str, token: &str) -> std::result::Result<Self, ProxyError> {
        let origin = derive_ha_origin(ha_url).map_err(ProxyError::Invalid)?;
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(|err| ProxyError::Invalid(err.to_string()))?;
        Ok(Self {
            client,
            origin,
            token: token.to_string(),
        })
    }

    #[tracing::instrument(
        name = "ha_rest",
        skip(self, payload),
        fields(origin = %self.origin, method = %method, path = %path, status = tracing::field::Empty),
    )]
    async fn request_value(
        &self,
        method: Method,
        path: &str,
        payload: Option<Value>,
    ) -> std::result::Result<Value, ProxyError> {
        let mut request = self
            .client
            .request(method, format!("{}{}", self.origin, path))
            .bearer_auth(&self.token)
            .header("Accept", "application/json");
        if let Some(body) = payload {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|err| {
            tracing::warn!(error = %err, "HA REST request failed to reach server");
            ProxyError::Unreachable
        })?;
        let status = response.status();
        tracing::Span::current().record("status", status.as_u16());
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            tracing::warn!("HA REST rejected credentials; check token rotation");
            return Err(ProxyError::AuthFailed);
        }
        if !status.is_success() {
            tracing::warn!(%status, "HA REST returned non-2xx");
            return Err(ProxyError::Unreachable);
        }
        let bytes = response.bytes().await.map_err(|err| {
            tracing::warn!(error = %err, "HA REST body read failed");
            ProxyError::Unreachable
        })?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|err| ProxyError::Invalid(err.to_string()))
    }

    pub async fn get_value(&self, path: &str) -> std::result::Result<Value, ProxyError> {
        self.request_value(Method::GET, path, None).await
    }

    pub async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
    ) -> std::result::Result<T, ProxyError> {
        let value = self.get_value(path).await?;
        serde_json::from_value(value).map_err(|err| ProxyError::Invalid(err.to_string()))
    }

    pub async fn post_value(
        &self,
        path: &str,
        payload: Value,
    ) -> std::result::Result<Value, ProxyError> {
        self.request_value(Method::POST, path, Some(payload)).await
    }

    pub async fn post_json<T: DeserializeOwned>(
        &self,
        path: &str,
        payload: Value,
    ) -> std::result::Result<T, ProxyError> {
        let value = self.post_value(path, payload).await?;
        serde_json::from_value(value).map_err(|err| ProxyError::Invalid(err.to_string()))
    }

    #[cfg(test)]
    pub(crate) fn from_parts(client: Client, origin: String, token: String) -> Self {
        Self {
            client,
            origin,
            token,
        }
    }
}

pub fn normalize_ha_url(ha_url: &str) -> std::result::Result<String, String> {
    let mut parsed =
        url::Url::parse(ha_url).map_err(|_| "home_assistant.url 不是合法 URL".to_string())?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "home_assistant.url 不是合法 URL".to_string())?;
    let port_number = parsed.port_or_known_default();
    if parsed.scheme() == "http" && host.ends_with(".local") {
        if let Some(ipv4_host) = prefer_ipv4_host(host, port_number) {
            parsed
                .set_host(Some(&ipv4_host))
                .map_err(|_| "home_assistant.url 不是合法 URL".to_string())?;
        }
    }
    Ok(parsed.to_string())
}

pub fn derive_ha_origin(ha_url: &str) -> std::result::Result<String, String> {
    let normalized = normalize_ha_url(ha_url)?;
    let parsed =
        url::Url::parse(&normalized).map_err(|_| "home_assistant.url 不是合法 URL".to_string())?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "home_assistant.url 不是合法 URL".to_string())?;
    let port = parsed
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    Ok(format!("{}://{}{}", parsed.scheme(), host, port))
}

fn prefer_ipv4_host(host: &str, port: Option<u16>) -> Option<String> {
    let port = port?;
    let addrs = (host, port).to_socket_addrs().ok()?;
    addrs.into_iter().find_map(|addr| match addr.ip() {
        IpAddr::V4(ip) => Some(ip.to_string()),
        IpAddr::V6(_) => None,
    })
}

pub async fn fetch_all_states(client: &HaClient) -> std::result::Result<Vec<Value>, ProxyError> {
    match client.get_value("/api/states").await? {
        Value::Array(items) => Ok(items),
        _ => Ok(Vec::new()),
    }
}

pub async fn fetch_all_states_typed(
    client: &HaClient,
) -> std::result::Result<Vec<HaState>, ProxyError> {
    client.get_json("/api/states").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefer_ipv4_host_resolves_localhost() {
        let resolved = prefer_ipv4_host("localhost", Some(8123));
        assert_eq!(resolved.as_deref(), Some("127.0.0.1"));
    }

    #[test]
    fn derive_ha_origin_keeps_non_local_host() {
        let origin = derive_ha_origin("https://example.com/api/mcp").unwrap();
        assert_eq!(origin, "https://example.com");
    }

    #[test]
    fn normalize_ha_url_keeps_path_for_non_local_host() {
        let normalized = normalize_ha_url("https://example.com/api/mcp").unwrap();
        assert_eq!(normalized, "https://example.com/api/mcp");
    }
}
