use crate::DeliveryFailure;

pub(crate) fn parse_webhook_url(raw: &str) -> Result<reqwest::Url, DeliveryFailure> {
    if raw.chars().any(char::is_control) {
        return Err(DeliveryFailure::Transport);
    }
    let value = raw.trim();
    let authority = value
        .split_once("://")
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("https"))
        .map(|(_, tail)| tail.split(['/', '?', '#']).next().unwrap_or(""))
        .ok_or(DeliveryFailure::Transport)?;
    if authority.is_empty()
        || authority.contains('@')
        || value.contains('#')
        || value.contains('\\')
        || value.chars().any(char::is_whitespace)
    {
        return Err(DeliveryFailure::Transport);
    }
    let url = reqwest::Url::parse(value).map_err(|_| DeliveryFailure::Transport)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(DeliveryFailure::Transport);
    }
    Ok(url)
}

use super::telegram::{limited_text, plain};
use crate::{NotificationDestination, NotificationTransport, RunNotification};
use std::{future::Future, pin::Pin, time::Duration};

pub struct GoogleChatTransport {
    client: reqwest::Client,
    loopback: Option<reqwest::Url>,
    timeout: Duration,
    response_limit: usize,
}
impl GoogleChatTransport {
    pub fn new() -> Result<Self, DeliveryFailure> {
        Self::build(None, Duration::from_secs(10), 65536)
    }
    /// Only for local HTTP fixtures. Never available through Settings or IPC.
    pub fn with_loopback_endpoint(
        endpoint: String,
        timeout: Duration,
        response_limit: usize,
    ) -> Result<Self, DeliveryFailure> {
        let url = reqwest::Url::parse(&endpoint).map_err(|_| DeliveryFailure::Transport)?;
        if endpoint.contains('@')
            || url.scheme() != "http"
            || !url.host_str().is_some_and(|h| {
                h.parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
            })
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(DeliveryFailure::Transport);
        }
        Self::build(Some(url), timeout, response_limit)
    }
    fn build(
        loopback: Option<reqwest::Url>,
        timeout: Duration,
        response_limit: usize,
    ) -> Result<Self, DeliveryFailure> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| DeliveryFailure::Transport)?;
        Ok(Self {
            client,
            loopback,
            timeout,
            response_limit,
        })
    }
    async fn deliver(
        &self,
        destination: NotificationDestination,
        notification: RunNotification,
    ) -> Result<(), DeliveryFailure> {
        let NotificationDestination::GoogleChat { webhook_url } = destination else {
            return Err(DeliveryFailure::Transport);
        };
        let mut url = parse_webhook_url(&webhook_url)?;
        if let Some(endpoint) = &self.loopback {
            url.set_scheme(endpoint.scheme())
                .map_err(|_| DeliveryFailure::Transport)?;
            url.set_host(endpoint.host_str())
                .map_err(|_| DeliveryFailure::Transport)?;
            url.set_port(endpoint.port())
                .map_err(|_| DeliveryFailure::Transport)?;
        }
        let text = limited_text(&format!(
            "{}\n{}",
            plain(&notification.title),
            plain(&notification.body)
        ));
        let mut response = self
            .client
            .post(url)
            .json(&serde_json::json!({"text":text}))
            .send()
            .await
            .map_err(request_failure)?;
        if !response.status().is_success() {
            return Err(DeliveryFailure::HttpRejected);
        }
        if response
            .content_length()
            .is_some_and(|n| n > self.response_limit as u64)
        {
            return Err(DeliveryFailure::ResponseTooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(request_failure)? {
            if chunk.len() > self.response_limit.saturating_sub(body.len()) {
                return Err(DeliveryFailure::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&body).map_err(|_| DeliveryFailure::InvalidResponse)?;
        if value
            .get("name")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty())
        {
            Ok(())
        } else {
            Err(DeliveryFailure::InvalidResponse)
        }
    }
}
impl NotificationTransport for GoogleChatTransport {
    fn send(
        &self,
        destination: NotificationDestination,
        notification: RunNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), DeliveryFailure>> + Send + '_>> {
        Box::pin(async move {
            tokio::time::timeout(self.timeout, self.deliver(destination, notification))
                .await
                .unwrap_or(Err(DeliveryFailure::Timeout))
        })
    }
}
fn request_failure(e: reqwest::Error) -> DeliveryFailure {
    if e.is_timeout() {
        DeliveryFailure::Timeout
    } else {
        DeliveryFailure::Transport
    }
}
