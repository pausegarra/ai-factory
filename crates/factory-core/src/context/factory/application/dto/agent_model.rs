use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentModel {
    pub id: String,
    pub name: String,
    pub efforts: Vec<String>,
}
