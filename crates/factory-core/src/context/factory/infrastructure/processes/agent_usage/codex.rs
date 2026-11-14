use crate::*;
use serde_json::Value;
pub(super) fn parse(value: &Value) -> UsageObservation {
    match metrics(value) {
        Ok(metrics) if !metrics.is_empty() => UsageObservation::available(metrics),
        Ok(_) => UsageObservation::state(UsageStatus::Unavailable, UsageCode::NoQuotaData),
        Err(_) => UsageObservation::state(UsageStatus::Error, UsageCode::Protocol),
    }
}
fn metrics(value: &Value) -> crate::Result<Vec<UsageMetric>> {
    let mut metrics = Vec::new();
    if let Some(map) = value.get("rateLimitsByLimitId").filter(|v| !v.is_null()) {
        let map = map
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Invalid quota buckets"))?;
        if !map.is_empty() {
            for (id, bucket) in map {
                windows(
                    bucket,
                    &format!("codex:{id}"),
                    Some(
                        bucket["limitName"]
                            .as_str()
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or(id),
                    ),
                    &mut metrics,
                )?;
            }
            return Ok(metrics);
        }
    }
    if let Some(bucket) = value.get("rateLimits").filter(|v| !v.is_null()) {
        windows(bucket, "codex", None, &mut metrics)?;
    }
    Ok(metrics)
}
fn windows(
    bucket: &Value,
    prefix: &str,
    name: Option<&str>,
    metrics: &mut Vec<UsageMetric>,
) -> crate::Result<()> {
    anyhow::ensure!(bucket.is_object(), "Invalid quota bucket");
    for (key, label) in [
        ("primary", "Primary window"),
        ("secondary", "Secondary window"),
    ] {
        let Some(window) = bucket.get(key).filter(|v| !v.is_null()) else {
            continue;
        };
        let used = window["usedPercent"]
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("Invalid quota percentage"))?;
        let period = window
            .get("windowDurationMins")
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_u64()
                    .map(|mins| format!("{mins} minutes"))
                    .ok_or_else(|| anyhow::anyhow!("Invalid quota period"))
            })
            .transpose()?;
        let metric = UsageMetric {
            id: format!("{prefix}:{key}"),
            label: name
                .map(|name| format!("{name} · {label}"))
                .unwrap_or_else(|| label.into()),
            kind: UsageMetricKind::Remaining,
            derivation: Some(UsageMetricDerivation::CodexUsedPercentComplement),
            value: remaining_percent_from_used_percent(used)?,
            unit: "percent".into(),
            period,
            resets_at: super::reset_at(window.get("resetsAt"), false)?,
            limit_reached: None,
        };
        metric.validate()?;
        metrics.push(metric);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn usage_codex_parser_matches_shared_contract() {
        let fixture: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/agent_usage/codex-remaining-contract.json"
        )))
        .unwrap();
        let mut count = 0;
        for case in fixture["cases"].as_array().unwrap() {
            let Some(result) = case["rpc_response"].get("result") else {
                continue;
            };
            let actual = serde_json::to_value(parse(result)).unwrap();
            let expected: UsageObservation =
                serde_json::from_value(case["wire"]["entries"][0]["observation"].clone()).unwrap();
            assert_eq!(
                actual,
                serde_json::to_value(expected).unwrap(),
                "{}",
                case["name"]
            );
            count += 1;
        }
        assert_eq!(count, 7);
    }
    #[test]
    fn usage_codex_map_deduplicates_legacy_and_preserves_windows_seconds() {
        let bucket =
            json!({"primary":{"usedPercent":27,"windowDurationMins":300,"resetsAt":1800000000}});
        let result = parse(
            &json!({"rateLimits":bucket,"rateLimitsByLimitId":{"codex":bucket,"other":{"secondary":{"usedPercent":100}}}}),
        );
        assert_eq!(result.status, UsageStatus::Available);
        assert_eq!(result.metrics.len(), 2);
        assert_eq!(result.metrics[0].value, 73.0);
        assert_eq!(result.metrics[0].id, "codex:codex:primary");
        assert_eq!(result.metrics[0].label, "codex · Primary window");
        assert_eq!(result.metrics[1].label, "other · Secondary window");
        assert_eq!(
            result.metrics[0].resets_at.as_deref(),
            Some("2027-01-15T08:00:00Z")
        );
        assert_eq!(result.metrics[1].value, 0.0);
        for m in result.metrics {
            assert!(m.validate().is_ok());
        }
    }
    #[test]
    fn usage_codex_invalid_window_invalidates_whole_observation() {
        for bad in [
            json!({"usedPercent":-1}),
            json!({"usedPercent":27,"resetsAt":9223372036854775807i64}),
            json!({"usedPercent":27,"windowDurationMins":"bad"}),
        ] {
            let result = parse(
                &json!({"rateLimits":{"primary":{"usedPercent":0},"secondary":bad},"secret":"fixture-secret"}),
            );
            assert_eq!(result.status, UsageStatus::Error);
            assert!(result.metrics.is_empty());
            assert!(!serde_json::to_string(&result)
                .unwrap()
                .contains("fixture-secret"));
        }
        for input in [
            json!({}),
            json!({"rateLimitsByLimitId":{}}),
            json!({"rateLimits":null}),
        ] {
            assert_eq!(parse(&input).code, Some(UsageCode::NoQuotaData));
        }
    }
}
