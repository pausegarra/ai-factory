use crate::context::factory::application::dto::issue_node_comment::*;
use sha2::{Digest, Sha256};
use std::time::Duration;
pub(crate) fn terminal_for_issue_comment(status: &str) -> bool {
    matches!(status, "succeeded" | "blocked" | "failed" | "skipped")
}
pub(crate) fn marker_for(run: &str, node: &str, revision: u32) -> String {
    let bytes = serde_json::to_vec(&serde_json::json!(["node-result:v1", run, node, revision]))
        .expect("JSON identity");
    format!(
        "<!-- ai-factory:node-result:v1:{:x} -->",
        Sha256::digest(bytes)
    )
}
fn limited(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.into();
    }
    let mut end = max.saturating_sub(3);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}
fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || c.is_whitespace())
        .filter(|c| {
            !matches!(*c,
                '\u{200b}' | '\u{feff}' | '\u{061c}' | '\u{200e}' | '\u{200f}' |
                '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
            )
        })
        .map(|c| match c {
            '<' => '‹',
            '>' => '›',
            '@' => '＠',
            _ => c,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn safe_link(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    let host = url.host_str()?.trim_matches(['[', ']']);
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| {
            let ip = match ip {
                std::net::IpAddr::V6(ip) => ip
                    .to_ipv4_mapped()
                    .map(std::net::IpAddr::V4)
                    .unwrap_or(std::net::IpAddr::V6(ip)),
                ip => ip,
            };
            ip.is_loopback() || ip.is_unspecified()
        })
    {
        return None;
    }
    Some(url.to_string().replace('(', "%28").replace(')', "%29"))
}
pub(crate) fn request_for(
    s: &IssueNodeCommentSnapshot,
    url: Option<&str>,
    redact: impl Fn(&str) -> String,
) -> IssueNodeCommentRequest {
    // Redact complete fields before any normalization or truncation (including identity).
    let field = |text: &str| {
        // Removing controls can join fragments into a known secret. Redact both
        // the original and the complete normalized field before truncation.
        clean(&redact(&clean(&redact(text))))
    };
    let name = field(&s.node_name).chars().take(160).collect::<String>();
    let node = field(&s.node_id).chars().take(96).collect::<String>();
    let name = if name.is_empty() { "Step" } else { &name };
    let summary = s.summary.as_deref().map(field).filter(|s| !s.is_empty());
    let error = s.error.as_deref().map(field).filter(|s| !s.is_empty());
    let text = match s.status.as_str() {
        "skipped" => format!(
            "Step was not executed.{}",
            error.map(|e| format!(" {e}")).unwrap_or_default()
        ),
        "succeeded" => summary.unwrap_or_else(|| "Step completed; no summary available.".into()),
        "blocked" => summary.unwrap_or_else(|| "Step blocked; attention required.".into()),
        _ => summary
            .or(error)
            .unwrap_or_else(|| "Step failed; no safe details available.".into()),
    };
    let marker = marker_for(&s.run_id, &s.node_id, s.revision);
    let mut body = format!(
        "AI Factory · {name} ({node}) · {}\n\nRun {} · attempt {} · result {}\n\n{}",
        s.status,
        limited(&field(&s.run_id), 256),
        s.run_attempt,
        s.revision,
        limited(&text, 3500)
    );
    if let Some(url) = url.and_then(safe_link).filter(|u| u.len() <= 1000) {
        body.push_str(&format!("\n\n[Run]({url})"));
    }
    body = limited(&body, 6000 - marker.len() - 2);
    body.push_str("\n\n");
    body.push_str(&marker);
    IssueNodeCommentRequest {
        repository: s.repository.clone(),
        number: s.number,
        actor: s.actor.clone(),
        marker,
        body,
    }
}
pub(crate) fn retry_delay(failures: u32) -> Option<Duration> {
    match failures {
        1 => Some(1),
        2 => Some(5),
        3 => Some(30),
        4 => Some(120),
        _ => None,
    }
    .map(Duration::from_secs)
}
use crate::context::factory::application::interfaces::issue_node_comment_transport::{
    IssueNodeCommentFailure as Failure, IssueNodeCommentTransport,
};
use crate::{now, safe_id, valid_deletion_id, Factory};
use std::sync::{Arc, Weak};
use tokio_util::sync::CancellationToken;
pub(crate) struct IssueNodeCommentDispatcher {
    handle: tokio::task::JoinHandle<()>,
    stop: CancellationToken,
}
impl IssueNodeCommentDispatcher {
    pub fn start(factory: Weak<Factory>, stop: CancellationToken) -> Self {
        let task_stop = stop.clone();
        let handle = tokio::spawn(async move {
            loop {
                if task_stop.is_cancelled() {
                    break;
                }
                let Some(f) = factory.upgrade() else { break };
                let job = match f.store.due_issue_node_comment(&now()) {
                    Ok(job) => job,
                    Err(_) => {
                        eprintln!("issue_node_comment_dispatch_failed reason=store_unavailable");
                        None
                    }
                };
                if let Some(job) = job {
                    let result = tokio::select! { biased; _=task_stop.cancelled()=>Err(Failure::Stopped),result=deliver_job(&f,&job,&task_stop)=>result };
                    record_delivery(&f, &job, result);
                    drop(f);
                } else {
                    drop(f);
                    tokio::select! {_=task_stop.cancelled()=>break,_=tokio::time::sleep(Duration::from_millis(300))=>{}}
                }
            }
        });
        Self { handle, stop }
    }
    pub async fn shutdown(self) {
        self.stop.cancel();
        let _ = self.handle.await;
    }
}
async fn deliver_job(
    f: &Arc<Factory>,
    job: &IssueNodeCommentJob,
    stop: &CancellationToken,
) -> Result<IssueNodeCommentReceipt, Failure> {
    if !f
        .store
        .issue_node_comment_allowed(&job.key)
        .unwrap_or(false)
    {
        return Err(Failure::RunUnavailable);
    }
    let load = f.clone();
    tokio::task::spawn_blocking(move || load.store.load_redaction_secrets())
        .await
        .map_err(|_| Failure::RedactionUnavailable)?
        .map_err(|_| Failure::RedactionUnavailable)?;
    // Retry a diagnostic whose previous append may have failed. Keep its durable
    // error code until reconciliation succeeds; never let log I/O undo a result.
    if job.failures > 0 {
        if let Ok(Some(code)) = f.store.issue_node_comment_last_error(&job.key) {
            diagnostic(f, job, &failure_line(job, &code, true, job.failures));
        }
    }
    let transport = f
        .issue_node_comment_transport(&job.snapshot.actor)
        .await
        .map_err(|_| Failure::CredentialUnavailable)?;
    let request = request_for(&job.snapshot, None, |text| f.store.redact_cached(text));
    transport
        .deliver(&request, &|| {
            !stop.is_cancelled()
                && f.store
                    .issue_node_comment_allowed(&job.key)
                    .unwrap_or(false)
        })
        .await
}
fn failure_line(
    job: &IssueNodeCommentJob,
    code: &str,
    metadata_safe: bool,
    attempt: u32,
) -> String {
    // A locked credential backend means the full redaction set is unavailable.
    // Do not expose a configured ID that could itself equal an unloaded secret.
    let node = if metadata_safe {
        job.snapshot.node_id.as_str()
    } else {
        "unavailable"
    };
    let retry = if retry_delay(attempt).is_some() {
        "pending"
    } else {
        "exhausted"
    };
    format!("issue_node_comment_delivery_failed node={node} revision={} attempt={attempt} reason={code} retry={retry}", job.snapshot.revision)
}
fn record_delivery(
    f: &Factory,
    job: &IssueNodeCommentJob,
    result: Result<IssueNodeCommentReceipt, Failure>,
) {
    let line = match result {
        Ok(receipt) => match f.store.confirm_issue_node_comment(&job.key, &receipt) {
            Ok(true) => Some(format!(
                "issue_node_comment_delivered node={} revision={} comment_id={}",
                job.snapshot.node_id, job.snapshot.revision, receipt.comment_id
            )),
            Ok(false) => None,
            Err(_) => {
                eprintln!("issue_node_comment_dispatch_failed reason=store_unavailable");
                None
            }
        },
        Err(Failure::Stopped | Failure::RunUnavailable) => None,
        Err(code) => match f
            .store
            .fail_issue_node_comment(&job.key, code.code(), &now())
        {
            Ok(true) => Some(failure_line(
                job,
                code.code(),
                code != Failure::RedactionUnavailable,
                job.failures + 1,
            )),
            Ok(false) => None,
            Err(_) => {
                eprintln!("issue_node_comment_dispatch_failed reason=store_unavailable");
                None
            }
        },
    };
    if let Some(line) = line {
        diagnostic(f, job, &line);
    }
}
fn diagnostic(f: &Factory, job: &IssueNodeCommentJob, line: &str) {
    use std::io::Write;
    let _gate = f.lifecycle_gate.lock().unwrap();
    let s = &job.snapshot;
    if !valid_deletion_id(&s.run_id) || !safe_id(&s.node_id) {
        return;
    }
    let Ok(detail) = f.store.detail(&s.run_id) else {
        return;
    };
    if !detail.nodes.iter().any(|n| n.id == s.node_id) {
        return;
    }
    let text = format!(
        "{}\n",
        limited(
            &clean(
                &f.store
                    .redact_cached(&format!("[Factory notification] {line}"))
            ),
            511
        )
    );
    let append = || -> crate::Result<()> {
        let dir = f.root.join("sessions").join(&s.run_id);
        std::fs::create_dir_all(&dir)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{}.log", s.node_id)))?;
        file.write_all(text.as_bytes())?;
        Ok(())
    };
    if append().is_err() {
        eprintln!("issue_node_comment_diagnostic_failed reason=io");
    } else {
        f.emit("log", Some(&s.run_id), Some(&s.node_id));
    }
}
impl Factory {
    pub(crate) async fn issue_node_comment_transport(
        &self,
        actor: &str,
    ) -> crate::Result<Box<dyn IssueNodeCommentTransport>> {
        Ok(Box::new(self.github(&self.store.token_for(actor).await?)?))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn sample(status: &str) -> IssueNodeCommentSnapshot {
        IssueNodeCommentSnapshot {
            run_id: "run-a".into(),
            node_id: "builder".into(),
            node_name: "Builder".into(),
            repository: "acme/repo".into(),
            number: 41,
            actor: "alice".into(),
            run_attempt: 1,
            revision: 1,
            status: status.into(),
            summary: Some("Terminado".into()),
            error: None,
        }
    }
    #[test]
    fn issue_node_comment_eligibility_and_delays() {
        for status in ["succeeded", "blocked", "failed", "skipped"] {
            assert!(terminal_for_issue_comment(status));
        }
        for status in ["pending", "running", "cancelled", "interrupted", "unknown"] {
            assert!(!terminal_for_issue_comment(status));
        }
        for (f, want) in [(1, 1), (2, 5), (3, 30), (4, 120)] {
            assert_eq!(retry_delay(f), Some(Duration::from_secs(want)));
        }
        assert_eq!(retry_delay(5), None);
    }
    #[test]
    fn issue_node_comment_identity() {
        let a = marker_for("a", "n", 1);
        assert!(a.starts_with("<!-- ai-factory:node-result:v1:"));
        assert_eq!(a, marker_for("a", "n", 1));
        for b in [
            marker_for("b", "n", 1),
            marker_for("a", "m", 1),
            marker_for("a", "n", 2),
        ] {
            assert_ne!(a, b);
        }
    }
    #[test]
    fn issue_node_comment_text_redacts_before_truncating() {
        let mut s = sample("failed");
        s.summary = Some(format!(
            "{}SECRET_TOKEN_FINAL{}",
            "x".repeat(3480),
            "é🦀".repeat(1800)
        ));
        s.error = Some("PRIVATE_ERROR".into());
        let r = request_for(&s, None, |x| x.replace("SECRET_TOKEN_FINAL", "[redacted]"));
        assert!(r.body.contains("[redacted]"));
        assert!(!r.body.contains("SECRET_TOKEN"));
        assert!(!r.body.contains("PRIVATE_ERROR"));
        assert!(r.body.len() <= 6000);
        assert!(r.body.contains("Builder"));
        assert!(r.body.contains("failed"));
        assert!(r.body.ends_with(&marker_for("run-a", "builder", 1)));
    }
    #[test]
    fn issue_node_comment_skipped_uses_only_recorded_cause() {
        for (error, want) in [
            (None, "Step was not executed."),
            (
                Some("Dependency did not succeed"),
                "Step was not executed. Dependency did not succeed",
            ),
            (
                Some("Workspace SECRET"),
                "Step was not executed. Workspace [redacted]",
            ),
        ] {
            let mut s = sample("skipped");
            s.summary = Some("old dependency summary".into());
            s.error = error.map(str::to_owned);
            let body = request_for(&s, None, |x| x.replace("SECRET", "[redacted]")).body;
            assert!(body.contains(want));
            assert!(!body.contains("old dependency"));
            if error.is_none() {
                assert!(!body.to_lowercase().contains("dependency"));
            }
        }
    }
    #[test]
    fn issue_node_comment_fallback_and_sanitization() {
        for (status, want) in [
            ("succeeded", "Step completed; no summary available."),
            ("blocked", "Step blocked; attention required."),
            ("failed", "Step failed; no safe details available."),
        ] {
            for summary in [
                None,
                Some(""),
                Some(" \n\u{202e}\u{0000}"),
                Some("\u{200b}\u{feff}"),
            ] {
                let mut s = sample(status);
                s.summary = summary.map(str::to_owned);
                assert!(request_for(&s, None, str::to_owned).body.contains(want));
            }
        }
        let mut s = sample("failed");
        s.summary = None;
        s.error = Some("ERROR".into());
        assert!(request_for(&s, None, str::to_owned).body.contains("ERROR"));
        s.summary = Some("<script>@all</script> <!-- evil --> \u{202e} hi".into());
        s.node_name = "🦀".repeat(200);
        s.node_id = "é".repeat(200);
        let body = request_for(&s, None, str::to_owned).body;
        assert!(!body.contains("<script>"));
        assert!(!body.contains("@all"));
        assert!(!body.contains('\u{202e}'));
        assert_eq!(body.matches("<!--").count(), 1);
        assert!(body.len() <= 6000);
    }
    #[test]
    fn issue_node_comment_run_links() {
        for url in [
            "file:///tmp/x",
            "javascript:evil",
            "https://localhost/x",
            "https://127.0.0.1/x",
            "https://[::1]/x",
            "https://[::ffff:127.0.0.1]/x",
            "https://user:pass@example.com/x",
        ] {
            assert!(!request_for(&sample("succeeded"), Some(url), str::to_owned)
                .body
                .contains("](http"));
        }
        assert!(request_for(
            &sample("succeeded"),
            Some("https://factory.example/runs/a"),
            str::to_owned
        )
        .body
        .contains("https://factory.example/runs/a"));
    }
    #[test]
    fn issue_node_comment_sanitization_cannot_reconstruct_a_known_secret() {
        let mut s = sample("failed");
        s.summary = Some("SUPER\u{202e}_SECRET".into());
        let body = request_for(&s, None, |text| text.replace("SUPER_SECRET", "[redacted]")).body;
        assert!(!body.contains("SUPER_SECRET"));
        assert!(body.contains("[redacted]"));
    }
}
