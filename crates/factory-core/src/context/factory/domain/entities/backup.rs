use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BackupComponent {
    Configuration,
    History,
    Credentials,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "SelectionInput")]
pub struct BackupSelection {
    pub components: Vec<BackupComponent>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectionInput {
    components: Vec<BackupComponent>,
}
#[derive(Debug)]
pub struct BackupRuleError;
impl std::fmt::Display for BackupRuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Choose distinct backup categories available in the archive")
    }
}
impl std::error::Error for BackupRuleError {}
impl TryFrom<SelectionInput> for BackupSelection {
    type Error = BackupRuleError;
    fn try_from(input: SelectionInput) -> Result<Self, Self::Error> {
        Self::new(input.components)
    }
}
impl BackupSelection {
    pub fn new(mut components: Vec<BackupComponent>) -> Result<Self, BackupRuleError> {
        if components.is_empty() || components.len() > 3 {
            return Err(BackupRuleError);
        }
        components.sort();
        if components.windows(2).any(|w| w[0] == w[1]) {
            return Err(BackupRuleError);
        }
        Ok(Self { components })
    }
    pub fn validate_present(&self, present: &[BackupComponent]) -> Result<(), BackupRuleError> {
        if self.components.iter().all(|v| present.contains(v)) {
            Ok(())
        } else {
            Err(BackupRuleError)
        }
    }
    pub fn contains(&self, c: BackupComponent) -> bool {
        self.components.contains(&c)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialIdentity {
    Github { profile_id: String, account: String },
    Telegram { connector_id: String },
    GoogleChat { connector_id: String },
}
pub fn valid_backup_run_id(id: &str) -> bool {
    super::run_deletion::valid_deletion_id(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_is_independent_and_selection_cannot_be_empty() {
        assert!(BackupSelection::new(vec![]).is_err());
        let selection = BackupSelection::new(vec![BackupComponent::History]).unwrap();
        assert_eq!(selection.components, vec![BackupComponent::History]);
        assert!(selection
            .validate_present(&[BackupComponent::History])
            .is_ok());
        assert!(selection
            .validate_present(&[BackupComponent::Configuration])
            .is_err());
        assert!(
            BackupSelection::new(vec![BackupComponent::History, BackupComponent::History]).is_err()
        );
    }
    #[test]
    fn backup_accepts_modern_and_legacy_ids_without_rewriting_them() {
        for id in [
            "89a5ec1c-b045-4e2b-a3aa-253d51a05e0f",
            "legacy-0123456789abcdef0123456789abcdef",
        ] {
            assert!(valid_backup_run_id(id));
        }
        for id in [
            "legacy-0123",
            "legacy-0123456789ABCDEF0123456789ABCDEF",
            "89A5EC1C-B045-4E2B-A3AA-253D51A05E0F",
            "../legacy-0123456789abcdef0123456789abcdef",
        ] {
            assert!(!valid_backup_run_id(id));
        }
    }
    #[test]
    fn backup_all_selections_are_canonical_and_ipc_cannot_skip_validation() {
        use BackupComponent::*;
        for mask in 1..8 {
            let components = [Configuration, History, Credentials]
                .into_iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, v)| v)
                .collect::<Vec<_>>();
            let selection = BackupSelection::new(components.clone()).unwrap();
            assert_eq!(selection.components, components);
            assert!(selection
                .validate_present(&[Configuration, History, Credentials])
                .is_ok());
        }
        assert!(serde_json::from_str::<BackupSelection>(r#"{"components":[]}"#).is_err());
        assert!(
            serde_json::from_str::<BackupSelection>(r#"{"components":["history","history"]}"#)
                .is_err()
        );
    }
}
