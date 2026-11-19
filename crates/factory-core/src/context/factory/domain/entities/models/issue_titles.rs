use anyhow::{ensure, Result};

pub(crate) const DEFAULT_ISSUE_TITLE_PREFIX: &str = "[AI]";

pub(crate) fn normalize_issue_title_prefix(prefix: &str) -> Result<String> {
    let prefix = prefix.trim();
    ensure!(!prefix.is_empty(), "Issue title prefix cannot be empty.");
    Ok(prefix.to_owned())
}

pub(crate) fn is_release_issue_title(title: &str) -> bool {
    title.starts_with("[Release]")
}

pub(crate) fn issue_title_matches_trigger(
    title: &str,
    trigger: &str,
    issue_title_prefix: &str,
) -> bool {
    match trigger {
        "new_issue" => {
            let prefix = issue_title_prefix.trim();
            !prefix.is_empty() && title.trim().starts_with(prefix)
        }
        "release" => is_release_issue_title(title),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_prefix_literal_case_start_trim_and_empty_guard() {
        for (prefix, title, expected) in [
            ("[AI]", "[AI]", true),
            ("[AI]", "[AI]tarea", true),
            ("[AI]", "[AI] tarea", true),
            ("[AI]", " \t[AI] tarea \n", true),
            ("[AI]", "[ai] tarea", false),
            ("[AI]", "Nota [AI] tarea", false),
            ("bot:", "bot: tarea", true),
            ("bot:", "[AI] tarea", false),
            ("BOT:", "bot: tarea", false),
            ("BOT:", "BOT:tarea", true),
            (" Team Bot: ", "\tTeam Bot: tarea\n", true),
            ("Team Bot:", "Team  Bot: tarea", false),
            ("[.*]", "[.*]literal", true),
            ("[.*]", "anything", false),
            ("", "anything", false),
            (" \t\n", "[AI]", false),
            ("\u{2003}bot:\u{2003}", "\u{2003}bot: tarea\u{2003}", true),
        ] {
            assert_eq!(
                issue_title_matches_trigger(title, "new_issue", prefix),
                expected,
                "{prefix:?} {title:?}"
            );
        }
    }

    #[test]
    fn issue_prefix_never_changes_release_and_allows_independent_overlap() {
        for prefix in ["[AI]", "bot:", "[", "[Release]", ""] {
            for (title, expected) in [
                ("[Release]", true),
                ("[Release]2", true),
                (" [Release]", false),
                ("[release]", false),
                ("x [Release]", false),
                ("[AI]", false),
            ] {
                assert_eq!(
                    issue_title_matches_trigger(title, "release", prefix),
                    expected
                );
            }
        }
        assert!(issue_title_matches_trigger("[Release] 2", "new_issue", "["));
        assert!(issue_title_matches_trigger("[Release] 2", "release", "["));
        assert!(!issue_title_matches_trigger("[AI]", "pr_review", "[AI]"));
    }
}
