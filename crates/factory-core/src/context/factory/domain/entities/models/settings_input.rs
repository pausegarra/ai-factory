use super::{Commands, Flow, Project, Settings, TokenProfile};
use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;

// Legacy lists are accepted only at this input boundary. Canonical settings
// always serialize scopes on each profile, including explicit empty lists.
#[derive(Deserialize)]
#[serde(default)]
pub(super) struct SettingsInput {
    owners: Vec<String>,
    authors: Vec<String>,
    reviewers: Vec<String>,
    token_profiles: Vec<TokenProfileInput>,
    codex_bindings: BTreeMap<String, String>,
    projects: Vec<Project>,
    commands: Commands,
    poll_interval_seconds: u64,
    max_concurrent: usize,
    start_at_login: bool,
    flows: Vec<Flow>,
}

#[derive(Deserialize)]
struct TokenProfileInput {
    #[serde(flatten)]
    profile: TokenProfile,
    #[serde(default, deserialize_with = "present_scope")]
    owners: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present_scope")]
    authors: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present_scope")]
    reviewers: Option<Vec<String>>,
}

fn present_scope<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error> {
    // Missing fields may inherit legacy values; explicit null is invalid input.
    Vec::<String>::deserialize(deserializer).map(Some)
}

impl Default for SettingsInput {
    fn default() -> Self {
        let defaults = Settings::default();
        Self {
            owners: vec![],
            authors: vec![],
            reviewers: vec![],
            token_profiles: vec![],
            codex_bindings: defaults.codex_bindings,
            projects: defaults.projects,
            commands: defaults.commands,
            poll_interval_seconds: defaults.poll_interval_seconds,
            max_concurrent: defaults.max_concurrent,
            start_at_login: defaults.start_at_login,
            flows: defaults.flows,
        }
    }
}

impl From<SettingsInput> for Settings {
    fn from(input: SettingsInput) -> Self {
        let token_profiles = input
            .token_profiles
            .into_iter()
            .map(|token| {
                let mut profile = token.profile;
                profile.apply_legacy_scope(&input.owners, &input.authors, &input.reviewers);
                if let Some(owners) = token.owners {
                    profile.owners = owners;
                }
                if let Some(authors) = token.authors {
                    profile.authors = authors;
                }
                if let Some(reviewers) = token.reviewers {
                    profile.reviewers = reviewers;
                }
                profile
            })
            .collect();
        Self {
            token_profiles,
            codex_bindings: input.codex_bindings,
            projects: input.projects,
            commands: input.commands,
            poll_interval_seconds: input.poll_interval_seconds,
            max_concurrent: input.max_concurrent,
            start_at_login: input.start_at_login,
            flows: input.flows,
        }
    }
}
