use anyhow::ensure;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageStatus {
    Available,
    NoAccounts,
    Pending,
    Failed,
    Unauthenticated,
    Unavailable,
    Error,
    Preview,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageMetricKind {
    Remaining,
    Used,
    Reported,
    Limit,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageMetricDerivation {
    CodexUsedPercentComplement,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageCode {
    NotConnected,
    SignInPending,
    SignInFailed,
    Unsupported,
    NoQuotaData,
    CliMissing,
    TimedOut,
    Protocol,
    QueryFailed,
    HomeUnavailable,
    Stopping,
    DesktopRequired,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UsageMetric {
    pub id: String,
    pub label: String,
    pub kind: UsageMetricKind,
    pub derivation: Option<UsageMetricDerivation>,
    pub value: f64,
    pub unit: String,
    pub period: Option<String>,
    pub resets_at: Option<String>,
    pub limit_reached: Option<bool>,
}
impl UsageMetric {
    pub fn validate(&self) -> crate::Result<()> {
        ensure!(
            !self.id.trim().is_empty()
                && !self.label.trim().is_empty()
                && !self.unit.trim().is_empty(),
            "Invalid quota identity"
        );
        ensure!(
            self.value.is_finite()
                && self.value >= 0.0
                && (self.unit != "percent" || self.value <= 100.0),
            "Invalid quota value"
        );
        ensure!(
            self.derivation.is_none()
                || (self.kind == UsageMetricKind::Remaining && self.unit == "percent"),
            "Invalid quota derivation"
        );
        if let Some(date) = &self.resets_at {
            chrono::DateTime::parse_from_rfc3339(date)?;
        }
        Ok(())
    }
}
pub fn remaining_percent_from_used_percent(used_percent: f64) -> crate::Result<f64> {
    ensure!(
        used_percent.is_finite() && (0.0..=100.0).contains(&used_percent),
        "Invalid quota percentage"
    );
    Ok(100.0 - used_percent)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn metric(value: f64, unit: &str) -> UsageMetric {
        UsageMetric {
            id: "primary".into(),
            label: "Primary window".into(),
            kind: UsageMetricKind::Remaining,
            derivation: None,
            value,
            unit: unit.into(),
            period: None,
            resets_at: None,
            limit_reached: None,
        }
    }
    #[test]
    fn usage_values_reject_invalid_numbers_instead_of_clamping() {
        for value in [-1.0, 100.1, f64::INFINITY, f64::NAN] {
            assert!(metric(value, "percent").validate().is_err());
        }
        for value in [0.0, 100.0] {
            assert!(metric(value, "percent").validate().is_ok());
        }
        assert!(metric(2400.0, "tokens").validate().is_ok());
        let mut m = metric(27.0, "percent");
        m.id.clear();
        assert!(m.validate().is_err());
        m.id = "primary".into();
        m.resets_at = Some("bad-date".into());
        assert!(m.validate().is_err());
        m.resets_at = Some("1970-01-01T00:00:00Z".into());
        assert!(m.validate().is_ok());
        m.derivation = Some(UsageMetricDerivation::CodexUsedPercentComplement);
        m.kind = UsageMetricKind::Reported;
        assert!(m.validate().is_err());
    }
    #[test]
    fn usage_values_codex_remaining_complement() {
        for (used, remaining) in [(27.0, 73.0), (0.0, 100.0), (100.0, 0.0)] {
            assert_eq!(
                remaining_percent_from_used_percent(used).unwrap(),
                remaining
            );
        }
        for used in [-1.0, 100.1, f64::NAN, f64::INFINITY] {
            assert!(remaining_percent_from_used_percent(used).is_err());
        }
    }
}
