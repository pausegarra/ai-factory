use crate::context::factory::application::{
    dto::backup_format::*, interfaces::backup_archive::BackupArchive,
};
use crate::context::factory::infrastructure::security::backup_crypto::*;
use crate::*;
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read, Write},
    path::Path,
};
type Result<T> = std::result::Result<T, BackupFailure>;
pub(crate) struct ZipBackupArchive;
const MAX_ARCHIVE: u64 = 160 * 1024 * 1024;
fn path_for(c: BackupComponent) -> &'static str {
    match c {
        BackupComponent::Configuration => "configuration.json",
        BackupComponent::History => "history.json",
        BackupComponent::Credentials => "credentials.bin",
    }
}
fn limit(name: &str) -> Result<u64> {
    match name {
        "manifest.json" => Ok(64 * 1024),
        "configuration.json" => Ok(8 * 1024 * 1024),
        "history.json" => Ok(128 * 1024 * 1024),
        "credentials.bin" => Ok(8 * 1024 * 1024 + 16),
        _ => Err(BackupFailure::invalid_archive()),
    }
}
pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn u16_at(b: &[u8], p: usize) -> Result<usize> {
    Ok(u16::from_le_bytes(
        b.get(p..p + 2)
            .ok_or_else(BackupFailure::invalid_archive)?
            .try_into()
            .map_err(|_| BackupFailure::invalid_archive())?,
    ) as usize)
}
fn u32_at(b: &[u8], p: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(
        b.get(p..p + 4)
            .ok_or_else(BackupFailure::invalid_archive)?
            .try_into()
            .map_err(|_| BackupFailure::invalid_archive())?,
    ) as usize)
}
fn require(ok: bool) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(BackupFailure::invalid_archive())
    }
}
// Validate every physical entry before the ZIP library builds its name index.
fn preflight(b: &[u8]) -> Result<()> {
    require(b.len() >= 22)?;
    let end = b.len() - 22;
    require(
        u32_at(b, end)? == 0x06054b50
            && u16_at(b, end + 20)? == 0
            && u16_at(b, end + 4)? == 0
            && u16_at(b, end + 6)? == 0,
    )?;
    let count = u16_at(b, end + 10)?;
    let size = u32_at(b, end + 12)?;
    let start = u32_at(b, end + 16)?;
    require(
        (2..=4).contains(&count)
            && count == u16_at(b, end + 8)?
            && size <= 65536
            && start.checked_add(size) == Some(end),
    )?;
    let mut central = start;
    let mut local_end = 0;
    let mut names = BTreeSet::new();
    for _ in 0..count {
        require(central + 46 <= end && u32_at(b, central)? == 0x02014b50)?;
        let flags = u16_at(b, central + 8)?;
        let method = u16_at(b, central + 10)?;
        let compressed = u32_at(b, central + 20)?;
        let real = u32_at(b, central + 24)?;
        let n = u16_at(b, central + 28)?;
        let extra = u16_at(b, central + 30)?;
        let comment = u16_at(b, central + 32)?;
        let offset = u32_at(b, central + 42)?;
        let attributes = u32_at(b, central + 38)?;
        require(attributes & 0x10 == 0)?;
        let mode = attributes >> 16;
        let kind = mode & 0o170000;
        require(
            flags & !0x800 == 0
                && method == 0
                && compressed == real
                && extra == 0
                && comment == 0
                && u16_at(b, central + 34)? == 0
                && (kind == 0 || kind == 0o100000)
                && u16_at(b, central + 6)? <= 20,
        )?;
        let name = std::str::from_utf8(
            b.get(central + 46..central + 46 + n)
                .ok_or_else(BackupFailure::invalid_archive)?,
        )
        .map_err(|_| BackupFailure::invalid_archive())?;
        require(name.is_ascii() && names.insert(name.to_owned()) && real as u64 <= limit(name)?)?;
        require(offset == local_end && offset + 30 <= start && u32_at(b, offset)? == 0x04034b50)?;
        require(
            u16_at(b, offset + 4)? <= 20
                && flags == u16_at(b, offset + 6)?
                && method == u16_at(b, offset + 8)?
                && u32_at(b, offset + 14)? == u32_at(b, central + 16)?
                && compressed == u32_at(b, offset + 18)?
                && real == u32_at(b, offset + 22)?
                && n == u16_at(b, offset + 26)?
                && u16_at(b, offset + 28)? == 0,
        )?;
        require(b.get(offset + 30..offset + 30 + n) == Some(name.as_bytes()))?;
        local_end = offset
            .checked_add(30 + n)
            .and_then(|p| p.checked_add(real))
            .ok_or_else(BackupFailure::invalid_archive)?;
        require(local_end <= start)?;
        central += 46 + n;
    }
    require(central == end && local_end == start && names.contains("manifest.json"))
}
fn bounded_json(bytes: &[u8]) -> Result<serde_json::Value> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| BackupFailure::invalid_archive())?;
    fn walk(v: &serde_json::Value, depth: usize) -> Result<()> {
        if depth > 128 {
            return Err(BackupFailure::limit_exceeded());
        }
        match v {
            serde_json::Value::String(s) => require(s.len() <= 1024 * 1024)?,
            serde_json::Value::Array(a) => {
                for v in a {
                    walk(v, depth + 1)?
                }
            }
            serde_json::Value::Object(o) => {
                for (k, v) in o {
                    require(k.len() <= 1024 * 1024)?;
                    walk(v, depth + 1)?
                }
            }
            _ => (),
        };
        Ok(())
    }
    walk(&value, 0)?;
    Ok(value)
}
fn check_keys(raw: &serde_json::Value, canonical: &serde_json::Value, field: &str) -> Result<()> {
    if matches!(
        field,
        "payload"
            | "result"
            | "result_schema"
            | "snapshot"
            | "receipt"
            | "value"
            | "codex_bindings"
            | "profile_activation"
    ) {
        return Ok(());
    }
    if let (Some(a), Some(b)) = (raw.as_object(), canonical.as_object()) {
        for (k, v) in a {
            if matches!(k.as_str(), "token" | "bot_token" | "webhook_url") {
                require(v.is_null())?;
                continue;
            }
            let expected = b.get(k).ok_or_else(BackupFailure::invalid_archive)?;
            check_keys(v, expected, k)?;
        }
    } else if let (Some(a), Some(b)) = (raw.as_array(), canonical.as_array()) {
        require(a.len() == b.len())?;
        for (a, b) in a.iter().zip(b) {
            check_keys(a, b, field)?;
        }
    }
    Ok(())
}
fn decode<T: DeserializeOwned + Serialize>(bytes: &[u8]) -> Result<T> {
    let raw = bounded_json(bytes)?;
    let data: T = serde_json::from_slice(bytes).map_err(|_| BackupFailure::invalid_archive())?;
    let canonical = serde_json::to_value(&data).map_err(|_| BackupFailure::invalid_archive())?;
    check_keys(&raw, &canonical, "")?;
    Ok(data)
}
fn public_bytes<T: Serialize>(data: &T, secrets: &[zeroize::Zeroizing<String>]) -> Result<Vec<u8>> {
    let mut value = serde_json::to_value(data).map_err(|_| BackupFailure::invalid_archive())?;
    redact_portable_value(&mut value, secrets)?;
    serde_json::to_vec(&value).map_err(|_| BackupFailure::invalid_archive())
}
impl ZipBackupArchive {
    fn load(
        &self,
        path: &Path,
    ) -> Result<(BackupInspection, ManifestV1, BTreeMap<String, Vec<u8>>)> {
        let file = std::fs::File::open(path).map_err(|_| BackupFailure::invalid_archive())?;
        require(
            file.metadata()
                .map_err(|_| BackupFailure::invalid_archive())?
                .is_file(),
        )?;
        let mut bytes = Vec::new();
        file.take(MAX_ARCHIVE + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| BackupFailure::invalid_archive())?;
        if bytes.len() as u64 > MAX_ARCHIVE {
            return Err(BackupFailure::limit_exceeded());
        }
        preflight(&bytes)?;
        let hash = digest(&bytes);
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|_| BackupFailure::invalid_archive())?;
        let mut entries = BTreeMap::new();
        for i in 0..zip.len() {
            let mut entry = zip
                .by_index(i)
                .map_err(|_| BackupFailure::invalid_archive())?;
            let name = entry
                .name()
                .map_err(|_| BackupFailure::invalid_archive())?
                .into_owned();
            let max = limit(&name)?;
            let mut data = Vec::new();
            entry
                .by_ref()
                .take(max + 1)
                .read_to_end(&mut data)
                .map_err(|_| BackupFailure::invalid_archive())?;
            require(data.len() as u64 <= max && data.len() as u64 == entry.size())?;
            entries.insert(name, data);
        }
        let manifest: ManifestV1 = decode(
            entries
                .get("manifest.json")
                .ok_or_else(BackupFailure::invalid_archive)?,
        )?;
        require(manifest.format == "ai-factory-backup")?;
        if manifest.version != 1 {
            return Err(BackupFailure::new(BackupErrorCode::UnsupportedVersion));
        }
        require(
            uuid::Uuid::parse_str(&manifest.archive_id)
                .is_ok_and(|v| v.to_string() == manifest.archive_id)
                && chrono::DateTime::parse_from_rfc3339(&manifest.created_at).is_ok(),
        )?;
        let selection = BackupSelection::new(manifest.components.clone())?;
        require(
            selection.components == manifest.components
                && manifest.entries.len() == selection.components.len()
                && entries.len() == selection.components.len() + 1,
        )?;
        for c in &selection.components {
            let descriptor = manifest
                .entries
                .get(c)
                .ok_or_else(BackupFailure::invalid_archive)?;
            require(descriptor.path == path_for(*c) && descriptor.format_version == 1)?;
            let data = entries
                .get(&descriptor.path)
                .ok_or_else(BackupFailure::invalid_archive)?;
            require(
                descriptor.size_bytes == data.len() as u64
                    && descriptor.sha256.eq_ignore_ascii_case(&digest(data)),
            )?;
        }
        require(selection.contains(BackupComponent::Credentials) == manifest.encryption.is_some())?;
        if let Some(e) = &manifest.encryption {
            e.validate()?;
        }
        if let Some(data) = entries.get("configuration.json") {
            let config: ConfigurationV1 = decode(data)?;
            validate_configuration(&config)?;
        }
        if let Some(data) = entries.get("history.json") {
            let history: HistoryV1 = decode(data)?;
            validate_history(&history)?;
        }
        let inspection = BackupInspection {
            archive_sha256: hash,
            archive_id: manifest.archive_id.clone(),
            components: manifest.components.clone(),
            encrypted_credentials: manifest.encryption.is_some(),
        };
        Ok((inspection, manifest, entries))
    }
}
impl BackupArchive for ZipBackupArchive {
    fn inspect(&self, path: &Path) -> Result<BackupInspection> {
        Ok(self.load(path)?.0)
    }
    fn read(
        &self,
        path: &Path,
        selection: &BackupSelection,
        password: Option<&str>,
    ) -> Result<ValidatedBackup> {
        let (inspection, manifest, mut entries) = self.load(path)?;
        selection.validate_present(&inspection.components)?;
        let configuration = if selection.contains(BackupComponent::Configuration) {
            Some(decode(
                &entries
                    .remove("configuration.json")
                    .ok_or_else(BackupFailure::invalid_archive)?,
            )?)
        } else {
            None
        };
        let history = if selection.contains(BackupComponent::History) {
            Some(decode(
                &entries
                    .remove("history.json")
                    .ok_or_else(BackupFailure::invalid_archive)?,
            )?)
        } else {
            None
        };
        let credentials = if selection.contains(BackupComponent::Credentials) {
            let e = manifest
                .encryption
                .as_ref()
                .ok_or_else(BackupFailure::invalid_archive)?;
            let plain = decrypt_credentials(
                entries
                    .get("credentials.bin")
                    .ok_or_else(BackupFailure::invalid_archive)?,
                password.ok_or_else(BackupFailure::invalid_password)?,
                e,
                &manifest.aad()?,
            )?;
            Some(decode_secrets(&plain)?)
        } else {
            None
        };
        Ok(ValidatedBackup {
            inspection,
            manifest,
            configuration,
            history,
            credentials,
            warnings: vec![],
        })
    }
    fn write(
        &self,
        path: &Path,
        selection: &BackupSelection,
        snapshot: BackupSnapshot,
        password: Option<&str>,
    ) -> Result<BackupOutcome> {
        if path.symlink_metadata().is_ok() {
            return Err(BackupFailure::new(BackupErrorCode::DestinationExists));
        }
        let parent = path
            .parent()
            .ok_or_else(BackupFailure::persistence_failed)?;
        let mut entries = BTreeMap::new();
        let counts = counts(
            snapshot.configuration.as_ref(),
            snapshot.history.as_ref(),
            snapshot.credentials.as_ref(),
        );
        if let Some(config) = &snapshot.configuration {
            entries.insert(
                BackupComponent::Configuration,
                public_bytes(config, &snapshot.redaction_secrets)?,
            );
        }
        if let Some(history) = &snapshot.history {
            entries.insert(
                BackupComponent::History,
                public_bytes(history, &snapshot.redaction_secrets)?,
            );
        }
        let mut manifest = ManifestV1 {
            format: "ai-factory-backup".into(),
            version: 1,
            archive_id: uuid::Uuid::new_v4().to_string(),
            created_at: now(),
            components: selection.components.clone(),
            entries: BTreeMap::new(),
            encryption: None,
        };
        for (c, data) in &entries {
            manifest.entries.insert(
                *c,
                ComponentDescriptorV1 {
                    path: path_for(*c).into(),
                    format_version: 1,
                    size_bytes: data.len() as u64,
                    sha256: digest(data),
                },
            );
        }
        if let Some(credentials) = &snapshot.credentials {
            manifest.encryption = Some(EncryptionV1::fresh()?);
            let data = encrypt_credentials(
                &encode_secrets(credentials)?,
                password.ok_or_else(BackupFailure::invalid_password)?,
                manifest.encryption.as_ref().unwrap(),
                &manifest.aad()?,
            )?;
            manifest.entries.insert(
                BackupComponent::Credentials,
                ComponentDescriptorV1 {
                    path: "credentials.bin".into(),
                    format_version: 1,
                    size_bytes: data.len() as u64,
                    sha256: digest(&data),
                },
            );
            entries.insert(BackupComponent::Credentials, data);
        }
        require(
            manifest.entries.len() == selection.components.len()
                && selection
                    .components
                    .iter()
                    .all(|c| manifest.entries.contains_key(c)),
        )?;
        let manifest_bytes =
            serde_json::to_vec(&manifest).map_err(|_| BackupFailure::invalid_archive())?;
        let total = entries.values().map(Vec::len).sum::<usize>() + manifest_bytes.len();
        if total as u64 > MAX_ARCHIVE {
            return Err(BackupFailure::limit_exceeded());
        }
        for (c, data) in &entries {
            if data.len() as u64 > limit(path_for(*c))? {
                return Err(BackupFailure::limit_exceeded());
            }
        }
        let mut temp = tempfile::NamedTempFile::new_in(parent)
            .map_err(|_| BackupFailure::persistence_failed())?;
        {
            let mut zip = zip::ZipWriter::new(temp.as_file_mut());
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .unix_permissions(0o600);
            zip.start_file("manifest.json", options)
                .map_err(|_| BackupFailure::persistence_failed())?;
            zip.write_all(&manifest_bytes)
                .map_err(|_| BackupFailure::persistence_failed())?;
            for (c, data) in entries {
                zip.start_file(path_for(c), options)
                    .map_err(|_| BackupFailure::persistence_failed())?;
                zip.write_all(&data)
                    .map_err(|_| BackupFailure::persistence_failed())?;
            }
            #[cfg(test)]
            if tests::FAIL_FINISH.with(std::cell::Cell::get) {
                return Err(BackupFailure::persistence_failed());
            }
            zip.finish()
                .map_err(|_| BackupFailure::persistence_failed())?;
        }
        temp.flush()
            .map_err(|_| BackupFailure::persistence_failed())?;
        temp.as_file()
            .sync_all()
            .map_err(|_| BackupFailure::persistence_failed())?;
        self.inspect(temp.path())?;
        #[cfg(unix)]
        let directory =
            std::fs::File::open(parent).map_err(|_| BackupFailure::persistence_failed())?;
        #[cfg(unix)]
        directory
            .sync_all()
            .map_err(|_| BackupFailure::persistence_failed())?;
        temp.persist_noclobber(path).map_err(|e| {
            if e.error.kind() == std::io::ErrorKind::AlreadyExists {
                BackupFailure::new(BackupErrorCode::DestinationExists)
            } else {
                BackupFailure::persistence_failed()
            }
        })?;
        #[cfg(unix)]
        directory
            .sync_all()
            .map_err(|_| BackupFailure::persistence_failed())?;
        Ok(BackupOutcome {
            committed: false,
            components: selection.components.clone(),
            path: Some(path.to_string_lossy().into()),
            counts,
            warnings: snapshot.warnings,
        })
    }
}
pub(crate) fn validate_configuration(config: &ConfigurationV1) -> Result<()> {
    config
        .settings
        .validate()
        .map_err(|_| BackupFailure::invalid_archive())?;
    require(
        config
            .settings
            .token_profiles
            .iter()
            .all(|p| p.token.is_none()),
    )?;
    require(
        config
            .settings
            .notification_connectors
            .iter()
            .all(|c| !matches!(c.provider, ConnectorProvider::Unsupported { .. })),
    )?;
    let mut seen = BTreeSet::new();
    for c in &config.credentials {
        require(
            seen.insert(&c.identity)
                && c.source_reference
                    .as_ref()
                    .is_none_or(|r| uuid::Uuid::parse_str(r).is_ok_and(|v| v.to_string() == *r)),
        )?;
    }
    Ok(())
}
pub(crate) fn validate_history(history: &HistoryV1) -> Result<()> {
    require(
        history.runs.len() <= 100000
            && history
                .runs
                .iter()
                .map(|r| r.detail.nodes.len())
                .sum::<usize>()
                <= 500000,
    )?;
    let mut ids = BTreeMap::new();
    for row in &history.runs {
        let d = &row.detail;
        require(
            valid_backup_run_id(&d.run.id)
                && ids.insert(&d.run.id, d).is_none()
                && valid_repository(&d.run.repository)
                && d.run.repository == d.event.repository
                && d.run.event_id == d.event.event_id
                && d.run.trigger_id == d.event.trigger_id
                && d.run.number == d.event.number
                && d.run.attempt > 0
                && d.flow.id == d.run.trigger_id
                && d.run.actor.eq_ignore_ascii_case(&d.event.actor),
        )?;
        require(
            terminal_for_deletion(&d.run.status)
                || matches!(d.run.status.as_str(), "queued" | "running"),
        )?;
        d.flow
            .validate()
            .map_err(|_| BackupFailure::invalid_archive())?;
        require(chrono::DateTime::parse_from_rfc3339(&d.run.created_at).is_ok())?;
        for timestamp in [&d.run.started_at, &d.run.finished_at]
            .into_iter()
            .flatten()
        {
            require(chrono::DateTime::parse_from_rfc3339(timestamp).is_ok())?;
        }
        let mut nodes = BTreeSet::new();
        for node in &d.nodes {
            for timestamp in [&node.started_at, &node.finished_at].into_iter().flatten() {
                require(chrono::DateTime::parse_from_rfc3339(timestamp).is_ok())?;
            }
            require(
                safe_id(&node.id)
                    && nodes.insert(&node.id)
                    && d.flow.nodes.iter().any(|n| n.id == node.id)
                    && matches!(
                        node.status.as_str(),
                        "pending"
                            | "running"
                            | "succeeded"
                            | "failed"
                            | "blocked"
                            | "cancelled"
                            | "interrupted"
                            | "skipped"
                    ),
            )?;
        }
    }
    let mut seen = BTreeSet::new();
    for row in &history.dedup {
        require(
            seen.insert(&row.event_id)
                && (ids.contains_key(&row.run_id)
                    || matches!(row.run_id.as_str(), "consumed" | "legacy-consumed")),
        )?;
    }
    let mut seen = BTreeSet::new();
    for row in &history.observations {
        require(seen.insert(&row.key) && portable_observation(&row.key))?;
        validate_observation(&row.key, &row.value)?;
        if let Some(owner) = portable_observation_owner(&row.key) {
            require(ids.contains_key(&owner.to_owned()))?;
        }
    }
    let mut seen = BTreeSet::new();
    let mut revisions = BTreeSet::new();
    for row in &history.comments {
        require(chrono::DateTime::parse_from_rfc3339(&row.next_at).is_ok())?;
        let snapshot:crate::context::factory::application::dto::issue_node_comment::IssueNodeCommentSnapshot=serde_json::from_value(row.snapshot.clone()).map_err(|_|BackupFailure::invalid_archive())?;
        let marker =
            crate::context::factory::application::services::issue_node_comments::marker_for(
                &row.run_id,
                &row.node_id,
                row.revision,
            );
        require(
            snapshot.revision == row.revision
                && row.revision > 0
                && revisions.insert((&row.run_id, &row.node_id, row.revision))
                && row.key
                    == marker
                        .trim_start_matches("<!-- ai-factory:")
                        .trim_end_matches(" -->"),
        )?;
        if let Some(receipt) = &row.receipt {
            let receipt:crate::context::factory::application::dto::issue_node_comment::IssueNodeCommentReceipt=serde_json::from_value(receipt.clone()).map_err(|_|BackupFailure::invalid_archive())?;
            require(receipt.comment_id > 0)?;
        }
        let d = ids
            .get(&row.run_id)
            .ok_or_else(BackupFailure::invalid_archive)?;
        require(
            seen.insert(&row.key)
                && d.nodes.iter().any(|n| n.id == row.node_id)
                && row.snapshot["run_id"] == row.run_id
                && row.snapshot["node_id"] == row.node_id
                && row.snapshot["repository"] == d.run.repository
                && row.snapshot["number"] == d.run.number
                && snapshot.run_attempt == d.run.attempt
                && snapshot.actor.eq_ignore_ascii_case(&d.run.actor)
                && matches!(row.state.as_str(), "pending" | "delivered" | "exhausted"),
        )?;
    }
    let mut seen = BTreeSet::new();
    for p in &history.pr_origins {
        require(
            valid_repository(&p.repository)
                && p.number > 0
                && p.origin_key.len() == 64
                && p.origin_key.bytes().all(|b| b.is_ascii_hexdigit())
                && seen.insert((&p.repository, p.number, &p.origin_key)),
        )?;
    }
    Ok(())
}
pub(crate) fn portable_observation(key: &str) -> bool {
    [
        "approval-comment:",
        "review:",
        "action:",
        "pr-update:",
        "pr-merge:",
        "pr-version:",
        "version-result:",
        "release-issue-close:",
    ]
    .iter()
    .any(|p| key.starts_with(p))
}

pub(crate) fn portable_observation_owner(key: &str) -> Option<&str> {
    [
        "pr-update:",
        "pr-merge:",
        "pr-version:",
        "release-issue-close:",
    ]
    .into_iter()
    .find_map(|prefix| key.strip_prefix(prefix))
}
fn repository_number(key: &str) -> bool {
    key.rsplit_once('#').is_some_and(|(repo, n)| {
        valid_repository(repo)
            && n.parse::<u64>()
                .is_ok_and(|n| n > 0 && n <= i64::MAX as u64)
    })
}
fn validate_observation(key: &str, value: &serde_json::Value) -> Result<()> {
    if let Some(rest) = key.strip_prefix("approval-comment:") {
        return require(
            rest.rsplit_once(':').is_some_and(|(identity, n)| {
                repository_number(identity) && n.parse::<u64>().is_ok_and(|n| n > 0)
            }) && value == &serde_json::Value::Bool(true),
        );
    }
    if let Some(rest) = key.strip_prefix("review:") {
        require(
            rest.split_once(':')
                .is_some_and(|(owner, reviewer)| valid_login(owner) && valid_login(reviewer)),
        )?;
        let map = value
            .as_object()
            .ok_or_else(BackupFailure::invalid_archive)?;
        for (identity, v) in map {
            require(
                repository_number(identity)
                    && v.as_object().is_some_and(|o| {
                        o.len() == 2 && o.contains_key("requested") && o.contains_key("generation")
                    })
                    && v["requested"].is_boolean()
                    && v["generation"].is_u64(),
            )?;
        }
        return Ok(());
    }
    if let Some(owner) = portable_observation_owner(key) {
        require(valid_backup_run_id(owner))?;
    } else if let Some(hash) = key.strip_prefix("action:") {
        require(hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))?;
    } else if let Some(event) = key.strip_prefix("version-result:") {
        require(!event.is_empty() && !event.chars().any(char::is_control))?;
    } else {
        return Err(BackupFailure::invalid_archive());
    }
    require(
        value["completed"] == true
            && value.as_object().is_some_and(|o| {
                o.keys()
                    .all(|k| matches!(k.as_str(), "completed" | "remote_id"))
            })
            && value
                .get("remote_id")
                .is_none_or(|v| v.is_u64() || v.is_i64()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::factory::application::interfaces::backup_archive::BackupArchive;
    #[test]
    fn backup_archive_stored_roundtrip_is_selective_and_does_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.zip");
        let selection = BackupSelection::new(vec![BackupComponent::History]).unwrap();
        let snapshot = BackupSnapshot {
            configuration: None,
            history: Some(HistoryV1::default()),
            credentials: None,
            redaction_secrets: vec![],
            warnings: vec![],
        };
        ZipBackupArchive
            .write(&path, &selection, snapshot, None)
            .unwrap();
        assert_eq!(
            ZipBackupArchive.inspect(&path).unwrap().components,
            vec![BackupComponent::History]
        );
        let bytes = std::fs::read(&path).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        assert_eq!(zip.len(), 2);
        assert!(zip.by_name("configuration.json").is_err());
        assert!(zip.by_name("credentials.bin").is_err());
        let snapshot = BackupSnapshot {
            configuration: None,
            history: Some(HistoryV1::default()),
            credentials: None,
            redaction_secrets: vec![],
            warnings: vec![],
        };
        assert!(ZipBackupArchive
            .write(&path, &selection, snapshot, None)
            .is_err());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn backup_archive_rejects_unsafe_entries_before_parsing() {
        use std::io::Write;
        for name in [
            "../escape",
            "/absolute",
            "C:\\escape",
            "manifest.json/",
            "unexpected",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("bad.zip");
            let file = std::fs::File::create(&path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
            zip.write_all(b"{}").unwrap();
            zip.finish().unwrap();
            assert!(ZipBackupArchive.inspect(&path).is_err(), "{name}");
        }
    }
    thread_local! {pub(super) static FAIL_FINISH:std::cell::Cell<bool>=const {std::cell::Cell::new(false)};}
    #[test]
    fn backup_archive_write_failure_never_publishes_partial_file() {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                FAIL_FINISH.with(|f| f.set(false));
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("incomplete.zip");
        let selection = BackupSelection::new(vec![BackupComponent::History]).unwrap();
        let snapshot = BackupSnapshot {
            configuration: None,
            history: Some(HistoryV1::default()),
            credentials: None,
            redaction_secrets: vec![],
            warnings: vec![],
        };
        FAIL_FINISH.with(|f| f.set(true));
        let _reset = Reset;
        assert!(ZipBackupArchive
            .write(&path, &selection, snapshot, None)
            .is_err());
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
