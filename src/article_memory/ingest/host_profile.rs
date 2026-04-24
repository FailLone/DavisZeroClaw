// Consumed starting Task 4 (types.rs) and Task 6 (queue.rs). Once all symbols
// are wired, remove this attribute — it's a staging allowance only.
#![allow(dead_code)]

use crate::app_config::ArticleMemoryIngestConfig;
use std::fmt;
use url::{Host, Url};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProfile {
    pub profile: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlValidationError {
    InvalidUrl,
    InvalidScheme,
    MissingHost,
    PrivateAddressBlocked(String),
}

impl fmt::Display for UrlValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl => write!(f, "url could not be parsed"),
            Self::InvalidScheme => write!(f, "only http and https schemes are allowed"),
            Self::MissingHost => write!(f, "url is missing a host"),
            Self::PrivateAddressBlocked(detail) => {
                write!(f, "private or loopback address blocked: {detail}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizeUrlError {
    InvalidUrl,
}

impl fmt::Display for NormalizeUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "url could not be parsed")
    }
}

pub fn resolve_profile(url: &str, config: &ArticleMemoryIngestConfig) -> ResolvedProfile {
    let host = match Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
    {
        Some(h) => h,
        None => {
            return ResolvedProfile {
                profile: config.default_profile.clone(),
                source: None,
            }
        }
    };
    for entry in &config.host_profiles {
        if host_matches_suffix(&host, &entry.match_suffix) {
            return ResolvedProfile {
                profile: entry.profile.clone(),
                source: entry.source.clone(),
            };
        }
    }
    ResolvedProfile {
        profile: config.default_profile.clone(),
        source: None,
    }
}

fn host_matches_suffix(host: &str, suffix: &str) -> bool {
    let s = suffix.to_ascii_lowercase();
    if s.is_empty() {
        return false;
    }
    host == s || host.ends_with(&format!(".{s}"))
}

pub fn normalize_url(url: &str) -> Result<String, NormalizeUrlError> {
    let mut parsed = Url::parse(url).map_err(|_| NormalizeUrlError::InvalidUrl)?;
    parsed.set_fragment(None);
    if let Some(host) = parsed.host_str() {
        let lowered = host.to_ascii_lowercase();
        let _ = parsed.set_host(Some(&lowered));
    }
    Ok(parsed.to_string())
}

pub fn validate_url_for_ingest(
    url: &str,
    config: &ArticleMemoryIngestConfig,
) -> Result<(), UrlValidationError> {
    let parsed = Url::parse(url).map_err(|_| UrlValidationError::InvalidUrl)?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err(UrlValidationError::InvalidScheme),
    }
    let host = parsed.host().ok_or(UrlValidationError::MissingHost)?;
    let host_string = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if config
        .allow_private_hosts
        .iter()
        .any(|h| h.eq_ignore_ascii_case(&host_string))
    {
        return Ok(());
    }
    match host {
        Host::Ipv4(ip) => {
            if ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.is_unspecified()
            {
                return Err(UrlValidationError::PrivateAddressBlocked(format!(
                    "{ip} is a private/loopback/link-local/broadcast/multicast/unspecified address"
                )));
            }
        }
        Host::Ipv6(ip) => {
            if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
                return Err(UrlValidationError::PrivateAddressBlocked(format!(
                    "{ip} is a loopback/unspecified/multicast IPv6 address"
                )));
            }
            let seg0 = ip.segments()[0];
            if (seg0 & 0xfe00) == 0xfc00 {
                return Err(UrlValidationError::PrivateAddressBlocked(format!(
                    "{ip} is a unique-local IPv6 address (fc00::/7)"
                )));
            }
            if (seg0 & 0xffc0) == 0xfe80 {
                return Err(UrlValidationError::PrivateAddressBlocked(format!(
                    "{ip} is a link-local IPv6 address (fe80::/10)"
                )));
            }
        }
        Host::Domain(name) => {
            let n = name.to_ascii_lowercase();
            if n == "localhost"
                || n.ends_with(".local")
                || n.ends_with(".internal")
                || n.ends_with(".localhost")
            {
                return Err(UrlValidationError::PrivateAddressBlocked(format!(
                    "{n} is a reserved private-use hostname"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::ArticleMemoryHostProfile;

    fn cfg_with(host_profiles: Vec<ArticleMemoryHostProfile>) -> ArticleMemoryIngestConfig {
        ArticleMemoryIngestConfig {
            host_profiles,
            ..Default::default()
        }
    }

    #[test]
    fn resolve_profile_matches_host_suffix() {
        let cfg = cfg_with(vec![ArticleMemoryHostProfile {
            match_suffix: "zhihu.com".into(),
            profile: "articles-zhihu".into(),
            source: Some("zhihu".into()),
        }]);
        assert_eq!(
            resolve_profile("https://zhuanlan.zhihu.com/p/1", &cfg).profile,
            "articles-zhihu"
        );
        assert_eq!(
            resolve_profile("https://www.zhihu.com/q/1", &cfg).profile,
            "articles-zhihu"
        );
        assert_eq!(
            resolve_profile("https://zhihu.com/", &cfg).profile,
            "articles-zhihu"
        );
        assert_eq!(
            resolve_profile("https://zhihu.com/", &cfg)
                .source
                .as_deref(),
            Some("zhihu")
        );
    }

    #[test]
    fn resolve_profile_rejects_unrelated_hosts() {
        let cfg = cfg_with(vec![ArticleMemoryHostProfile {
            match_suffix: "zhihu.com".into(),
            profile: "articles-zhihu".into(),
            source: None,
        }]);
        // zhihubus.com must NOT match zhihu.com suffix rule
        assert_eq!(
            resolve_profile("https://zhihubus.com/", &cfg).profile,
            "articles-generic"
        );
        assert_eq!(
            resolve_profile("https://fakezhihu.com/", &cfg).profile,
            "articles-generic"
        );
    }

    #[test]
    fn resolve_profile_first_hit_wins() {
        let cfg = cfg_with(vec![
            ArticleMemoryHostProfile {
                match_suffix: "zhihu.com".into(),
                profile: "articles-zhihu".into(),
                source: None,
            },
            ArticleMemoryHostProfile {
                match_suffix: "zhuanlan.zhihu.com".into(),
                profile: "articles-never".into(),
                source: None,
            },
        ]);
        assert_eq!(
            resolve_profile("https://zhuanlan.zhihu.com/p/1", &cfg).profile,
            "articles-zhihu"
        );
    }

    #[test]
    fn resolve_profile_empty_config_defaults() {
        let cfg = cfg_with(vec![]);
        assert_eq!(
            resolve_profile("https://x.com/", &cfg).profile,
            "articles-generic"
        );
    }

    #[test]
    fn resolve_profile_invalid_url_defaults() {
        let cfg = cfg_with(vec![]);
        assert_eq!(
            resolve_profile("not a url", &cfg).profile,
            "articles-generic"
        );
    }

    #[test]
    fn normalize_url_strips_fragment_and_lowercases_host() {
        let out = normalize_url("HTTPS://WWW.Zhihu.COM/p/1#section").unwrap();
        assert_eq!(out, "https://www.zhihu.com/p/1");
    }

    #[test]
    fn normalize_url_preserves_query_and_path_case() {
        let out = normalize_url("https://example.com/Path?Q=Value").unwrap();
        assert!(out.ends_with("/Path?Q=Value"));
    }

    #[test]
    fn validate_rejects_non_http_schemes() {
        let cfg = cfg_with(vec![]);
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,x",
            "ftp://x",
        ] {
            assert!(matches!(
                validate_url_for_ingest(url, &cfg),
                Err(UrlValidationError::InvalidScheme) | Err(UrlValidationError::InvalidUrl)
            ));
        }
    }

    #[test]
    fn validate_rejects_localhost_variants() {
        let cfg = cfg_with(vec![]);
        for url in [
            "http://127.0.0.1/",
            "http://localhost/",
            "http://[::1]/",
            "http://0.0.0.0/",
            "http://foo.local/",
            "http://bar.internal/",
        ] {
            match validate_url_for_ingest(url, &cfg) {
                Err(UrlValidationError::PrivateAddressBlocked(_)) => {}
                other => panic!("expected PrivateAddressBlocked for {url}, got {other:?}"),
            }
        }
    }

    #[test]
    fn validate_rejects_private_ipv4() {
        let cfg = cfg_with(vec![]);
        for url in [
            "http://10.0.0.1/",
            "http://172.16.0.1/",
            "http://172.31.255.1/",
            "http://192.168.1.1/",
            "http://169.254.169.254/",
        ] {
            assert!(matches!(
                validate_url_for_ingest(url, &cfg),
                Err(UrlValidationError::PrivateAddressBlocked(_))
            ));
        }
    }

    #[test]
    fn validate_rejects_ipv6_ula_and_link_local() {
        let cfg = cfg_with(vec![]);
        for url in [
            "http://[fc00::1]/",
            "http://[fd00::1]/",
            "http://[fe80::1]/",
        ] {
            assert!(matches!(
                validate_url_for_ingest(url, &cfg),
                Err(UrlValidationError::PrivateAddressBlocked(_))
            ));
        }
    }

    #[test]
    fn validate_allows_public_domain_and_public_ip() {
        let cfg = cfg_with(vec![]);
        assert!(validate_url_for_ingest("https://zhihu.com/", &cfg).is_ok());
        assert!(validate_url_for_ingest("https://1.1.1.1/", &cfg).is_ok());
        assert!(validate_url_for_ingest("https://[2001:4860:4860::8888]/", &cfg).is_ok());
    }

    #[test]
    fn validate_allowlist_bypasses_private_block() {
        let cfg = ArticleMemoryIngestConfig {
            allow_private_hosts: vec!["wiki.internal".into()],
            ..Default::default()
        };
        assert!(validate_url_for_ingest("http://wiki.internal/page", &cfg).is_ok());
    }
}
