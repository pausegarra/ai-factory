use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use zeroize::Zeroizing;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationV1 {
    pub settings: Settings,
    pub credentials: Vec<CredentialDescriptor>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialDescriptor {
    pub identity: CredentialIdentity,
    pub source_reference: Option<String>,
    pub available: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRowV1 {
    pub detail: RunDetail,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DedupRowV1 {
    pub event_id: String,
    pub run_id: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationRowV1 {
    pub key: String,
    pub value: Value,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrOriginRowV1 {
    pub repository: String,
    pub number: u64,
    pub origin_key: String,
    pub event_id: Option<String>,
    pub issue_repository: Option<String>,
    pub issue_number: Option<u64>,
    pub actor: Option<String>,
    pub trigger_id: Option<String>,
    pub trusted: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentRowV1 {
    pub key: String,
    pub run_id: String,
    pub node_id: String,
    pub revision: u32,
    pub snapshot: Value,
    pub state: String,
    pub failures: u32,
    pub next_at: String,
    pub last_error: Option<String>,
    pub receipt: Option<Value>,
}
#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HistoryV1 {
    pub runs: Vec<RunRowV1>,
    pub dedup: Vec<DedupRowV1>,
    pub observations: Vec<ObservationRowV1>,
    pub pr_origins: Vec<PrOriginRowV1>,
    pub comments: Vec<CommentRowV1>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncryptionV1 {
    pub algorithm: String,
    pub kdf: String,
    pub argon_version: u32,
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
    pub salt: [u8; 16],
    pub nonce: [u8; 12],
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentDescriptorV1 {
    pub path: String,
    pub format_version: u32,
    pub size_bytes: u64,
    pub sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestV1 {
    pub format: String,
    pub version: u32,
    pub archive_id: String,
    pub created_at: String,
    pub components: Vec<BackupComponent>,
    pub entries: BTreeMap<BackupComponent, ComponentDescriptorV1>,
    pub encryption: Option<EncryptionV1>,
}
#[derive(Serialize)]
pub struct ArchiveBindingV1<'a> {
    pub format: &'a str,
    pub version: u32,
    pub archive_id: &'a str,
    pub components: &'a [BackupComponent],
    pub encryption: &'a Option<EncryptionV1>,
    pub configuration: Option<&'a ComponentDescriptorV1>,
    pub history: Option<&'a ComponentDescriptorV1>,
}
impl ManifestV1 {
    pub fn aad(&self) -> std::result::Result<Vec<u8>, BackupFailure> {
        serde_json::to_vec(&ArchiveBindingV1 {
            format: &self.format,
            version: self.version,
            archive_id: &self.archive_id,
            components: &self.components,
            encryption: &self.encryption,
            configuration: self.entries.get(&BackupComponent::Configuration),
            history: self.entries.get(&BackupComponent::History),
        })
        .map_err(|_| BackupFailure::invalid_archive())
    }
}
pub struct SecretRecord {
    pub identity: CredentialIdentity,
    pub source_reference: Option<String>,
    pub value: Zeroizing<String>,
}
#[derive(Default)]
pub struct CredentialsV1 {
    pub records: Vec<SecretRecord>,
    pub missing: Vec<CredentialIdentity>,
}
pub type SecretBundle = CredentialsV1;
pub struct BackupSnapshot {
    pub configuration: Option<ConfigurationV1>,
    pub history: Option<HistoryV1>,
    pub credentials: Option<SecretBundle>,
    pub redaction_secrets: Vec<Zeroizing<String>>,
    pub warnings: Vec<BackupWarning>,
}
pub struct ValidatedBackup {
    pub inspection: BackupInspection,
    pub manifest: ManifestV1,
    pub configuration: Option<ConfigurationV1>,
    pub history: Option<HistoryV1>,
    pub credentials: Option<SecretBundle>,
    pub warnings: Vec<BackupWarning>,
}
pub struct RestorePlan {
    pub archive_sha256: String,
    pub selection: BackupSelection,
    pub effective_configuration: Option<ConfigurationV1>,
    pub normalized_history: Option<HistoryV1>,
    pub credential_updates: SecretBundle,
    pub warnings: Vec<BackupWarning>,
}

pub(crate) fn counts(
    config: Option<&ConfigurationV1>,
    history: Option<&HistoryV1>,
    credentials: Option<&SecretBundle>,
) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    if let Some(c) = config {
        out.insert("profiles".into(), c.settings.token_profiles.len() as u64);
        out.insert(
            "connectors".into(),
            c.settings.notification_connectors.len() as u64,
        );
    }
    if let Some(h) = history {
        out.insert("runs".into(), h.runs.len() as u64);
        out.insert(
            "nodes".into(),
            h.runs.iter().map(|r| r.detail.nodes.len() as u64).sum(),
        );
    }
    if let Some(c) = credentials {
        out.insert("credentials".into(), c.records.len() as u64);
    }
    out
}
