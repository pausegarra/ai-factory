use crate::*;
use serde_json::Value;
pub(super) fn parse(value: &Value) -> UsageObservation {
    match metrics(value) {
        Ok(None) => UsageObservation::state(UsageStatus::Unauthenticated, UsageCode::NotConnected),
        Ok(Some(metrics)) if !metrics.is_empty() => UsageObservation::available(metrics),
        Ok(_) => UsageObservation::state(UsageStatus::Unavailable, UsageCode::NoQuotaData),
        Err(_) => UsageObservation::state(UsageStatus::Error, UsageCode::Protocol),
    }
}
fn metrics(value: &Value) -> crate::Result<Option<Vec<UsageMetric>>> {
    let data = value
        .get("data")
        .ok_or_else(|| anyhow::anyhow!("Missing quota response"))?;
    if data.is_null() {
        return Ok(None);
    }
    anyhow::ensure!(data.is_object(), "Invalid quota response");
    let mut metrics = Vec::new();
    if let Some(go) = data.get("go").filter(|v| !v.is_null()) {
        anyhow::ensure!(go.is_object(), "Invalid quota meters");
        for key in ["rolling", "weekly", "monthly"] {
            let Some(meter) = go.get(key).filter(|v| !v.is_null()) else {
                continue;
            };
            let metric = UsageMetric {
                id: format!("opencode:{key}"),
                label: key.into(),
                kind: UsageMetricKind::Reported,
                derivation: None,
                value: meter["percent"]
                    .as_f64()
                    .ok_or_else(|| anyhow::anyhow!("Invalid quota percentage"))?,
                unit: "percent".into(),
                period: None,
                resets_at: super::reset_at(meter.get("resetsAt"), true)?,
                limit_reached: meter
                    .get("limited")
                    .filter(|v| !v.is_null())
                    .map(|v| {
                        v.as_bool()
                            .ok_or_else(|| anyhow::anyhow!("Invalid quota state"))
                    })
                    .transpose()?,
            };
            metric.validate()?;
            metrics.push(metric);
        }
    }
    Ok(Some(metrics))
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn usage_opencode_keeps_reported_percent_and_millisecond_resets() {
        let result = parse(
            &json!({"location":{},"data":{"workspace":{"usageURL":"fixture-secret"},"go":{"rolling":{"percent":37,"limited":false,"resetsAt":1800000000000i64},"weekly":{"percent":0,"limited":true},"monthly":{"percent":83,"limited":false}},"budgets":[]}}),
        );
        assert_eq!(result.status, UsageStatus::Available);
        assert_eq!(result.metrics.len(), 3);
        assert_eq!(result.metrics[0].value, 37.0);
        assert_eq!(result.metrics[1].value, 0.0);
        assert_eq!(result.metrics[0].kind, UsageMetricKind::Reported);
        assert_eq!(result.metrics[0].derivation, None);
        assert_eq!(
            result.metrics[0].resets_at.as_deref(),
            Some("2027-01-15T08:00:00Z")
        );
        assert_eq!(result.metrics[1].limit_reached, Some(true));
        assert_eq!(result.metrics[0].period, None);
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("fixture-secret"));
    }
    #[test]
    fn usage_opencode_absence_is_distinct_from_errors_and_history() {
        assert_eq!(
            parse(&json!({"location":{},"data":null})).status,
            UsageStatus::Unauthenticated
        );
        assert_eq!(
            parse(&json!({"location":{},"data":{"budgets":[]}})).code,
            Some(UsageCode::NoQuotaData)
        );
        for bad in [
            json!({}),
            json!({"tokens":120,"cost":5}),
            json!({"data":{"go":{"rolling":{"percent":101}}}}),
            json!({"data":{"go":{"rolling":{"percent":37,"resetsAt":9223372036854775807i64}}}}),
        ] {
            let result = parse(&bad);
            assert_eq!(result.status, UsageStatus::Error);
            assert_eq!(result.code, Some(UsageCode::Protocol));
            assert!(result.metrics.is_empty());
        }
    }
}
