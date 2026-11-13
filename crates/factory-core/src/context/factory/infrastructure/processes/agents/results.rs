mod artifact_files;
mod artifacts;

pub(super) use artifact_files::archive as archive_artifacts;

use anyhow::{ensure, Context, Result};
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
        "Single-attempt output contract (mandatory):\nNo response repair or automatic retry is available. Your first final response is the only response Factory will validate; invalid output fails this node and skips its dependents. Complete the work, then emit the required envelope in this same execution.\nReturn exactly one JSON object, without Markdown fences or surrounding prose. Required root fields: {fields}. No other root fields are allowed. Put branch, verification, native_smoke, git and other node-specific fields inside the node data, only when its schema allows them.\nReserved action keys in node data: publish, update_pr, merge, review, comment and release request GitHub operations, never descriptive report sections. Use review_details, findings or verification for report content. Approval-gate reports must not contain action keys. Include an action key only for an explicitly assigned operation allowed by the node schema and flow. data.review is reserved for PR submission with event and body.\nDo not invent status values: use only succeeded, blocked or failed. Use succeeded only when the assigned deliverable is complete, blocked for a concrete blocker, rejected approval gate or final PR rejection, failed for execution errors. Analysis-only nodes complete their report with succeeded even when findings require changes; the downstream decision node decides approval. summary must be a nonempty string. artifacts must be an array of objects with exactly path, sha256 and kind strings; preserve real paths and hashes. kind is only a descriptive category, not a verification claim. Use artifact when the category is missing or blank; never block solely because category metadata is missing. Never fabricate artifacts or verification. Prefer absolute artifact paths. Relative paths are resolved in the workspace and project memory; if both contain that path, use an absolute path to disambiguate. A directory reference with sha256=\"\" is expanded into verified regular files. For a file whose hash is unavailable, return sha256=\"\"; Factory computes it from the actual bytes. A nonempty supplied hash must match exactly. Symlinks and parent traversal are forbidden.\nBefore ending this execution, check that the final response parses as JSON, contains every required root field, uses one allowed status, has a nonempty summary and an artifacts array, places node-specific fields only inside node data, and satisfies both schemas below. Preserve blockers, failed checks and verification limitations truthfully inside the envelope; never replace it with a prose summary, handoff, code fence or an offer to continue. Skill/repository instructions to write reports or plans apply to deliverable files, not to the final response format.\nEnvelope shape example (populate node data according to its schema; the empty object is not a substitute for required fields):\n{}\n\nOutput JSON Schema:\n{}\n\nNode data JSON Schema:\n{}",
        serde_json::to_string_pretty(&example)?,
        serde_json::to_string_pretty(transport)?,
        serde_json::to_string_pretty(data_schema)?
    ))
}

pub(super) fn validate(
    mut value: Value,
    codex: bool,
    validator: &jsonschema::Validator,
) -> Result<Value> {
    if codex {
        if let Some(encoded) = value.as_object_mut().and_then(|v| v.remove("data_json")) {
            ensure!(
                value.get("data").is_none(),
                "Return only data_json, not both data and data_json"
            );
            value["data"] = serde_json::from_str(
                encoded
                    .as_str()
                    .context("data_json must be JSON-encoded string")?,
            )
            .context("data_json contains invalid JSON")?;
        }
    }
    artifacts::normalize_result(&mut value)?;
    let errors: Vec<_> = validator
        .iter_errors(&value)
        .take(16)
        .map(|error| {
            let path = error.instance_path.to_string();
            format!("at {}: {error}", if path.is_empty() { "/" } else { &path })
        })
        .collect();
    ensure!(
        errors.is_empty(),
        "Invalid node result: {}",
        errors.join("; ")
    );
    ensure!(
        value["status"] != "succeeded" || value["data"]["approved"] != false,
        "Succeeded result contradicts approved=false; a rejected review must be blocked"
    );
    Ok(value)
}
