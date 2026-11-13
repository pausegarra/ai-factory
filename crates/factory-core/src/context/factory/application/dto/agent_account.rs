use serde::{Deserialize, Serialize};

/// Public account metadata. Credentials remain exclusively in the managed CLI home.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentAccount {
    pub id: String,
    pub agent: String,
    pub label: String,
    pub email: Option<String>,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct CodexLogin {
    pub account: AgentAccount,
    pub auth_url: String,
}
