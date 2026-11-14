use anyhow::{ensure, Context, Result};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(in super::super) struct HashFormatIssue {
    pub index: usize,
    pub path: String,
    pub sha256: String,
    pub reason: String,
}

pub(in super::super) fn hash_format_issues(value: &Value) -> Result<Vec<HashFormatIssue>> {
    let mut issues = Vec::new();
    for (index, artifact) in value["artifacts"]
        .as_array()
        .context("Artifact array missing")?
        .iter()
        .enumerate()
    {
        let hash = artifact["sha256"]
            .as_str()
            .context("Artifact hash missing")?;
        if hash.is_empty() || (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())) {
            continue;
        }
        issues.push(HashFormatIssue {
            index,
            path: artifact["path"]
                .as_str()
                .context("Artifact path missing")?
                .into(),
            sha256: hash.into(),
            reason: format!(
                "expected empty string or 64 ASCII hexadecimal characters; length={}; non_hex={}",
                hash.len(),
                hash.bytes().any(|b| !b.is_ascii_hexdigit())
            ),
        });
    }
    Ok(issues)
}

pub(in super::super) fn ensure_artifact_hash_formats(value: &Value) -> Result<()> {
    let issues = hash_format_issues(value)?;
    ensure!(
        issues.is_empty(),
        "Invalid artifact sha256 format: {}",
        serde_json::to_string(&issues)?
    );
    Ok(())
}

pub(in super::super) fn ensure_hash_only_changes(
    previous: &Value,
    candidate: &Value,
) -> Result<()> {
    let prior = previous["artifacts"]
        .as_array()
        .context("Artifact array missing")?;
    let mut restored = candidate.clone();
    let artifacts = restored["artifacts"]
        .as_array_mut()
        .context("Artifact array missing")?;
    ensure!(
        prior.len() == artifacts.len(),
        "only malformed artifact sha256 fields may change; artifact count changed"
    );
    for issue in hash_format_issues(previous)? {
        let artifact = artifacts
            .get_mut(issue.index)
            .context("Correction artifact missing")?;
        ensure!(
            artifact["sha256"].is_string(),
            "Correction artifact hash must be a string"
        );
        let object = artifact
            .as_object_mut()
            .context("Correction artifact must be an object")?;
        object.insert("sha256".into(), Value::String(issue.sha256));
    }
    ensure!(&restored == previous, "only malformed artifact sha256 fields may change; envelope, data and manifest must be preserved");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn result(hash: &str) -> Value {
        json!({"status":"succeeded","summary":"done","artifacts":[{"path":"report.md","sha256":hash,"kind":"report"}],"data":{"branch":"feature"}})
    }
    #[test]
    fn format_accepts_only_empty_or_64_ascii_hex_bytes() {
        for hash in ["".into(), "a".repeat(64), "A".repeat(64)] {
            assert!(hash_format_issues(&result(&hash)).unwrap().is_empty());
        }
        for hash in [
            "47452e8575b83bef2aa9e37c1a0a90b??".into(),
            "a".repeat(63),
            "a".repeat(65),
            "g".repeat(64),
            format!(" {}", "a".repeat(63)),
            format!("{}\n", "a".repeat(63)),
            format!("0x{}", "a".repeat(62)),
            "é".repeat(32),
        ] {
            let issues = hash_format_issues(&result(&hash)).unwrap();
            assert_eq!(issues.len(), 1);
            assert_eq!(issues[0].index, 0);
            assert_eq!(issues[0].path, "report.md");
            assert_eq!(issues[0].sha256, hash);
        }
    }
    #[test]
    fn correction_preserves_every_field_except_previously_malformed_hashes() {
        let previous = result("bad??");
        let mut corrected = previous.clone();
        corrected["artifacts"][0]["sha256"] = json!("");
        ensure_hash_only_changes(&previous, &corrected).unwrap();
        for pointer in [
            "/status",
            "/summary",
            "/data/branch",
            "/artifacts/0/path",
            "/artifacts/0/kind",
        ] {
            let mut changed = corrected.clone();
            *changed.pointer_mut(pointer).unwrap() = json!("changed");
            assert!(
                ensure_hash_only_changes(&previous, &changed).is_err(),
                "{pointer}"
            );
        }
        for hash in ["".into(), "a".repeat(64)] {
            let mut changed = result(&hash);
            changed["artifacts"][0]["sha256"] = json!("b".repeat(64));
            assert!(ensure_hash_only_changes(&result(&hash), &changed).is_err());
        }
        let mut removed = corrected.clone();
        removed["artifacts"] = json!([]);
        assert!(ensure_hash_only_changes(&previous, &removed).is_err());
        let mut added = corrected.clone();
        added["artifacts"]
            .as_array_mut()
            .unwrap()
            .push(corrected["artifacts"][0].clone());
        assert!(ensure_hash_only_changes(&previous, &added).is_err());
        let mut action = corrected;
        action["data"]["publish"] = json!({"title":"changed","body":"changed"});
        assert!(ensure_hash_only_changes(&previous, &action).is_err());
    }
    #[test]
    fn duplicate_paths_keep_distinct_issue_indices() {
        let mut v = result("bad??");
        let second = v["artifacts"][0].clone();
        v["artifacts"].as_array_mut().unwrap().push(second);
        let issues = hash_format_issues(&v).unwrap();
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[1].index, 1);
    }
}
