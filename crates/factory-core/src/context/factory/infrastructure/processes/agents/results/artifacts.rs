use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};

// Read the logical references before normalizing them. A named map,
// a grouped collection and an array can describe exactly the same files.
struct ArtifactManifest<'a> {
    entries: Vec<&'a Value>,
}

impl<'a> ArtifactManifest<'a> {
    fn parse(value: &'a Value) -> Result<Self> {
        let mut entries = Vec::new();
        Self::collect(value, &mut entries)?;
        Ok(Self { entries })
    }

    fn collect(value: &'a Value, entries: &mut Vec<&'a Value>) -> Result<()> {
        if value.get("path").is_some() || value.get("sha256").is_some() {
            ensure!(
                value.get("path").and_then(Value::as_str).is_some()
                    && value.get("sha256").and_then(Value::as_str).is_some(),
                "Artifact reference requires both path and sha256 strings"
            );
            entries.push(value);
            return Ok(());
        }
        match value {
            Value::Array(items) => {
                for item in items {
                    Self::collect(item, entries)?;
                }
            }
            Value::Object(items) => {
                for item in items.values() {
                    Self::collect(item, entries)?;
                }
            }
            _ => bail!("Artifact manifest must contain path and sha256 string references, in arrays or named collections"),
        }
        Ok(())
    }
}

fn declared_kind(artifact: &Value) -> Option<&str> {
    artifact
        .get("kind")
        .and_then(Value::as_str)
        .filter(|kind| !kind.trim().is_empty())
        .or_else(|| {
            artifact
                .get("type")
                .and_then(Value::as_str)
                .filter(|kind| !kind.trim().is_empty())
        })
}

pub(super) fn normalize_result(value: &mut Value) -> Result<()> {
    let Some(raw) = value.get("artifacts") else {
        return Ok(());
    };
    let manifest = ArtifactManifest::parse(raw).context("Invalid node artifacts")?;
    let mut artifacts: Vec<Value> = manifest.entries.into_iter().cloned().collect();
    for artifact in &mut artifacts {
        let missing = artifact.get("kind").is_none()
            || artifact
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.trim().is_empty());
        if !missing {
            continue;
        }
        let kind = declared_kind(artifact).unwrap_or("artifact").to_owned();
        let fields = artifact.as_object_mut().unwrap();
        // Categories are descriptive metadata; paths, hashes and outcomes stay exact.
        fields.insert("kind".into(), json!(kind));
        if fields
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|alias| alias == kind)
        {
            fields.remove("type");
        }
    }
    value["artifacts"] = Value::Array(artifacts);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_references_cannot_disappear_as_empty_groups() {
        for artifact in [
            json!({"path":{},"sha256":{}}),
            json!({"path":[],"sha256":[]}),
            json!({"path":"report.md","sha256":7}),
            json!({"path":"report.md"}),
        ] {
            let original = json!({"artifacts":[artifact]});
            assert!(
                normalize_result(&mut original.clone()).is_err(),
                "{original}"
            );
        }
    }
}
