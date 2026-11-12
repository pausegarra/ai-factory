use serde::Serialize;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize)]
pub struct InstalledSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip)]
    pub opencode_id: String,
    #[serde(skip)]
    pub manifest_path: PathBuf,
}
