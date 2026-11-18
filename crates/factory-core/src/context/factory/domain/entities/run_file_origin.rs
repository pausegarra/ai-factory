#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunFileOrigin {
    Local,
    Imported,
}
pub(crate) fn local_operational_owner(key: &str) -> Option<&str> {
    for prefix in [
        "identity:",
        "pr-workspace:",
        "version-workspace:",
        "release-workspace:",
        "pr-release:",
    ] {
        if let Some(id) = key.strip_prefix(prefix) {
            return Some(id);
        }
    }
    for prefix in ["review-feedback:", "review-history:"] {
        if let Some(rest) = key.strip_prefix(prefix) {
            return rest.split(':').next();
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn run_file_origin_operational_owners_are_exact() {
        for prefix in [
            "identity",
            "pr-workspace",
            "version-workspace",
            "release-workspace",
            "pr-release",
        ] {
            assert_eq!(
                local_operational_owner(&format!("{prefix}:owner-neighbor")),
                Some("owner-neighbor")
            );
        }
        for prefix in ["review-feedback", "review-history"] {
            assert_eq!(
                local_operational_owner(&format!("{prefix}:owner:builder")),
                Some("owner")
            );
        }
        for key in [
            "review:global",
            "action:hash",
            "pr-update:owner",
            "approval-comment:acme/repo#1:1",
        ] {
            assert_eq!(local_operational_owner(key), None);
        }
    }
}
