use super::Github;
use anyhow::{bail, ensure, Result};
use reqwest::{header, Method, StatusCode};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

const MAX_CACHE_ENTRIES: usize = 512;
const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 1024 * 1024;
const MAX_CREDENTIALS: usize = 128;
const IDENTITY_TTL_SECONDS: i64 = 300;
const CORE_RESERVE: u64 = 50;

#[derive(Clone, Eq, Hash, PartialEq)]
struct Credential {
    base: String,
    digest: [u8; 32],
}
#[derive(Clone, Eq, Hash, PartialEq)]
struct CacheKey {
    credential: Credential,
    path: String,
}
#[derive(Clone)]
struct CachedJson {
    value: Value,
    etag: Option<header::HeaderValue>,
    modified: Option<header::HeaderValue>,
    bytes: usize,
}
struct Identity {
    value: Value,
    expires: i64,
}
type IdentitySlot = Arc<tokio::sync::Mutex<Option<Identity>>>;

#[derive(Clone, Copy)]
enum Resource {
    Core,
    Search,
}
impl Resource {
    fn for_path(path: &str) -> Self {
        if path.starts_with("/search/") {
            Self::Search
        } else {
            Self::Core
        }
    }
    fn from_headers(headers: &header::HeaderMap, path: &str) -> Option<Self> {
        match headers
            .get("x-ratelimit-resource")
            .and_then(|value| value.to_str().ok())
        {
            Some("core") => Some(Self::Core),
            Some("search") => Some(Self::Search),
            Some(_) => None,
            None => Some(Self::for_path(path)),
        }
    }
}
#[derive(Clone, Copy)]
struct Budget {
    remaining: u64,
    reset: i64,
}
#[derive(Default)]
struct RateState {
    core: Option<Budget>,
    search: Option<Budget>,
    backoff_until: i64,
}
impl RateState {
    fn budget(&self, resource: Resource) -> Option<Budget> {
        match resource {
            Resource::Core => self.core,
            Resource::Search => self.search,
        }
    }
    fn check(&self, resource: Resource, polling: bool, now: i64) -> Result<()> {
        ensure!(
            self.backoff_until <= now,
            "GitHub API backoff until {}",
            self.backoff_until
        );
        if let Some(budget) = self.budget(resource) {
            let reserve =
                polling && matches!(resource, Resource::Core) && budget.remaining <= CORE_RESERVE;
            ensure!(
                budget.reset <= now || (budget.remaining > 0 && !reserve),
                "GitHub API rate limit; reset {}",
                budget.reset
            );
        }
        Ok(())
    }
    fn observe(&mut self, headers: &header::HeaderMap, status: StatusCode, path: &str, now: i64) {
        if let (Some(resource), Some(remaining)) = (
            Resource::from_headers(headers, path),
            number(headers, "x-ratelimit-remaining"),
        ) {
            let reset = number(headers, "x-ratelimit-reset")
                .and_then(|value| i64::try_from(value).ok())
                .unwrap_or(now + 60);
            let slot = match resource {
                Resource::Core => &mut self.core,
                Resource::Search => &mut self.search,
            };
            let remaining = slot
                .filter(|old| old.reset == reset)
                .map_or(remaining, |old| old.remaining.min(remaining));
            *slot = Some(Budget { remaining, reset });
        }
        if let Some(delay) = retry_after(headers, now) {
            self.backoff_until = self.backoff_until.max(now.saturating_add(delay));
        } else if status == StatusCode::TOO_MANY_REQUESTS {
            self.backoff_until = self.backoff_until.max(now + 60);
        }
    }
}

#[derive(Default)]
pub(crate) struct HttpState {
    cache: HashMap<CacheKey, CachedJson>,
    cache_order: VecDeque<CacheKey>,
    cache_bytes: usize,
    identities: HashMap<Credential, IdentitySlot>,
    rates: HashMap<Credential, RateState>,
}
impl HttpState {
    fn rate(&mut self, credential: &Credential) -> &mut RateState {
        if !self.rates.contains_key(credential) && self.rates.len() >= MAX_CREDENTIALS {
            if let Some(old) = self.rates.keys().next().cloned() {
                self.rates.remove(&old);
            }
        }
        self.rates.entry(credential.clone()).or_default()
    }
    fn remove_cached(&mut self, key: &CacheKey) {
        if let Some(old) = self.cache.remove(key) {
            self.cache_bytes -= old.bytes;
        }
        self.cache_order.retain(|old| old != key);
    }
    fn cache(&mut self, key: CacheKey, entry: Option<CachedJson>) {
        self.remove_cached(&key);
        let Some(entry) = entry.filter(|entry| entry.bytes <= MAX_ENTRY_BYTES) else {
            return;
        };
        while self.cache.len() >= MAX_CACHE_ENTRIES
            || self.cache_bytes + entry.bytes > MAX_CACHE_BYTES
        {
            let Some(old) = self.cache_order.pop_front() else {
                break;
            };
            if let Some(value) = self.cache.remove(&old) {
                self.cache_bytes -= value.bytes;
            }
        }
        self.cache_bytes += entry.bytes;
        self.cache_order.push_back(key.clone());
        self.cache.insert(key, entry);
    }
}

impl Github {
    fn credential(&self) -> Credential {
        Credential {
            base: self.base.clone(),
            digest: Sha256::digest(self.token.as_bytes()).into(),
        }
    }
    pub(super) async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<reqwest::Response> {
        let response = self.send_conditional(method, path, body, None).await?;
        if response.status() == StatusCode::FORBIDDEN
            || response.status() == StatusCode::TOO_MANY_REQUESTS
        {
            return Err(self.error_response(response, path).await);
        }
        Ok(response)
    }
    async fn send_conditional(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        cached: Option<&CachedJson>,
    ) -> Result<reqwest::Response> {
        ensure!(
            path.starts_with('/') && !path.starts_with("//"),
            "Invalid API path"
        );
        let credential = self.credential();
        self.http
            .lock()
            .map_err(|_| anyhow::anyhow!("GitHub HTTP state unavailable"))?
            .rate(&credential)
            .check(Resource::for_path(path), self.polling, now())?;
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(cached) = cached {
            if let Some(etag) = &cached.etag {
                request = request.header(header::IF_NONE_MATCH, etag);
            } else if let Some(modified) = &cached.modified {
                request = request.header(header::IF_MODIFIED_SINCE, modified);
            }
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(reqwest::Error::without_url)?;
        let mut state = self
            .http
            .lock()
            .map_err(|_| anyhow::anyhow!("GitHub HTTP state unavailable"))?;
        let rate = state.rate(&credential);
        rate.observe(response.headers(), response.status(), path, now());
        if response.status() == StatusCode::NOT_MODIFIED {
            rate.check(Resource::for_path(path), self.polling, now())?;
        }
        Ok(response)
    }
    pub async fn request(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let conditional = self.polling && method == Method::GET;
        let key = CacheKey {
            credential: self.credential(),
            path: path.into(),
        };
        let cached = if conditional {
            self.http
                .lock()
                .map_err(|_| anyhow::anyhow!("GitHub HTTP state unavailable"))?
                .cache
                .get(&key)
                .cloned()
        } else {
            None
        };
        let response = self
            .send_conditional(method, path, body, cached.as_ref())
            .await?;
        let status = response.status();
        let headers = response.headers().clone();
        if status == StatusCode::NOT_MODIFIED {
            let Some(mut cached) = cached else {
                bail!(
                    "GitHub API 304 for {}",
                    path.split('?').next().unwrap_or(path)
                );
            };
            if let Some(etag) = headers.get(header::ETAG) {
                cached.etag = Some(etag.clone());
            }
            if let Some(modified) = headers.get(header::LAST_MODIFIED) {
                cached.modified = Some(modified.clone());
            }
            let value = cached.value.clone();
            self.http
                .lock()
                .map_err(|_| anyhow::anyhow!("GitHub HTTP state unavailable"))?
                .cache(key, Some(cached));
            return Ok(value);
        }
        if !status.is_success() {
            return Err(self.error_response(response, path).await);
        }
        let value = if status == StatusCode::NO_CONTENT {
            Value::Null
        } else {
            response
                .json::<Value>()
                .await
                .map_err(reqwest::Error::without_url)?
        };
        if conditional {
            let etag = headers.get(header::ETAG).cloned();
            let modified = headers.get(header::LAST_MODIFIED).cloned();
            let entry = if etag.is_some() || modified.is_some() {
                Some(CachedJson {
                    bytes: serde_json::to_vec(&value)?.len(),
                    value: value.clone(),
                    etag,
                    modified,
                })
            } else {
                None
            };
            self.http
                .lock()
                .map_err(|_| anyhow::anyhow!("GitHub HTTP state unavailable"))?
                .cache(key, entry);
        }
        Ok(value)
    }
    async fn error_response(&self, response: reqwest::Response, path: &str) -> anyhow::Error {
        let status = response.status();
        let headers = response.headers().clone();
        let primary_reset = (number(&headers, "x-ratelimit-remaining") == Some(0))
            .then(|| number(&headers, "x-ratelimit-reset"))
            .flatten();
        if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
            let error = response.json::<Value>().await.unwrap_or(Value::Null);
            let message = error["message"].as_str().unwrap_or("").to_ascii_lowercase();
            if message.contains("secondary rate") || message.contains("abuse detection") {
                let until = now().saturating_add(retry_after(&headers, now()).unwrap_or(60));
                let Ok(mut state) = self.http.lock() else {
                    return anyhow::anyhow!("GitHub HTTP state unavailable");
                };
                let rate = state.rate(&self.credential());
                rate.backoff_until = rate.backoff_until.max(until);
            }
        }
        let path = path.split('?').next().unwrap_or(path);
        if let Some(reset) = primary_reset {
            anyhow::anyhow!("GitHub API {} for {}; reset {reset}", status.as_u16(), path)
        } else {
            anyhow::anyhow!("GitHub API {} for {}", status.as_u16(), path)
        }
    }
    pub(super) async fn verify_poll_actor(&self, actor: &str) -> Result<Value> {
        if !self.polling {
            return self.verify_actor(actor).await;
        }
        let key = self.credential();
        let slot = {
            let mut state = self
                .http
                .lock()
                .map_err(|_| anyhow::anyhow!("GitHub HTTP state unavailable"))?;
            if !state.identities.contains_key(&key) && state.identities.len() >= MAX_CREDENTIALS {
                if let Some(old) = state.identities.keys().next().cloned() {
                    state.identities.remove(&old);
                }
            }
            state.identities.entry(key).or_default().clone()
        };
        // Serialize only identity verification for this credential; never hold the state lock across I/O.
        let mut identity = slot.lock().await;
        if identity
            .as_ref()
            .is_none_or(|identity| identity.expires <= now())
        {
            let value = self.request(Method::GET, "/user", None).await?;
            *identity = Some(Identity {
                value,
                expires: now() + IDENTITY_TTL_SECONDS,
            });
        }
        let value = &identity.as_ref().unwrap().value;
        ensure!(
            value["login"]
                .as_str()
                .is_some_and(|login| login.eq_ignore_ascii_case(actor)),
            "API token account does not match actor {actor}"
        );
        Ok(value.clone())
    }
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn number(headers: &header::HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.parse().ok()
}
fn retry_after(headers: &header::HeaderMap, now: i64) -> Option<i64> {
    let value = headers.get(header::RETRY_AFTER)?.to_str().ok()?;
    value
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= 0)
        .map(|value| value.max(1))
        .or_else(|| {
            chrono::DateTime::parse_from_rfc2822(value)
                .ok()
                .map(|date| date.timestamp().saturating_sub(now).max(1))
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    type Reply = (u16, Vec<(String, String)>, Value);
    async fn server(handler: impl Fn(&str) -> Reply + Send + Sync + 'static) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handler = Arc::new(handler);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let handler = handler.clone();
                tokio::spawn(async move {
                    let mut bytes = vec![0; 16384];
                    let n = socket.read(&mut bytes).await.unwrap();
                    let request = String::from_utf8_lossy(&bytes[..n]);
                    let (status, headers, body) = handler(&request);
                    let body = if status == 304 {
                        String::new()
                    } else {
                        body.to_string()
                    };
                    let headers = headers
                        .into_iter()
                        .map(|(name, value)| format!("{name}: {value}\r\n"))
                        .collect::<String>();
                    let response = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n{body}",body.len());
                    socket.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });
        format!("http://{address}")
    }
    fn polling(base: &str) -> Github {
        let mut api = Github::with_base("first-secret", base).unwrap();
        api.polling = true;
        api
    }
    #[tokio::test]
    async fn conditional_polling_updates_payload_and_reuses_matching_304() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let base = server(move |request| {
            let mut seen = seen.lock().unwrap();
            seen.push(request.to_lowercase());
            match seen.len() {
                1 => (
                    200,
                    vec![("ETag".into(), "\"one\"".into())],
                    json!({"version":1}),
                ),
                2 => (304, vec![], Value::Null),
                3 => (
                    200,
                    vec![("ETag".into(), "\"two\"".into())],
                    json!({"version":2}),
                ),
                _ => (304, vec![], Value::Null),
            }
        })
        .await;
        let api = polling(&base);
        for version in [1, 1, 2, 2] {
            assert_eq!(
                api.request(Method::GET, "/items", None).await.unwrap()["version"],
                version
            );
        }
        let requests = requests.lock().unwrap();
        assert!(!requests[0].contains("if-none-match"));
        assert!(requests[1].contains("if-none-match: \"one\""));
        assert!(requests[3].contains("if-none-match: \"two\""));
    }
    #[tokio::test]
    async fn identity_cache_checks_expected_actor_without_repeating_user_request() {
        let count = Arc::new(Mutex::new(0));
        let seen = count.clone();
        let base = server(move |_| {
            *seen.lock().unwrap() += 1;
            (200, vec![], json!({"login":"alice"}))
        })
        .await;
        let api = polling(&base);
        api.verify_poll_actor("alice").await.unwrap();
        api.verify_poll_actor("ALICE").await.unwrap();
        assert!(api.verify_poll_actor("bob").await.is_err());
        assert_eq!(*count.lock().unwrap(), 1);
    }
    #[tokio::test]
    async fn last_modified_fallback_and_missing_validators_use_fresh_reads() {
        let count = Arc::new(Mutex::new(0));
        let seen = count.clone();
        let base = server(move |request| {
            let mut count = seen.lock().unwrap();
            *count += 1;
            match *count {
                1 => (
                    200,
                    vec![(
                        "Last-Modified".into(),
                        "Wed, 21 Oct 2015 07:28:00 GMT".into(),
                    )],
                    json!(1),
                ),
                2 => {
                    assert!(request
                        .to_lowercase()
                        .contains("if-modified-since: wed, 21 oct 2015 07:28:00 gmt"));
                    (200, vec![], json!(2))
                }
                _ => {
                    assert!(!request.to_lowercase().contains("if-modified-since"));
                    (200, vec![], json!(3))
                }
            }
        })
        .await;
        let api = polling(&base);
        for expected in 1..=3 {
            assert_eq!(
                api.request(Method::GET, "/items", None).await.unwrap(),
                json!(expected)
            );
        }
    }
    #[tokio::test]
    async fn rotated_credentials_and_action_reads_never_reuse_polling_cache() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let base = server(move |request| {
            seen.lock().unwrap().push(request.to_lowercase());
            (
                200,
                vec![("ETag".into(), "old".into())],
                json!({"login":"alice"}),
            )
        })
        .await;
        let api = polling(&base);
        api.request(Method::GET, "/items", None).await.unwrap();
        let mut other = Github::with_base("second-secret", &base).unwrap();
        other.polling = true;
        other.http = api.http.clone();
        other.request(Method::GET, "/items", None).await.unwrap();
        let mut action = Github::with_base("first-secret", &base).unwrap();
        action.http = api.http.clone();
        action.request(Method::GET, "/items", None).await.unwrap();
        api.request(Method::POST, "/items", Some(&json!({})))
            .await
            .unwrap();
        let requests = requests.lock().unwrap();
        assert!(requests
            .iter()
            .all(|request| !request.contains("if-none-match")));
    }
    #[tokio::test]
    async fn core_reserve_does_not_block_search_or_worker_actions() {
        let base = server(|request| {
            let search = request.starts_with("GET /search/");
            (
                200,
                vec![
                    (
                        "X-RateLimit-Resource".into(),
                        if search { "search" } else { "core" }.into(),
                    ),
                    (
                        "X-RateLimit-Remaining".into(),
                        if search { "1" } else { "49" }.into(),
                    ),
                    (
                        "X-RateLimit-Reset".into(),
                        (chrono::Utc::now().timestamp() + 300).to_string(),
                    ),
                ],
                json!([]),
            )
        })
        .await;
        let api = polling(&base);
        api.request(Method::GET, "/items", None).await.unwrap();
        assert!(api
            .request(Method::GET, "/items", None)
            .await
            .unwrap_err()
            .to_string()
            .contains("reset"));
        api.request(Method::GET, "/search/issues?q=one", None)
            .await
            .unwrap();
        let mut action = Github::with_base("first-secret", &base).unwrap();
        action.http = api.http.clone();
        action.request(Method::GET, "/items", None).await.unwrap();
    }
    #[tokio::test]
    async fn exhausted_304_never_returns_stale_cached_success() {
        let count = Arc::new(Mutex::new(0));
        let seen = count.clone();
        let base = server(move |_| {
            let mut count = seen.lock().unwrap();
            *count += 1;
            if *count == 1 {
                (200, vec![("ETag".into(), "old".into())], json!([1]))
            } else {
                (
                    304,
                    vec![
                        ("X-RateLimit-Remaining".into(), "0".into()),
                        (
                            "X-RateLimit-Reset".into(),
                            (chrono::Utc::now().timestamp() + 300).to_string(),
                        ),
                    ],
                    Value::Null,
                )
            }
        })
        .await;
        let api = polling(&base);
        api.request(Method::GET, "/items", None).await.unwrap();
        assert!(api
            .request(Method::GET, "/items", None)
            .await
            .unwrap_err()
            .to_string()
            .contains("reset"));
        assert!(api.request(Method::GET, "/items", None).await.is_err());
        assert_eq!(*count.lock().unwrap(), 2);
    }
    #[tokio::test]
    async fn secondary_limit_pauses_all_resources_for_only_matching_credential() {
        let count = Arc::new(Mutex::new(0));
        let seen = count.clone();
        let base = server(move |_| {
            *seen.lock().unwrap() += 1;
            (
                403,
                vec![],
                json!({"message":"You have exceeded a secondary rate limit."}),
            )
        })
        .await;
        let api = polling(&base);
        assert!(api
            .request(Method::GET, "/items?secret=value", None)
            .await
            .is_err());
        let error = api
            .request(Method::GET, "/search/issues?q=value", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("backoff"));
        assert!(!error.contains("first-secret"));
        assert!(!error.contains("q=value"));
        let mut other = Github::with_base("second-secret", &base).unwrap();
        other.http = api.http.clone();
        assert!(other.request(Method::GET, "/items", None).await.is_err());
        assert_eq!(*count.lock().unwrap(), 2);
    }
    #[tokio::test]
    async fn retry_after_429_pauses_even_uncached_actions() {
        let count = Arc::new(Mutex::new(0));
        let seen = count.clone();
        let base = server(move |_| {
            *seen.lock().unwrap() += 1;
            (429, vec![("Retry-After".into(), "120".into())], json!({}))
        })
        .await;
        let api = Github::with_base("first-secret", &base).unwrap();
        assert!(api
            .request(Method::POST, "/items", Some(&json!({})))
            .await
            .is_err());
        assert!(api
            .request(Method::GET, "/search/issues", None)
            .await
            .unwrap_err()
            .to_string()
            .contains("backoff"));
        assert_eq!(*count.lock().unwrap(), 1);
    }
    #[tokio::test]
    async fn uncached_304_preserves_sanitized_api_error_format() {
        let base = server(|_| (304, vec![], Value::Null)).await;
        let api = Github::with_base("secret", &base).unwrap();
        assert_eq!(
            api.request(Method::GET, "/items?private=value", None)
                .await
                .unwrap_err()
                .to_string(),
            "GitHub API 304 for /items"
        );
    }
    #[tokio::test]
    async fn not_modified_response_refreshes_validator_metadata() {
        let count = Arc::new(Mutex::new(0));
        let seen = count.clone();
        let base = server(move |request| {
            let mut count = seen.lock().unwrap();
            *count += 1;
            match *count {
                1 => (200, vec![("ETag".into(), "old".into())], json!([1])),
                2 => (304, vec![("ETag".into(), "new".into())], Value::Null),
                _ => {
                    assert!(request.to_lowercase().contains("if-none-match: new"));
                    (304, vec![], Value::Null)
                }
            }
        })
        .await;
        let api = polling(&base);
        for _ in 0..3 {
            assert_eq!(
                api.request(Method::GET, "/items", None).await.unwrap(),
                json!([1])
            );
        }
    }
    #[test]
    fn rate_budgets_and_backoff_expire_at_reset_without_sleeping() {
        let mut state = RateState {
            core: Some(Budget {
                remaining: 0,
                reset: 120,
            }),
            search: Some(Budget {
                remaining: 1,
                reset: 120,
            }),
            backoff_until: 0,
        };
        assert!(state.check(Resource::Core, false, 119).is_err());
        assert!(state.check(Resource::Search, true, 119).is_ok());
        assert!(state.check(Resource::Core, true, 120).is_ok());
        state.backoff_until = 125;
        assert!(state.check(Resource::Search, false, 124).is_err());
        assert!(state.check(Resource::Search, false, 125).is_ok());
    }
    #[test]
    fn cache_evicts_old_entries_and_rejects_oversized_payloads() {
        let credential = Credential {
            base: "fixture".into(),
            digest: [0; 32],
        };
        let mut state = HttpState::default();
        for index in 0..=MAX_CACHE_ENTRIES {
            state.cache(
                CacheKey {
                    credential: credential.clone(),
                    path: index.to_string(),
                },
                Some(CachedJson {
                    value: json!(index),
                    etag: Some(header::HeaderValue::from_static("v1")),
                    modified: None,
                    bytes: 8,
                }),
            );
        }
        assert_eq!(state.cache.len(), MAX_CACHE_ENTRIES);
        assert_eq!(state.cache_bytes, 8 * MAX_CACHE_ENTRIES);
        assert!(!state.cache.contains_key(&CacheKey {
            credential: credential.clone(),
            path: "0".into()
        }));
        state.cache(
            CacheKey {
                credential,
                path: "huge".into(),
            },
            Some(CachedJson {
                value: Value::Null,
                etag: Some(header::HeaderValue::from_static("v1")),
                modified: None,
                bytes: MAX_ENTRY_BYTES + 1,
            }),
        );
        assert_eq!(state.cache.len(), MAX_CACHE_ENTRIES);
        assert!(state.cache_bytes <= MAX_CACHE_BYTES);
    }
    #[tokio::test]
    async fn raw_send_secondary_limit_blocks_following_poll_without_network() {
        let count = Arc::new(Mutex::new(0));
        let seen = count.clone();
        let base = server(move |_| {
            *seen.lock().unwrap() += 1;
            (
                403,
                vec![],
                json!({"message":"You have exceeded a secondary rate limit."}),
            )
        })
        .await;
        let api = Github::with_base("secret", &base).unwrap();
        let error = api
            .send(
                Method::GET,
                "/repos/acme/repo/git/ref/heads/topic?private=value",
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "GitHub API 403 for /repos/acme/repo/git/ref/heads/topic"
        );
        let mut poll = polling(&base);
        poll.token = "secret".into();
        poll.http = api.http.clone();
        assert!(poll
            .request(Method::GET, "/search/issues", None)
            .await
            .unwrap_err()
            .to_string()
            .contains("backoff"));
        assert_eq!(*count.lock().unwrap(), 1);
    }
    #[tokio::test]
    async fn raw_permission_error_keeps_following_reads_available_and_404_response_intact() {
        let count = Arc::new(Mutex::new(0));
        let seen = count.clone();
        let base = server(move |_| {
            let mut count = seen.lock().unwrap();
            *count += 1;
            match *count {
                1 => (
                    403,
                    vec![],
                    json!({"message":"Resource not accessible by personal access token"}),
                ),
                2 => (200, vec![], json!([])),
                _ => (404, vec![], json!({"message":"Not Found"})),
            }
        })
        .await;
        let api = Github::with_base("secret", &base).unwrap();
        assert_eq!(
            api.send(Method::GET, "/items", None)
                .await
                .unwrap_err()
                .to_string(),
            "GitHub API 403 for /items"
        );
        assert_eq!(
            api.request(Method::GET, "/items", None).await.unwrap(),
            json!([])
        );
        let missing = api.send(Method::GET, "/missing", None).await.unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            missing.json::<Value>().await.unwrap()["message"],
            "Not Found"
        );
        assert_eq!(*count.lock().unwrap(), 3);
    }
    #[tokio::test]
    async fn raw_primary_limit_includes_reset_and_raw_429_sets_backoff() {
        for status in [403, 429] {
            let count = Arc::new(Mutex::new(0));
            let seen = count.clone();
            let reset = now() + 300;
            let base = server(move |_| {
                *seen.lock().unwrap() += 1;
                let headers = if status == 403 {
                    vec![
                        ("X-RateLimit-Remaining".into(), "0".into()),
                        ("X-RateLimit-Reset".into(), reset.to_string()),
                    ]
                } else {
                    vec![("Retry-After".into(), "120".into())]
                };
                (
                    status,
                    headers,
                    json!({"message":"API rate limit exceeded"}),
                )
            })
            .await;
            let api = Github::with_base("secret", &base).unwrap();
            let error = api
                .send(Method::GET, "/items?private=value", None)
                .await
                .unwrap_err()
                .to_string();
            assert!(error.starts_with(&format!("GitHub API {status} for /items")));
            assert!(!error.contains("private"));
            if status == 403 {
                assert!(error.contains(&format!("reset {reset}")));
            }
            assert!(api.request(Method::GET, "/items", None).await.is_err());
            assert_eq!(*count.lock().unwrap(), 1);
        }
    }
}
