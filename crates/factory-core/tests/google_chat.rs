mod common;
use factory_core::{
    context::factory::infrastructure::rest_clients::google_chat::GoogleChatTransport, *,
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
    NotificationDestination::GoogleChat {
        webhook_url: "https://chat.googleapis.com/v1/spaces/fixture/messages?key=fixture_key&token=fixture_token&extra=keep".into(),
    }
}
#[tokio::test]
async fn google_chat_posts_safe_plain_text_json() {
    let mut server = common::telegram_http::Server::json_response(
        200,
        serde_json::json!({"name":"spaces/fixture/messages/message-1"}),
    )
    .await;
    let transport = GoogleChatTransport::with_loopback_endpoint(
        server.base.clone(),
        Duration::from_secs(1),
        65536,
    )
    .unwrap();
    let result = transport.send(destination(), message()).await;
    let requests = server.requests();
    server.shutdown().await;
    assert_eq!(result, Ok(()));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(
        requests[0].path,
        "/v1/spaces/fixture/messages?key=fixture_key&token=fixture_token&extra=keep"
    );
    assert!(requests[0].content_type.starts_with("application/json"));
    assert_eq!(
        requests[0].body,
        serde_json::json!({"text":"AI Factory — Run failed\nacme/repo #18 · Safe title · Execution timed out."})
    );
}
#[tokio::test]
async fn google_chat_rejects_remote_failures_without_exposing_response() {
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
            DeliveryFailure::InvalidResponse,
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
        let transport = GoogleChatTransport::with_loopback_endpoint(
            server.base.clone(),
            Duration::from_secs(1),
            65536,
        )
        .unwrap();
        assert_eq!(transport.send(destination(), message()).await, Err(want));
        assert_eq!(server.requests().len(), 1);
        server.shutdown().await;
    }
}
#[tokio::test]
async fn google_chat_limits_timeout_body_redirects_and_invalid_json() {
    for (raw,delay,limit,want) in [
        (b"HTTP/1.1 200 OK\r\nContent-Length: 999999\r\n\r\n".to_vec(),0,65536,DeliveryFailure::ResponseTooLarge),
        (b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n10\r\n0123456789abcdef\r\n0\r\n\r\n".to_vec(),0,8,DeliveryFailure::ResponseTooLarge),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nbad".to_vec(),0,65536,DeliveryFailure::InvalidResponse),
        (b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9\r\nContent-Length: 0\r\n\r\n".to_vec(),0,65536,DeliveryFailure::HttpRejected),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec(),250,65536,DeliveryFailure::Timeout),
        (vec![],0,65536,DeliveryFailure::Transport),
    ] {
        let mut server=common::telegram_http::Server::raw(raw,delay).await;
        let transport=GoogleChatTransport::with_loopback_endpoint(server.base.clone(),Duration::from_millis(80),limit).unwrap();
        assert_eq!(transport.send(destination(),message()).await,Err(want));server.shutdown().await;
    }
}
#[tokio::test]
async fn google_chat_normalizes_and_limits_utf16_without_splitting_characters() {
    let mut server = common::telegram_http::Server::json_response(
        200,
        serde_json::json!({"name":"spaces/fixture/messages/message-1"}),
    )
    .await;
    let t = GoogleChatTransport::with_loopback_endpoint(
        server.base.clone(),
        Duration::from_secs(1),
        65536,
    )
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
async fn google_chat_slow_body_uses_total_deadline_and_connection_failure_is_safe() {
    let mut server = common::telegram_http::Server::delayed_body(
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n12345".to_vec(),
        200,
    )
    .await;
    let t = GoogleChatTransport::with_loopback_endpoint(
        server.base.clone(),
        Duration::from_millis(50),
        65536,
    )
    .unwrap();
    assert_eq!(
        t.send(destination(), message()).await,
        Err(DeliveryFailure::Timeout)
    );
    server.shutdown().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let t = GoogleChatTransport::with_loopback_endpoint(endpoint, Duration::from_secs(1), 65536)
        .unwrap();
    assert_eq!(
        t.send(destination(), message()).await,
        Err(DeliveryFailure::Transport)
    );
}
#[tokio::test]
async fn google_chat_rejects_unsafe_destinations_and_fixture_overrides() {
    let mut server =
        common::telegram_http::Server::json_response(200, serde_json::json!({"name":"ok"})).await;
    let t = GoogleChatTransport::with_loopback_endpoint(
        server.base.clone(),
        Duration::from_secs(1),
        65536,
    )
    .unwrap();
    for url in [
        "http://host/path",
        "https://u:p@host/path",
        "https://@host/path",
        "https://host/#fragment",
        "https://host/path?token=abc\n",
        "https:///path",
        "https://host/a b",
        "https://host/a\\b",
    ] {
        assert_eq!(
            t.send(
                NotificationDestination::GoogleChat {
                    webhook_url: url.into()
                },
                message()
            )
            .await,
            Err(DeliveryFailure::Transport)
        );
    }
    assert!(server.requests().is_empty());
    server.shutdown().await;
    for endpoint in [
        "http://example.com",
        "https://127.0.0.1",
        "http://localhost",
        "http://127.0.0.1/path",
        "http://127.0.0.1?x=1",
        "http://u:p@127.0.0.1",
        "http://@127.0.0.1",
    ] {
        assert!(GoogleChatTransport::with_loopback_endpoint(
            endpoint.into(),
            Duration::from_secs(1),
            65536
        )
        .is_err());
    }
}
#[tokio::test]
async fn google_chat_enforces_64k_limit_on_chunked_responses_without_content_length() {
    for (length, want) in [
        (65536, Ok(())),
        (65537, Err(DeliveryFailure::ResponseTooLarge)),
    ] {
        let mut body = b"{\"name\":\"spaces/fixture/messages/1\"}".to_vec();
        body.resize(length, b' ');
        let mut raw=format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{length:x}\r\n").into_bytes();
        raw.extend(body);
        raw.extend_from_slice(b"\r\n0\r\n\r\n");
        let mut server = common::telegram_http::Server::raw(raw, 0).await;
        let t = GoogleChatTransport::with_loopback_endpoint(
            server.base.clone(),
            Duration::from_secs(1),
            65536,
        )
        .unwrap();
        assert_eq!(t.send(destination(), message()).await, want);
        server.shutdown().await;
    }
}

#[tokio::test]
async fn google_chat_partial_response_contract_rejects_non_messages() {
    for body in [
        serde_json::json!(null),
        serde_json::json!([]),
        serde_json::json!({}),
        serde_json::json!({"name":7}),
        serde_json::json!({"name":""}),
        serde_json::json!({"name":"   "}),
        serde_json::json!({"error":"PRIVATE_TOKEN"}),
    ] {
        let mut server = common::telegram_http::Server::json_response(200, body).await;
        let t = GoogleChatTransport::with_loopback_endpoint(
            server.base.clone(),
            Duration::from_secs(1),
            65536,
        )
        .unwrap();
        assert_eq!(
            t.send(destination(), message()).await,
            Err(DeliveryFailure::InvalidResponse)
        );
        assert_eq!(server.requests().len(), 1);
        server.shutdown().await;
    }
}
#[tokio::test]
async fn google_chat_never_follows_redirects_to_another_receiver() {
    let mut sink =
        common::telegram_http::Server::json_response(200, serde_json::json!({"name":"wrong"}))
            .await;
    let mut source = common::telegram_http::Server::raw(
        format!(
            "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\n\r\n",
            sink.base
        )
        .into_bytes(),
        0,
    )
    .await;
    let t = GoogleChatTransport::with_loopback_endpoint(
        source.base.clone(),
        Duration::from_secs(1),
        65536,
    )
    .unwrap();
    assert_eq!(
        t.send(destination(), message()).await,
        Err(DeliveryFailure::HttpRejected)
    );
    assert_eq!(source.requests().len(), 1);
    assert!(sink.requests().is_empty());
    source.shutdown().await;
    sink.shutdown().await;
}

#[tokio::test]
async fn composite_routes_each_provider_and_rejects_crossed_destinations() {
    use factory_core::context::factory::infrastructure::rest_clients::{
        notifications::ConnectorTransports, telegram::TelegramTransport,
    };
    let mut tg = common::telegram_http::Server::json_response(
        200,
        serde_json::json!({"ok":true,"result":{"message_id":1}}),
    )
    .await;
    let mut gc = common::telegram_http::Server::json_response(
        200,
        serde_json::json!({"name":"spaces/fixture/messages/1"}),
    )
    .await;
    let telegram =
        TelegramTransport::with_endpoint(tg.base.clone(), Duration::from_secs(1), 65536).unwrap();
    let google =
        GoogleChatTransport::with_loopback_endpoint(gc.base.clone(), Duration::from_secs(1), 65536)
            .unwrap();
    assert_eq!(
        telegram.send(destination(), message()).await,
        Err(DeliveryFailure::Transport)
    );
    assert_eq!(
        google
            .send(
                NotificationDestination::Telegram {
                    chat_id: "chat".into(),
                    bot_token: "123:fixture".into()
                },
                message()
            )
            .await,
        Err(DeliveryFailure::Transport)
    );
    assert!(tg.requests().is_empty());
    assert!(gc.requests().is_empty());
    let composite = ConnectorTransports::with_transports(telegram, google);
    assert_eq!(composite.send(destination(), message()).await, Ok(()));
    assert_eq!(
        composite
            .send(
                NotificationDestination::Telegram {
                    chat_id: "chat".into(),
                    bot_token: "123:fixture".into()
                },
                message()
            )
            .await,
        Ok(())
    );
    assert_eq!(gc.requests().len(), 1);
    assert_eq!(tg.requests().len(), 1);
    tg.shutdown().await;
    gc.shutdown().await;
}
