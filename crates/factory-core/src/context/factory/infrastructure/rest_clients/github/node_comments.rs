use super::*;
use crate::context::factory::application::{
    dto::issue_node_comment::*, interfaces::issue_node_comment_transport::*,
};
use IssueNodeCommentFailure as Failure;
fn failure(error: anyhow::Error) -> Failure {
    if let Some(e) = error.downcast_ref::<reqwest::Error>() {
        if e.is_timeout() {
            Failure::Timeout
        } else if e.is_decode() {
            Failure::InvalidResponse
        } else {
            Failure::Transport
        }
    } else if error.to_string().starts_with("GitHub API ") {
        Failure::HttpRejected
    } else if error.to_string().starts_with("GitHub pagination limit") {
        Failure::PaginationIncomplete
    } else if error.to_string() == "GitHub request no longer allowed" {
        Failure::RunUnavailable
    } else if error.to_string().starts_with("API token account") {
        Failure::ActorMismatch
    } else {
        Failure::InvalidResponse
    }
}
fn receipt(value: &Value, request: &IssueNodeCommentRequest) -> Option<IssueNodeCommentReceipt> {
    let id = value["id"].as_u64().filter(|id| *id > 0)?;
    if !value["user"]["login"]
        .as_str()?
        .eq_ignore_ascii_case(&request.actor)
        || !value["body"]
            .as_str()?
            .lines()
            .any(|line| line == request.marker)
    {
        return None;
    }
    let html_url = value["html_url"].as_str().and_then(|text| {
        let url = reqwest::Url::parse(text).ok()?;
        (url.scheme() == "https"
            && url.host_str() == Some("github.com")
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.path() == format!("/{}/issues/{}", request.repository, request.number)
            && url.fragment() == Some(format!("issuecomment-{id}").as_str())
            && url.query().is_none())
        .then(|| url.to_string())
    });
    Some(IssueNodeCommentReceipt {
        comment_id: id,
        html_url,
    })
}
impl IssueNodeCommentTransport for Github {
    fn deliver<'a>(
        &'a self,
        request: &'a IssueNodeCommentRequest,
        can_send: &'a (dyn Fn() -> bool + Send + Sync),
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::result::Result<IssueNodeCommentReceipt, Failure>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            if !valid_repository(&request.repository)
                || request.number == 0
                || !valid_login(&request.actor)
            {
                return Err(Failure::InvalidResponse);
            }
            if !can_send() {
                return Err(Failure::RunUnavailable);
            }
            self.verify_actor(&request.actor).await.map_err(failure)?;
            if !can_send() {
                return Err(Failure::RunUnavailable);
            }
            let path = format!(
                "/repos/{}/issues/{}/comments",
                request.repository, request.number
            );
            let rows = self.pages_guarded(&path, can_send).await.map_err(failure)?;
            if !can_send() {
                return Err(Failure::RunUnavailable);
            }
            if let Some(receipt) = rows.iter().find_map(|row| receipt(row, request)) {
                return Ok(receipt);
            }
            // The caller's lifecycle guard is checked immediately before any mutation.
            if !can_send() {
                return Err(Failure::RunUnavailable);
            }
            let value = self
                .request(Method::POST, &path, Some(&json!({"body":request.body})))
                .await
                .map_err(failure)?;
            receipt(&value, request).ok_or(Failure::InvalidResponse)
        })
    }
}
#[cfg(test)]
mod tests {
    #[allow(dead_code)]
    mod actions_http {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/common/actions_http.rs"
        ));
    }
    use super::*;
    use actions_http::{Reply, Script, Step};
    fn request() -> IssueNodeCommentRequest {
        IssueNodeCommentRequest {
            repository: "acme/repo".into(),
            number: 41,
            actor: "alice".into(),
            marker: "<!-- ai-factory:node-result:v1:abc -->".into(),
            body: "done\n\n<!-- ai-factory:node-result:v1:abc -->".into(),
        }
    }
    fn user() -> Step {
        Step::new("GET", "/user", Reply::Json(200, json!({"login":"alice"})))
            .header("Authorization", "Bearer alice-token")
    }
    fn list(value: Value) -> Step {
        Step::new(
            "GET",
            "/repos/acme/repo/issues/41/comments?per_page=100&page=1",
            Reply::Json(200, value),
        )
        .header("Authorization", "Bearer alice-token")
    }
    fn comment(actor: &str, body: &str) -> Value {
        json!({"id":9001,"body":body,"user":{"login":actor},"html_url":"https://github.com/acme/repo/issues/41#issuecomment-9001"})
    }
    fn post(reply: Reply) -> Step {
        Step::new("POST", "/repos/acme/repo/issues/41/comments", reply)
            .body(json!({"body":request().body}))
            .header("Authorization", "Bearer alice-token")
    }
    #[tokio::test]
    async fn node_comments_create_and_reconcile_exact_actor_marker() {
        let r = request();
        let mut server = Script::start(vec![
            user(),
            list(json!([])),
            post(Reply::Json(201, comment("Alice", &r.body))),
            user(),
            list(json!([comment("alice", &r.body)])),
        ])
        .await;
        let api = Github::with_base("alice-token", &server.base).unwrap();
        for _ in 0..2 {
            let receipt = api.deliver(&r, &|| true).await.expect("confirmed receipt");
            assert_eq!(receipt.comment_id, 9001);
            assert_eq!(
                receipt.html_url.as_deref(),
                Some("https://github.com/acme/repo/issues/41#issuecomment-9001")
            );
        }
        assert_eq!(server.writes().len(), 1);
        server.assert_done();
        server.shutdown().await;
    }
    #[tokio::test]
    async fn node_comments_uncertain_post_reconciles_without_second_write() {
        for reply in [
            Reply::Disconnect,
            Reply::Json(503, json!({})),
            Reply::Raw(201, b"broken".to_vec()),
        ] {
            let r = request();
            let mut server = Script::start(vec![
                user(),
                list(json!([])),
                post(reply),
                user(),
                list(json!([comment("alice", &r.body)])),
            ])
            .await;
            let api = Github::with_base("alice-token", &server.base).unwrap();
            assert!(api.deliver(&r, &|| true).await.is_err());
            assert_eq!(api.deliver(&r, &|| true).await.unwrap().comment_id, 9001);
            assert_eq!(server.writes().len(), 1);
            server.assert_done();
            server.shutdown().await;
        }
    }
    #[tokio::test]
    async fn node_comments_fail_closed_and_check_lifecycle_before_post() {
        for steps in [
            vec![Step::new(
                "GET",
                "/user",
                Reply::Json(200, json!({"login":"mallory"})),
            )],
            vec![user(), list(json!({"bad":"list"}))],
            vec![
                user(),
                Step::new(
                    "GET",
                    "/repos/acme/repo/issues/41/comments?per_page=100&page=1",
                    Reply::Json(500, json!({})),
                ),
            ],
        ] {
            let mut server = Script::start(steps).await;
            let api = Github::with_base("alice-token", &server.base).unwrap();
            assert!(api.deliver(&request(), &|| true).await.is_err());
            assert!(server.writes().is_empty());
            server.assert_done();
            server.shutdown().await;
        }
        let live = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let flag = live.clone();
        let mut server = Script::start(vec![
            user(),
            list(json!([])).then(move || flag.store(false, std::sync::atomic::Ordering::SeqCst)),
        ])
        .await;
        let api = Github::with_base("alice-token", &server.base).unwrap();
        assert_eq!(
            api.deliver(&request(), &|| live
                .load(std::sync::atomic::Ordering::SeqCst))
                .await
                .unwrap_err(),
            IssueNodeCommentFailure::RunUnavailable
        );
        server.assert_done();
        assert!(server.writes().is_empty());
        server.shutdown().await;
    }
    #[tokio::test]
    async fn node_comments_ignore_other_actor_partial_marker_and_invalid_receipts() {
        for reply in [
            Reply::Json(403, json!({})),
            Reply::Json(201, json!({"id":0})),
            Reply::Json(201, comment("mallory", &request().body)),
        ] {
            let r = request();
            let rows = json!([
                comment("mallory", &r.body),
                comment("alice", &format!("x{}", r.marker))
            ]);
            let mut server = Script::start(vec![user(), list(rows), post(reply)]).await;
            let api = Github::with_base("alice-token", &server.base).unwrap();
            assert!(api.deliver(&r, &|| true).await.is_err());
            assert_eq!(server.writes().len(), 1);
            server.assert_done();
            server.shutdown().await;
        }
    }
    #[tokio::test]
    async fn node_comments_page_two_and_incomplete_pagination() {
        let r = request();
        let full = json!(vec![comment("mallory", &r.body); 100]);
        let mut server = Script::start(vec![
            user(),
            list(full.clone()),
            Step::new(
                "GET",
                "/repos/acme/repo/issues/41/comments?per_page=100&page=2",
                Reply::Json(200, json!([comment("alice", &r.body)])),
            ),
        ])
        .await;
        let api = Github::with_base("alice-token", &server.base).unwrap();
        assert_eq!(api.deliver(&r, &|| true).await.unwrap().comment_id, 9001);
        server.assert_done();
        assert!(server.writes().is_empty());
        server.shutdown().await;
        let mut steps = vec![user()];
        for page in 1..=100 {
            steps.push(Step::new(
                "GET",
                &format!("/repos/acme/repo/issues/41/comments?per_page=100&page={page}"),
                Reply::Json(200, full.clone()),
            ));
        }
        let mut server = Script::start(steps).await;
        let api = Github::with_base("alice-token", &server.base).unwrap();
        assert_eq!(
            api.deliver(&r, &|| true).await.unwrap_err(),
            IssueNodeCommentFailure::PaginationIncomplete
        );
        server.assert_done();
        assert!(server.writes().is_empty());
        server.shutdown().await;
    }
    #[tokio::test]
    async fn node_comments_deletion_between_pages_stops_further_requests() {
        let live = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let flag = live.clone();
        let r = request();
        let mut server = Script::start(vec![
            user(),
            list(json!(vec![comment("mallory", &r.body); 100]))
                .then(move || flag.store(false, std::sync::atomic::Ordering::SeqCst)),
        ])
        .await;
        let api = Github::with_base("alice-token", &server.base).unwrap();
        assert_eq!(
            api.deliver(&r, &|| live.load(std::sync::atomic::Ordering::SeqCst))
                .await
                .unwrap_err(),
            Failure::RunUnavailable
        );
        server.assert_done();
        assert!(server.writes().is_empty());
        server.shutdown().await;
    }
}
