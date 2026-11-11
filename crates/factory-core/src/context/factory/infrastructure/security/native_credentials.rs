use crate::{Credentials, Result};

pub struct NativeCredentials;

impl NativeCredentials {
    fn entry(reference: &str) -> Result<keyring::Entry> {
        keyring::Entry::new("dev.aifactory.desktop.github", reference)
            .map_err(|_| anyhow::anyhow!("Cannot access the OS credential store"))
    }
}

impl Credentials for NativeCredentials {
    fn get(&self, reference: &str) -> Result<Option<String>> {
        match Self::entry(reference)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => anyhow::bail!("Cannot read the OS credential store; unlock it and retry"),
        }
    }

    fn set(&self, reference: &str, secret: &str) -> Result<()> {
        Self::entry(reference)?.set_password(secret).map_err(|_| {
            anyhow::anyhow!("Cannot write the OS credential store; unlock it and retry")
        })
    }

    fn delete(&self, reference: &str) -> Result<()> {
        match Self::entry(reference)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => anyhow::bail!("Cannot remove an OS credential; unlock the store and retry"),
        }
    }
}
