pub use crate::context::factory::domain::entities::agent_usage::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UsageObservation {
    pub status: UsageStatus,
    pub metrics: Vec<UsageMetric>,
    pub code: Option<UsageCode>,
}
impl UsageObservation {
    pub fn state(status: UsageStatus, code: UsageCode) -> Self {
        Self {
            status,
            metrics: vec![],
            code: Some(code),
        }
    }
    pub fn available(metrics: Vec<UsageMetric>) -> Self {
        Self {
            status: UsageStatus::Available,
            metrics,
            code: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UsageEntry {
    pub id: String,
    pub label: String,
    pub email: Option<String>,
    pub account_status: Option<String>,
    pub observation: UsageObservation,
    pub checked_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AgentUsage {
    pub agent: String,
    pub status: UsageStatus,
    pub entries: Vec<UsageEntry>,
    pub checked_at: String,
    pub code: Option<UsageCode>,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn usage_dto_roundtrip_preserves_nulls_and_closed_error() {
        let dto = UsageObservation::state(UsageStatus::Error, UsageCode::Protocol);
        let wire = serde_json::to_value(&dto).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"status":"error","code":"protocol","metrics":[]})
        );
        assert_eq!(
            serde_json::from_value::<UsageObservation>(wire).unwrap(),
            dto
        );
    }
}
