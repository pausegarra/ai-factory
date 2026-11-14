use crate::{DeliveryFailure, NotificationDestination, NotificationTransport, RunNotification};
use std::{future::Future, pin::Pin, time::Duration};
type DeliveryResult<T> = Result<T, DeliveryFailure>;

pub struct TelegramTransport {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    timeout: Duration,
    response_limit: usize,
}
impl TelegramTransport {
    pub fn new() -> DeliveryResult<Self> {
        Self::with_endpoint(
            "https://api.telegram.org".into(),
            Duration::from_secs(10),
            65536,
        )
    }
    /// Alternative endpoints are restricted to loopback fixtures or the official HTTPS API.
    /// This is never exposed through Settings or IPC.
    pub fn with_endpoint(
        endpoint: String,
        timeout: Duration,
        response_limit: usize,
    ) -> DeliveryResult<Self> {
        let endpoint = reqwest::Url::parse(&endpoint).map_err(|_| DeliveryFailure::Transport)?;
        let loopback = endpoint.host_str().is_some_and(|h| {
            h.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        });
        if !(endpoint.scheme() == "https" && endpoint.host_str() == Some("api.telegram.org")
            || endpoint.scheme() == "http" && loopback)
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.path() != "/"
        {
            return Err(DeliveryFailure::Transport);
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| DeliveryFailure::Transport)?;
        Ok(Self {
            client,
            endpoint,
            timeout,
            response_limit,
        })
    }
    async fn deliver(
        &self,
        destination: NotificationDestination,
        notification: RunNotification,
    ) -> DeliveryResult<()> {
        let NotificationDestination::Telegram { chat_id, bot_token } = destination else {
            return Err(DeliveryFailure::Transport);
        };
        if bot_token.is_empty()
            || !bot_token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'_' | b'-'))
        {
            return Err(DeliveryFailure::Transport);
        }
        let mut url = self.endpoint.clone();
        url.set_path(&format!("/bot{bot_token}/sendMessage"));
        let text = limited_text(&format!(
            "{}\n{}",
            plain(&notification.title),
            plain(&notification.body)
        ));
        let mut response = self
            .client
            .post(url)
            .json(&serde_json::json!({
                "chat_id": chat_id, "text": text, "link_preview_options": {"is_disabled": true}
            }))
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
        match value.get("ok").and_then(|v| v.as_bool()) {
            Some(false) => Err(DeliveryFailure::ApiRejected),
            Some(true)
                if value
                    .get("result")
                    .and_then(|v| v.get("message_id"))
                    .and_then(|v| v.as_i64())
                    .is_some() =>
            {
                Ok(())
            }
            _ => Err(DeliveryFailure::InvalidResponse),
        }
    }
}
impl NotificationTransport for TelegramTransport {
    fn send(
        &self,
        destination: NotificationDestination,
        notification: RunNotification,
    ) -> Pin<Box<dyn Future<Output = DeliveryResult<()>> + Send + '_>> {
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
pub(super) fn plain(text: &str) -> String {
    text.chars()
        .filter_map(|c| {
            if c.is_whitespace() || matches!(c, '<' | '>' | '&') {
                Some(' ')
            } else if c.is_control()
                || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                None
            } else {
                Some(c)
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
pub(super) fn limited_text(text: &str) -> String {
    let mut budget = 4096;
    text.chars()
        .take_while(|c| {
            let n = c.len_utf16();
            if n > budget {
                false
            } else {
                budget -= n;
                true
            }
        })
        .collect()
}
