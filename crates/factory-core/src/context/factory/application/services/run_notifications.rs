use crate::context::factory::domain::entities::run_transition::RunTransition;
use crate::RunNotification;
pub(crate) fn notification_for(
    transition: &RunTransition,
    redact: impl Fn(&str) -> String,
) -> Option<RunNotification> {
    let run = &transition.run;
    let title = match (transition.previous_status.as_str(), run.status.as_str()) {
        ("queued", "running") => "AI Factory — Run started",
        ("queued" | "running", "succeeded") => "AI Factory — Run succeeded",
        ("queued" | "running", "failed") => "AI Factory — Run failed",
        ("queued" | "running", "blocked") => "AI Factory — Run blocked",
        _ => return None,
    };
    // Redact before limiting length so a truncated token cannot leak its prefix.
    let repository = visible_text(&redact(&run.repository), 101);
    let redacted_title = visible_text(&redact(&run.title), usize::MAX);
    let safe_title = if contains_credential_marker(&redacted_title) {
        "[Title hidden]".into()
    } else {
        let title = visible_text(&redacted_title, 120);
        if title.is_empty() {
            "(untitled)".into()
        } else {
            title
        }
    };
    let mut body = format!("{repository} #{} · {safe_title}", run.number);
    if matches!(run.status.as_str(), "failed" | "blocked") {
        if let Some(error) = run.error.as_deref().filter(|e| !e.trim().is_empty()) {
            body.push_str(" · ");
            body.push_str(safe_error_summary(error));
        }
    }
    Some(RunNotification {
        run_id: run.id.clone(),
        attempt: run.attempt,
        status: run.status.clone(),
        title: title.into(),
        body: visible_text(&body, 360),
    })
}

fn contains_credential_marker(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "github_pat_",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "authorization:",
        "bearer ",
        "password=",
        "token=",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
        || lower.match_indices("sk-").any(|(index, _)| {
            index == 0
                || !lower[..index]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
}

fn visible_text(text: &str, limit: usize) -> String {
    let plain: String = text
        .chars()
        .filter_map(|c| {
            if c.is_whitespace() || matches!(c, '<' | '>' | '&') {
                Some(' ')
            } else if c.is_control()
                || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                None
            } else {
                Some(c)
            }
        })
        .collect();
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if plain.chars().count() <= limit {
        plain
    } else {
        plain
            .chars()
            .take(limit.saturating_sub(1))
            .chain(std::iter::once('…'))
            .collect()
    }
}

// External error text is untrusted. Only closed, static summaries reach the OS.
fn safe_error_summary(error: &str) -> &'static str {
    let error = error.to_ascii_lowercase();
    if error.contains("timed out") || error.contains("timeout") {
        "Execution timed out."
    } else if error.contains("agent exited with") {
        "Agent process exited unsuccessfully."
    } else if error.contains("merge and source branch cleanup were not confirmed") {
        "Merge or branch cleanup was not confirmed."
    } else if error.contains("publication was not confirmed") {
        "Release publication was not confirmed."
    } else {
        "Execution reported an error. See run details."
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn transition(previous: &str, status: &str, title: &str, error: Option<&str>) -> RunTransition {
        RunTransition {
            previous_status: previous.into(),
            run: crate::Run {
                id: "run-alpha".into(),
                event_id: "event".into(),
                trigger_id: "new_issue".into(),
                repository: "acme/repo".into(),
                number: 11,
                title: title.into(),
                actor: "alice".into(),
                url: "https://private.test".into(),
                status: status.into(),
                current_step: None,
                created_at: "now".into(),
                started_at: None,
                finished_at: None,
                error: error.map(|s| s.to_owned()),
                attempt: 2,
                workspace: Some("PRIVATE_PATH".into()),
            },
        }
    }
    #[test]
    fn notification_state_matrix_only_reports_lifecycle_edges() {
        let states = [
            "queued",
            "running",
            "succeeded",
            "failed",
            "blocked",
            "cancelled",
            "interrupted",
            "unknown",
        ];
        for previous in states {
            for status in states {
                let actual = notification_for(
                    &transition(previous, status, "Task-based work", None),
                    |s| s.to_owned(),
                );
                let expected = matches!(
                    (previous, status),
                    ("queued", "running" | "succeeded" | "failed" | "blocked")
                        | ("running", "succeeded" | "failed" | "blocked")
                );
                assert_eq!(actual.is_some(), expected, "{previous} -> {status}");
                if let Some(m) = actual {
                    assert_eq!(m.run_id, "run-alpha");
                    assert_eq!(m.attempt, 2);
                    assert_eq!(m.body, "acme/repo #11 · Task-based work");
                }
            }
        }
    }
    #[test]
    fn notification_external_error_never_reaches_os() {
        let error="Authorization: Bearer EXTERNAL_PRIVATE_SENTINEL\r\nTOKEN=another-private-value https://user:pass@private.test {\"secret\":\"JSON_PRIVATE\"}\ntrace private/log";
        let m = notification_for(
            &transition("running", "failed", "Title", Some(error)),
            |s| s.to_owned(),
        )
        .unwrap();
        assert_eq!(
            m.body,
            "acme/repo #11 · Title · Execution reported an error. See run details."
        );
        for secret in [
            "EXTERNAL_PRIVATE_SENTINEL",
            "another-private-value",
            "user:pass",
            "JSON_PRIVATE",
            "private/log",
            "PRIVATE_PATH",
        ] {
            assert!(!m.body.contains(secret));
        }
    }
    #[test]
    fn notification_marked_titles_are_hidden() {
        for title in [
            "ghp_PRIVATE",
            "github_pat_PRIVATE",
            "prefix GHO_private",
            "ghu_private",
            "ghs_private",
            "ghr_private",
            "sk-private",
            "prefix sk-private",
            "Authorization: private",
            "Bearer private",
            "password=private",
            "TOKEN=private",
        ] {
            let m = notification_for(&transition("queued", "running", title, None), |s| {
                s.to_owned()
            })
            .unwrap();
            assert_eq!(m.body, "acme/repo #11 · [Title hidden]", "{title}");
        }
    }
    #[test]
    fn notification_normalized_credential_markers_are_hidden_before_truncation() {
        for title in [
            "Bearer\nPRIVATE",
            "to\u{202e}ken=PRIVATE",
            "gh\u{0007}p_PRIVATE",
            "prefix\tSK-private",
        ] {
            let m = notification_for(&transition("queued", "running", title, None), |s| {
                s.to_owned()
            })
            .unwrap();
            assert_eq!(m.body, "acme/repo #11 · [Title hidden]");
        }
    }
    #[test]
    fn notification_redaction_precedes_truncation_and_preserves_unicode() {
        for title in [
            format!("{}PRIVATE_TOKEN tail", "é🦀".repeat(58)),
            "PRIVATE_TOKEN start".into(),
            format!("{}PRIVATE_TOKEN", "x".repeat(150)),
        ] {
            let m = notification_for(&transition("queued", "running", &title, None), |s| {
                s.replace("PRIVATE_TOKEN", "[redacted]")
            })
            .unwrap();
            assert!(!m.body.contains("PRIVATE"));
            assert!(m.body.chars().count() <= 360);
            assert!(
                m.body
                    .strip_prefix("acme/repo #11 · ")
                    .unwrap()
                    .chars()
                    .count()
                    <= 120
            );
        }
        let m = notification_for(
            &transition("queued", "running", &"é🦀".repeat(500), None),
            |s| s.to_owned(),
        )
        .unwrap();
        assert!(m.body.ends_with('…'));
        assert_eq!(
            m.body
                .strip_prefix("acme/repo #11 · ")
                .unwrap()
                .chars()
                .count(),
            120
        );
    }
    #[test]
    fn notification_visible_text_is_plain_single_line() {
        let m = notification_for(
            &transition(
                "queued",
                "running",
                "  Hola\r\n<bold>& \u{202e}🦀\u{2066}\u{0007}  fin  ",
                None,
            ),
            |s| s.to_owned(),
        )
        .unwrap();
        assert_eq!(m.body, "acme/repo #11 · Hola bold 🦀 fin");
        assert!(!m.body.chars().any(char::is_control));
        let m = notification_for(
            &transition("queued", "running", " \n<>&\u{202e}", None),
            |s| s.to_owned(),
        )
        .unwrap();
        assert_eq!(m.body, "acme/repo #11 · (untitled)");
    }
    #[test]
    fn notification_does_not_show_residual_error_on_success_or_start() {
        for (previous, status) in [("queued", "running"), ("running", "succeeded")] {
            let m = notification_for(
                &transition(previous, status, "Title", Some("timeout PRIVATE")),
                |s| s.to_owned(),
            )
            .unwrap();
            assert_eq!(m.body, "acme/repo #11 · Title");
        }
    }
}
