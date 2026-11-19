use anyhow::{ensure, Result};

pub(crate) const DEFAULT_ISSUE_TITLE_PREFIX: &str = "[AI]";

pub(crate) const DEFAULT_RELEASE_ISSUE_TITLE_PREFIX: &str = "[RELEASE]";

pub(crate) fn normalize_release_issue_title_prefix(prefix: &str) -> Result<String> {
    let prefix = prefix.trim();
    ensure!(
        !prefix.is_empty(),
        "Release issue title prefix cannot be empty."
    );
    Ok(prefix.to_owned())
}

pub(crate) fn normalize_issue_title_prefix(prefix: &str) -> Result<String> {
    let prefix = prefix.trim();
    ensure!(!prefix.is_empty(), "Issue title prefix cannot be empty.");
    Ok(prefix.to_owned())
}

fn title_starts_with_prefix(title: &str, prefix: &str) -> bool {
    let prefix = prefix.trim();
    !prefix.is_empty() && title.trim().starts_with(prefix)
}

pub(crate) fn is_release_issue_title(title: &str, release_issue_title_prefix: &str) -> bool {
    title_starts_with_prefix(title, release_issue_title_prefix)
}

pub(crate) fn issue_title_matches_trigger(
    title: &str,
    trigger: &str,
    issue_title_prefix: &str,
    release_issue_title_prefix: &str,
) -> bool {
    match trigger {
        "new_issue" => title_starts_with_prefix(title, issue_title_prefix),
        "release" => is_release_issue_title(title, release_issue_title_prefix),
        _ => false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_default_changes_case_and_trims_title() {
        assert!(issue_title_matches_trigger(
            " \t[RELEASE]2\n",
            "release",
            "task:",
            DEFAULT_RELEASE_ISSUE_TITLE_PREFIX
        ));
        assert!(!issue_title_matches_trigger(
            "[Release]2",
            "release",
            "task:",
            DEFAULT_RELEASE_ISSUE_TITLE_PREFIX
        ));
    }

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
                issue_title_matches_trigger(
                    title,
                    "new_issue",
                    prefix,
                    DEFAULT_RELEASE_ISSUE_TITLE_PREFIX
                ),
                expected,
                "{prefix:?} {title:?}"
            );
        }
    }

    #[test]
    fn release_prefix_literal_case_start_trim_and_empty_guard() {
        for (prefix, title, expected) in [
            ("[RELEASE]", "[RELEASE]", true),
            ("[RELEASE]", "[RELEASE]2", true),
            ("[RELEASE]", " \t[RELEASE] 2\n", true),
            ("[RELEASE]", "[release] 2", false),
            ("[RELEASE]", "[Release] 2", false),
            ("[RELEASE]", "note [RELEASE] 2", false),
            (" ship: ", "\tship:2\n", true),
            ("Ship:", "ship:2", false),
            ("Team Release:", "Team  Release: 2", false),
            ("[.*]", "[.*]literal", true),
            ("[.*]", "anything", false),
            ("", "anything", false),
            (" \t\n", "[RELEASE]", false),
            ("\u{2003}ship:\u{2003}", "\u{2003}ship:2\u{2003}", true),
        ] {
            assert_eq!(
                is_release_issue_title(title, prefix),
                expected,
                "{prefix:?} {title:?}"
            );
            assert_eq!(
                issue_title_matches_trigger(title, "release", "task:", prefix),
                expected
            );
        }
    }

    #[test]
    fn two_prefixes_are_independent_allow_overlap_and_do_not_select_pr_triggers() {
        for issue_prefix in ["[AI]", "task:", "[", ""] {
            assert!(issue_title_matches_trigger(
                "ship:2",
                "release",
                issue_prefix,
                "ship:"
            ));
            assert!(!issue_title_matches_trigger(
                "[AI] task",
                "release",
                issue_prefix,
                "ship:"
            ));
        }
        for release_prefix in ["[RELEASE]", "ship:", "[", ""] {
            assert!(issue_title_matches_trigger(
                "task:work",
                "new_issue",
                "task:",
                release_prefix
            ));
            assert!(!issue_title_matches_trigger(
                "ship:work",
                "new_issue",
                "task:",
                release_prefix
            ));
        }
        for trigger in ["pr_review", "pr_approved", "unknown"] {
            assert!(!issue_title_matches_trigger(
                "ship:work",
                trigger,
                "ship:",
                "ship:"
            ));
        }
        assert!(issue_title_matches_trigger(
            "[RELEASE]2",
            "new_issue",
            "[",
            "[RELEASE]"
        ));
        assert!(issue_title_matches_trigger(
            "[RELEASE]2",
            "release",
            "[",
            "[RELEASE]"
        ));
    }
}
