mod common;
use common::backup::*;
use factory_core::*;
use std::io::{Read, Write};
fn rewrite(path: &std::path::Path, transform: impl FnOnce(&mut Vec<(String, Vec<u8>)>)) {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(path).unwrap())).unwrap();
    let mut entries = (0..zip.len())
        .map(|i| {
            let mut f = zip.by_index(i).unwrap();
            let name = f.name().unwrap().into_owned();
            let mut bytes = Vec::new();
            f.read_to_end(&mut bytes).unwrap();
            (name, bytes)
        })
        .collect::<Vec<_>>();
    transform(&mut entries);
    let mut out = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, bytes) in entries {
        out.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        out.write_all(&bytes).unwrap();
    }
    out.finish().unwrap();
}
#[test]
fn public_components_never_include_managed_secrets_and_export_has_no_mutations() {
    let root = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let f = common::open(root.path().into()).unwrap();
    let mut settings = token_settings();
    settings.flows[0].nodes[0].prompt = "A fixture-managed-token-89 in free text".into();
    f.save_settings(settings).unwrap();
    seed_run(root.path(), "[AI] fixture-managed-token-89");
    let before = table_rows(root.path(), "settings");
    let refs = table_rows(root.path(), "credential_refs");
    let backend = common::backend(root.path());
    let secrets = backend.values.lock().unwrap().clone();
    for chosen in [history(), configuration(), all()] {
        let path = files.path().join(format!(
            "{}.zip",
            chosen.components.len().to_string() + &format!("{:?}", chosen.components[0])
        ));
        f.export_backup(
            path.clone(),
            chosen.clone(),
            if chosen.components.contains(&BackupComponent::Credentials) {
                password()
            } else {
                None
            },
        )
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        for secret in ["fixture-managed-token-89", "fixture password 89"] {
            assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()));
        }
        assert!(f.inspect_backup(path).is_ok());
    }
    assert_eq!(table_rows(root.path(), "settings"), before);
    assert_eq!(table_rows(root.path(), "credential_refs"), refs);
    assert_eq!(*backend.values.lock().unwrap(), secrets);
}
#[test]
fn malformed_archive_metadata_is_rejected_without_mutating_any_category() {
    for fault in [
        "format",
        "version",
        "checksum",
        "unknown",
        "kdf",
        "duplicate_json",
    ] {
        let root = tempfile::tempdir().unwrap();
        let files = tempfile::tempdir().unwrap();
        let f = common::open(root.path().into()).unwrap();
        let path = files.path().join("bad.zip");
        f.export_backup(path.clone(), all(), password()).unwrap();
        let before = table_rows(root.path(), "settings");
        let secrets = common::backend(root.path()).values.lock().unwrap().clone();
        rewrite(&path, |entries| {
            let manifest = &mut entries
                .iter_mut()
                .find(|(n, _)| n == "manifest.json")
                .unwrap()
                .1;
            let mut value: serde_json::Value = serde_json::from_slice(manifest).unwrap();
            match fault {
                "format" => value["format"] = "other".into(),
                "version" => value["version"] = 2.into(),
                "checksum" => value["entries"]["history"]["sha256"] = "a".repeat(64).into(),
                "unknown" => value["unrecognized"] = true.into(),
                "kdf" => value["encryption"]["memory_kib"] = u32::MAX.into(),
                _ => (),
            };
            *manifest = serde_json::to_vec(&value).unwrap();
            if fault == "duplicate_json" {
                manifest.splice(1..1, b"\"version\":1,".iter().copied());
            }
        });
        assert!(f.inspect_backup(path.clone()).is_err(), "{fault}");
        assert!(
            f.prepare_backup_import(path, all(), password()).is_err(),
            "{fault}"
        );
        assert_eq!(table_rows(root.path(), "settings"), before);
        assert_eq!(
            *common::backend(root.path()).values.lock().unwrap(),
            secrets
        );
    }
}
#[test]
fn physical_zip_attacks_are_rejected_without_extraction() {
    let root = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let f = common::open(root.path().into()).unwrap();
    let path = files.path().join("valid.zip");
    f.export_backup(path.clone(), history(), None).unwrap();
    let valid = std::fs::read(&path).unwrap();
    for fault in [
        "count",
        "disk",
        "directory_size",
        "overlap",
        "compression",
        "symlink",
        "dos_directory",
        "traversal",
        "comment",
        "trailing",
    ] {
        let mut bytes = valid.clone();
        let end = bytes.len() - 22;
        let central = u32::from_le_bytes(bytes[end + 16..end + 20].try_into().unwrap()) as usize;
        match fault {
            "count" => {
                bytes[end + 8..end + 12].fill(255);
            }
            "disk" => bytes[end + 4] = 1,
            "directory_size" => bytes[end + 12..end + 16].fill(255),
            "overlap" => bytes[central + 42..central + 46].fill(0xff),
            "compression" => bytes[central + 10] = 8,
            "symlink" => bytes[central + 38..central + 42]
                .copy_from_slice(&((0o120777u32) << 16).to_le_bytes()),
            "dos_directory" => bytes[central + 38] |= 0x10,
            "comment" => bytes[end + 20] = 1,
            "trailing" => bytes.push(0),
            "traversal" => {
                let target = b"history.json";
                let positions = bytes
                    .windows(target.len())
                    .enumerate()
                    .filter(|(_, w)| *w == target)
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>();
                for p in positions {
                    bytes[p..p + 12].copy_from_slice(b"../evil.json");
                }
            }
            _ => (),
        }
        let bad = files.path().join(format!("{fault}.zip"));
        std::fs::write(&bad, bytes).unwrap();
        assert!(f.inspect_backup(bad).is_err(), "{fault}");
    }
    assert!(!files.path().join("evil.json").exists());
}
#[test]
fn credentials_in_an_unselected_category_do_not_require_a_password() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let source = common::open(src.path().into()).unwrap();
    let target = common::open(dst.path().into()).unwrap();
    source.save_settings(token_settings()).unwrap();
    let path = files.path().join("all.zip");
    source
        .export_backup(path.clone(), all(), password())
        .unwrap();
    let ticket = target.prepare_backup_import(path, history(), None).unwrap();
    assert!(
        target
            .apply_backup_import(ticket.confirmation_id, true, None)
            .unwrap()
            .committed
    );
}

#[test]
fn invalid_history_timestamps_and_relationships_are_rejected_before_writes() {
    use sha2::{Digest, Sha256};
    for fault in ["timestamp", "flow", "dedup", "comment_owner", "observation"] {
        let root = tempfile::tempdir().unwrap();
        let files = tempfile::tempdir().unwrap();
        let f = common::open(root.path().into()).unwrap();
        seed_mixed_backup_history(root.path(), &f);
        let path = files.path().join("history.zip");
        f.export_backup(path.clone(), history(), None).unwrap();
        let before = table_rows(root.path(), "runs");
        rewrite(&path, |entries| {
            let history = &mut entries
                .iter_mut()
                .find(|(n, _)| n == "history.json")
                .unwrap()
                .1;
            let mut value: serde_json::Value = serde_json::from_slice(history).unwrap();
            match fault {
                "timestamp" => {
                    value["runs"][0]["detail"]["run"]["finished_at"] = "invalid-time".into()
                }
                "flow" => value["runs"][0]["detail"]["flow"]["id"] = "different".into(),
                "dedup" => value["dedup"][0]["run_id"] = "missing".into(),
                "observation" => value["observations"].as_array_mut().unwrap().push(
                    serde_json::json!({"key":"approval-comment:invalid#0:bad","value":false}),
                ),
                _ => value["comments"][0]["snapshot"]["run_id"] = "other-owner".into(),
            };
            *history = serde_json::to_vec(&value).unwrap();
            let hash = format!("{:x}", Sha256::digest(&*history));
            let size = history.len();
            let manifest = &mut entries
                .iter_mut()
                .find(|(n, _)| n == "manifest.json")
                .unwrap()
                .1;
            let mut value: serde_json::Value = serde_json::from_slice(manifest).unwrap();
            value["entries"]["history"]["sha256"] = hash.into();
            value["entries"]["history"]["size_bytes"] = size.into();
            *manifest = serde_json::to_vec(&value).unwrap();
        });
        assert!(
            f.prepare_backup_import(path, history(), None).is_err(),
            "{fault}"
        );
        assert_eq!(table_rows(root.path(), "runs"), before);
    }
}
