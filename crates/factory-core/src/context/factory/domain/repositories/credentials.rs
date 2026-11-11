use crate::Result;

/// Secret storage port. Implementations must not fall back to plaintext files.
pub trait Credentials: Send + Sync {
    fn get(&self, reference: &str) -> Result<Option<String>>;
    fn set(&self, reference: &str, secret: &str) -> Result<()>;
    fn delete(&self, reference: &str) -> Result<()>;
}
