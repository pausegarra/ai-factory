mod common;
use factory_core::{
    context::factory::infrastructure::rest_clients::telegram::TelegramTransport, *,
};
use std::time::Duration;
fn message() -> RunNotification {
    RunNotification {
        run_id: "private-run".into(),
        attempt: 1,
        status: "failed".into(),
        title: "AI Factory — Run failed".into(),
        body: "acme/repo #18 · Safe title · Execution timed out.".into(),
    }
}
fn destination() -> NotificationDestination {
    NotificationDestination::Telegram {
        chat_id: "-1001".into(),
        bot_token: "123456:fixture_only".into(),
    }
}
#[tokio::test]
async fn telegram_posts_safe_plain_text_json() {
    let mut server = common::telegram_http::Server::json_response(
        200,
        serde_json::json!({"ok":true,"result":{"message_id":1}}),
    )
    .await;
    let transport =
        TelegramTransport::with_endpoint(server.base.clone(), Duration::from_secs(1), 65536)
            .unwrap();
    let result = transport.send(destination(), message()).await;
    let requests = server.requests();
    server.shutdown().await;
    assert_eq!(result, Ok(()));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert!(requests[0].path == "/bot123456:fixture_only/sendMessage");
    assert_eq!(
        requests[0].body,
        serde_json::json!({"chat_id":"-1001","text":"AI Factory — Run failed\nacme/repo #18 · Safe title · Execution timed out.","link_preview_options":{"is_disabled":true}})
    );
}
#[tokio::test]
async fn telegram_rejects_remote_failures_without_exposing_response() {
    for (status, body, want) in [
        (
            401,
            serde_json::json!({"description":"PRIVATE_TOKEN"}),
            DeliveryFailure::HttpRejected,
        ),
        (
            429,
            serde_json::json!({"description":"PRIVATE_TOKEN"}),
            DeliveryFailure::HttpRejected,
        ),
        (
            500,
            serde_json::json!({"description":"PRIVATE_TOKEN"}),
            DeliveryFailure::HttpRejected,
        ),
        (
            200,
            serde_json::json!({"ok":false,"description":"PRIVATE_TOKEN"}),
            DeliveryFailure::ApiRejected,
        ),
        (
            200,
            serde_json::json!({"ok":true,"result":{}}),
            DeliveryFailure::InvalidResponse,
        ),
        (
            200,
            serde_json::json!({"ok":1}),
            DeliveryFailure::InvalidResponse,
        ),
    ] {
        let mut server = common::telegram_http::Server::json_response(status, body).await;
        let transport =
            TelegramTransport::with_endpoint(server.base.clone(), Duration::from_secs(1), 65536)
                .unwrap();
        assert_eq!(transport.send(destination(), message()).await, Err(want));
        assert_eq!(server.requests().len(), 1);
        server.shutdown().await;
    }
}
#[tokio::test]
async fn telegram_limits_timeout_body_redirects_and_invalid_json() {
    for (raw,delay,limit,want) in [
        (b"HTTP/1.1 200 OK\r\nContent-Length: 999999\r\n\r\n".to_vec(),0,65536,DeliveryFailure::ResponseTooLarge),
        (b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n10\r\n0123456789abcdef\r\n0\r\n\r\n".to_vec(),0,8,DeliveryFailure::ResponseTooLarge),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nbad".to_vec(),0,65536,DeliveryFailure::InvalidResponse),
        (b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9\r\nContent-Length: 0\r\n\r\n".to_vec(),0,65536,DeliveryFailure::HttpRejected),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec(),250,65536,DeliveryFailure::Timeout),
        (vec![],0,65536,DeliveryFailure::Transport),
    ] {
        let mut server=common::telegram_http::Server::raw(raw,delay).await;
        let transport=TelegramTransport::with_endpoint(server.base.clone(),Duration::from_millis(80),limit).unwrap();
        assert_eq!(transport.send(destination(),message()).await,Err(want));server.shutdown().await;
    }
}
#[tokio::test]
async fn telegram_normalizes_and_limits_utf16_without_splitting_characters() {
    let mut server = common::telegram_http::Server::json_response(
        200,
        serde_json::json!({"ok":true,"result":{"message_id":1}}),
    )
    .await;
    let t = TelegramTransport::with_endpoint(server.base.clone(), Duration::from_secs(1), 65536)
        .unwrap();
    let mut m = message();
    m.title = "Title\n\u{202e}\u{200f}\u{061c}<>&".into();
    m.body = "🦀".repeat(4000);
    assert_eq!(t.send(destination(), m).await, Ok(()));
    let requests = server.requests();
    let text = requests[0].body["text"].as_str().unwrap();
    assert!(text.encode_utf16().count() <= 4096);
    assert_eq!(text.matches('\n').count(), 1);
    assert!(!text
        .chars()
        .any(|c| matches!(c, '\u{202e}' | '\u{200f}' | '\u{061c}')));
    server.shutdown().await;
}
#[tokio::test]
async fn telegram_slow_body_uses_total_deadline_and_connection_failure_is_safe() {
    let mut server = common::telegram_http::Server::delayed_body(
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n12345".to_vec(),
        200,
    )
    .await;
    let t = TelegramTransport::with_endpoint(server.base.clone(), Duration::from_millis(50), 65536)
        .unwrap();
    assert_eq!(
        t.send(destination(), message()).await,
        Err(DeliveryFailure::Timeout)
    );
    server.shutdown().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let t = TelegramTransport::with_endpoint(endpoint, Duration::from_secs(1), 65536).unwrap();
    assert_eq!(
        t.send(destination(), message()).await,
        Err(DeliveryFailure::Transport)
    );
}
#[tokio::test]
async fn telegram_rejects_token_path_injection_before_sending() {
    let mut server = common::telegram_http::Server::json_response(
        200,
        serde_json::json!({"ok":true,"result":{"message_id":1}}),
    )
    .await;
    let t = TelegramTransport::with_endpoint(server.base.clone(), Duration::from_secs(1), 65536)
        .unwrap();
    for token in [
        "123:token/path",
        "123:token?query",
        "123:token#fragment",
        "123:token\n",
    ] {
        assert_eq!(
            t.send(
                NotificationDestination::Telegram {
                    chat_id: "1".into(),
                    bot_token: token.into()
                },
                message()
            )
            .await,
            Err(DeliveryFailure::Transport)
        );
    }
    assert!(server.requests().is_empty());
    server.shutdown().await;
    assert!(TelegramTransport::with_endpoint(
        "http://example.com".into(),
        Duration::from_secs(1),
        65536
    )
    .is_err());
}
#[tokio::test]
async fn telegram_enforces_64k_limit_on_chunked_responses_without_content_length() {
    for (length, want) in [
        (65536, Ok(())),
        (65537, Err(DeliveryFailure::ResponseTooLarge)),
    ] {
        let mut body = b"{\"ok\":true,\"result\":{\"message_id\":1}}".to_vec();
        body.resize(length, b' ');
        let mut raw=format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{length:x}\r\n").into_bytes();
        raw.extend(body);
        raw.extend_from_slice(b"\r\n0\r\n\r\n");
        let mut server = common::telegram_http::Server::raw(raw, 0).await;
        let t =
            TelegramTransport::with_endpoint(server.base.clone(), Duration::from_secs(1), 65536)
                .unwrap();
        assert_eq!(t.send(destination(), message()).await, want);
        server.shutdown().await;
    }
}
