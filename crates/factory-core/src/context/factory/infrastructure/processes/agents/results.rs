mod artifact_files;
mod artifacts;

pub(super) use artifact_files::archive as archive_artifacts;

use anyhow::Result;
use serde_json::{json, Value};

pub(super) fn response_contract(
    codex: bool,
    transport: &Value,
    data_schema: &Value,
) -> Result<String> {
    let mut example = json!({
        "status":"succeeded",
        "summary":"Describe the completed deliverable and verification, or the concrete blocker.",
        "artifacts":[],
        "data":{}
    });
    let fields = if codex {
        example.as_object_mut().unwrap().remove("data");
        example["data_json"] = json!("{}");
        "status, summary, artifacts, data_json. data_json must be a JSON-encoded string, not the unencoded data; encode node data matching its configured schema"
    } else {
        "status, summary, artifacts, data. data must match the configured node data schema"
    };
    Ok(format!(
        "Single-work-execution output contract (mandatory):\nExecute the assigned work once. Factory may request up to {max_corrections} output-only corrections for invalid JSON or schema after this execution. A correction must preserve completed work and must never repeat commands, merge, version bump or commit. Do not rely on correction: validate your complete envelope before returning. Complete the work, then emit the required envelope in this same execution.\nReturn exactly one JSON object, without Markdown fences or surrounding prose. Required root fields: {fields}. No other root fields are allowed. Put branch, verification, native_smoke, git and other node-specific fields inside the node data, only when its schema allows them.\nReserved action keys in node data: publish, update_pr, merge, review, comment and bump_version request GitHub operations; release is unsupported. These are never descriptive report sections. Use review_details, findings or verification for report content. Approval-gate reports must not contain action keys. Include an action key only for an explicitly assigned operation allowed by the node schema and flow. data.review is reserved for PR submission with event and body.\nDo not invent status values: use only succeeded, blocked or failed. Use succeeded only when the assigned deliverable is complete, blocked for a concrete blocker, rejected approval gate or final PR rejection, failed for execution errors. Analysis-only nodes complete their report with succeeded even when findings require changes; the downstream decision node decides approval. summary must be a nonempty string. artifacts must be an array of objects with exactly path, sha256 and kind strings; preserve real paths and hashes. kind is only a descriptive category, not a verification claim. Use artifact when the category is missing or blank; never block solely because category metadata is missing. Never fabricate artifacts or verification. Prefer absolute artifact paths. Relative paths are resolved in the workspace and project memory; if both contain that path, use an absolute path to disambiguate. A directory reference with sha256=\"\" is expanded into verified regular files. For a file whose hash is unavailable, return sha256=\"\"; Factory computes it from the actual bytes. A nonempty supplied hash must match exactly. Symlinks and parent traversal are forbidden.\nBefore ending this execution, check that the final response parses as JSON, contains every required root field, uses one allowed status, has a nonempty summary and an artifacts array, places node-specific fields only inside node data, and satisfies both schemas below. Preserve blockers, failed checks and verification limitations truthfully inside the envelope; never replace it with a prose summary, handoff, code fence or an offer to continue. Skill/repository instructions to write reports or plans apply to deliverable files, not to the final response format.\nEnvelope shape example (populate node data according to its schema; the empty object is not a substitute for required fields):\n{}\n\nOutput JSON Schema:\n{}\n\nNode data JSON Schema:\n{}",
        serde_json::to_string_pretty(&example)?,
        serde_json::to_string_pretty(transport)?,
        serde_json::to_string_pretty(data_schema)?,
        max_corrections = super::repair::MAX_RESPONSE_CORRECTIONS
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResponseFailureKind {
    Missing,
    OuterJson,
    EncodedData,
    Schema,
    Artifact,
    Contract,
}
#[derive(Debug)]
pub(super) struct ResponseFailure {
    pub kind: ResponseFailureKind,
    pub diagnostic: String,
}
impl ResponseFailure {
    fn new(kind: ResponseFailureKind, diagnostic: impl Into<String>) -> Self {
        Self {
            kind,
            diagnostic: diagnostic.into(),
        }
    }
    pub fn repairable(&self) -> bool {
        matches!(
            self.kind,
            ResponseFailureKind::OuterJson
                | ResponseFailureKind::EncodedData
                | ResponseFailureKind::Schema
        )
    }
}
impl std::fmt::Display for ResponseFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.diagnostic)
    }
}
impl std::error::Error for ResponseFailure {}

pub(super) fn parse_response(
    raw: Option<&str>,
    codex: bool,
    validator: &jsonschema::Validator,
) -> std::result::Result<Value, ResponseFailure> {
    use ResponseFailureKind::*;
    let raw = raw.filter(|raw| !raw.trim().is_empty()).ok_or_else(|| {
        ResponseFailure::new(Missing, "Agent produced no valid structured final result")
    })?;
    let value = serde_json::from_str(raw).map_err(|error| {
        ResponseFailure::new(OuterJson, format!("Invalid node result JSON: {error}"))
    })?;
    validate(value, codex, validator)
}
pub(super) fn validate(
    mut value: Value,
    codex: bool,
    validator: &jsonschema::Validator,
) -> std::result::Result<Value, ResponseFailure> {
    use ResponseFailureKind::*;
    if codex {
        if let Some(encoded) = value.as_object_mut().and_then(|v| v.remove("data_json")) {
            if value.get("data").is_some() {
                return Err(ResponseFailure::new(
                    EncodedData,
                    "Return only data_json, not both data and data_json",
                ));
            }
            let encoded = encoded.as_str().ok_or_else(|| {
                ResponseFailure::new(EncodedData, "data_json must be JSON-encoded string")
            })?;
            value["data"] = serde_json::from_str(encoded).map_err(|error| {
                ResponseFailure::new(
                    EncodedData,
                    format!("data_json contains invalid JSON: {error}"),
                )
            })?;
        }
    }
    artifacts::normalize_result(&mut value)
        .map_err(|error| ResponseFailure::new(Artifact, format!("{error:#}")))?;
    let errors: Vec<_> = validator
        .iter_errors(&value)
        .take(16)
        .map(|error| {
            let path = error.instance_path.to_string();
            format!("at {}: {error}", if path.is_empty() { "/" } else { &path })
        })
        .collect();
    if !errors.is_empty() {
        return Err(ResponseFailure::new(
            Schema,
            format!("Invalid node result: {}", errors.join("; ")),
        ));
    }
    if value["status"] == "succeeded" && value["data"]["approved"] == false {
        return Err(ResponseFailure::new(
            Contract,
            "Succeeded result contradicts approved=false; a rejected review must be blocked",
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_inner_brace_is_repairable_without_a_partial_result() {
        let schema = super::super::envelope(&json!({"type":"object"}));
        let validator = jsonschema::validator_for(&schema).unwrap();
        let raw = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/node_output/codex-missing-data-json-brace.json"
        ));
        let error = parse_response(Some(raw), true, &validator).unwrap_err();
        assert_eq!(error.kind, ResponseFailureKind::EncodedData);
        assert!(error.repairable());
        assert!(error.diagnostic.contains("data_json contains invalid JSON"));
        assert!(error.diagnostic.contains("EOF"));
        let corrected = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/node_output/codex-corrected-version-result.json"
        ));
        assert_eq!(
            parse_response(Some(corrected), true, &validator).unwrap()["data"]["bump_version"]
                ["version"],
            "0.9.1"
        );
    }
    #[test]
    fn response_failures_distinguish_structure_from_terminal_contracts() {
        let schema = super::super::envelope(&json!({"type":"object", "required":["deliverable"]}));
        let validator = jsonschema::validator_for(&schema).unwrap();
        for (raw, kind, repairable) in [
            (None, ResponseFailureKind::Missing, false),
            (Some("  "), ResponseFailureKind::Missing, false),
            (Some("{invalid"), ResponseFailureKind::OuterJson, true),
            (
                Some(r#"{"status":"succeeded","summary":"done","artifacts":[],"data":{}}"#),
                ResponseFailureKind::Schema,
                true,
            ),
            (
                Some(
                    r#"{"status":"succeeded","summary":"done","artifacts":[7],"data":{"deliverable":"x"}}"#,
                ),
                ResponseFailureKind::Artifact,
                false,
            ),
            (
                Some(
                    r#"{"status":"succeeded","summary":"done","artifacts":[],"data":{"deliverable":"x","approved":false}}"#,
                ),
                ResponseFailureKind::Contract,
                false,
            ),
            (
                Some(r#"{"data":{},"data_json":"{}"}"#),
                ResponseFailureKind::EncodedData,
                true,
            ),
            (
                Some(r#"{"data_json":{}}"#),
                ResponseFailureKind::EncodedData,
                true,
            ),
        ] {
            let error = parse_response(raw, true, &validator).unwrap_err();
            assert_eq!(error.kind, kind, "{raw:?}");
            assert_eq!(error.repairable(), repairable);
        }
    }
}
