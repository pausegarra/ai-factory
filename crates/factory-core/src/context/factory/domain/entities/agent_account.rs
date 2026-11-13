use crate::Result;
use anyhow::ensure;

pub(crate) fn validate_account_id(id: &str) -> Result<()> {
    let parsed =
        uuid::Uuid::parse_str(id).map_err(|_| anyhow::anyhow!("Invalid agent account ID"))?;
    ensure!(parsed.to_string() == id, "Invalid agent account ID");
    Ok(())
}

pub(crate) fn account_label(label: &str) -> Result<String> {
    let label = label.trim();
    ensure!(
        !label.is_empty() && label.chars().count() <= 100 && !label.chars().any(char::is_control),
        "Agent account label must contain 1 to 100 printable characters"
    );
    Ok(label.into())
}
