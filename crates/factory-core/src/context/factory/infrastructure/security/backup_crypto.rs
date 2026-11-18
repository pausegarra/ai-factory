use crate::context::factory::application::dto::backup_format::*;
use crate::BackupFailure;
use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;
type Result<T> = std::result::Result<T, BackupFailure>;
impl EncryptionV1 {
    pub fn fresh() -> Result<Self> {
        let mut salt = [0; 16];
        let mut nonce = [0; 12];
        OsRng
            .try_fill_bytes(&mut salt)
            .map_err(|_| BackupFailure::persistence_failed())?;
        OsRng
            .try_fill_bytes(&mut nonce)
            .map_err(|_| BackupFailure::persistence_failed())?;
        Ok(Self {
            algorithm: "aes-256-gcm".into(),
            kdf: "argon2id".into(),
            argon_version: 19,
            memory_kib: 65536,
            iterations: 3,
            parallelism: 1,
            salt,
            nonce,
        })
    }
    pub fn validate(&self) -> Result<()> {
        if self.algorithm != "aes-256-gcm"
            || self.kdf != "argon2id"
            || self.argon_version != 19
            || self.memory_kib != 65536
            || self.iterations != 3
            || self.parallelism != 1
        {
            Err(BackupFailure::invalid_archive())
        } else {
            Ok(())
        }
    }
}
fn cipher(password: &str, e: &EncryptionV1) -> Result<Aes256Gcm> {
    e.validate()?;
    if !(12..=1024).contains(&password.len()) {
        return Err(BackupFailure::invalid_password());
    }
    let params =
        argon2::Params::new(65536, 3, 1, Some(32)).map_err(|_| BackupFailure::invalid_archive())?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), &e.salt, key.as_mut())
        .map_err(|_| BackupFailure::invalid_password())?;
    Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| BackupFailure::invalid_password())
}
pub fn encrypt_credentials(
    plaintext: &[u8],
    password: &str,
    e: &EncryptionV1,
    aad: &[u8],
) -> Result<Vec<u8>> {
    cipher(password, e)?
        .encrypt(
            Nonce::from_slice(&e.nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| BackupFailure::invalid_password())
}
pub fn decrypt_credentials(
    ciphertext: &[u8],
    password: &str,
    e: &EncryptionV1,
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    cipher(password, e)?
        .decrypt(
            Nonce::from_slice(&e.nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| BackupFailure::invalid_password())
}
pub fn redact_portable_value(
    value: &mut serde_json::Value,
    secrets: &[Zeroizing<String>],
) -> Result<()> {
    let mut patterns=secrets.iter().flat_map(|s|crate::context::factory::infrastructure::repositories::secure_settings::redaction_patterns(s)).filter(|s|!s.is_empty()).collect::<Vec<_>>();
    patterns.sort_by_key(|v| std::cmp::Reverse(v.len()));
    patterns.dedup();
    fn walk(v: &mut serde_json::Value, p: &[String], depth: usize) -> Result<()> {
        if depth > 128 {
            return Err(BackupFailure::limit_exceeded());
        }
        match v {
            serde_json::Value::String(text) => {
                if text.len() > 1024 * 1024 {
                    return Err(BackupFailure::limit_exceeded());
                }
                for pattern in p {
                    *text = text.replace(pattern, "[REDACTED]");
                }
            }
            serde_json::Value::Array(a) => {
                for v in a {
                    walk(v, p, depth + 1)?
                }
            }
            serde_json::Value::Object(o) => {
                for (key, v) in o {
                    if p.iter().any(|p| key.contains(p)) {
                        return Err(BackupFailure::invalid_archive());
                    }
                    walk(v, p, depth + 1)?
                }
            }
            _ => (),
        };
        Ok(())
    }
    let patterns = Zeroizing::new(patterns);
    walk(value, &patterns, 0)
}
#[derive(Serialize)]
struct BorrowedSecret<'a> {
    identity: &'a crate::CredentialIdentity,
    source_reference: &'a Option<String>,
    value: &'a str,
}
#[derive(Serialize)]
struct BorrowedBundle<'a> {
    records: Vec<BorrowedSecret<'a>>,
    missing: &'a [crate::CredentialIdentity],
}
pub fn encode_secrets(bundle: &SecretBundle) -> Result<Zeroizing<Vec<u8>>> {
    let borrowed = BorrowedBundle {
        records: bundle
            .records
            .iter()
            .map(|r| BorrowedSecret {
                identity: &r.identity,
                source_reference: &r.source_reference,
                value: &r.value,
            })
            .collect(),
        missing: &bundle.missing,
    };
    serde_json::to_vec(&borrowed)
        .map(Zeroizing::new)
        .map_err(|_| BackupFailure::invalid_archive())
}
fn secret_string<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Zeroizing<String>, D::Error> {
    String::deserialize(d).map(Zeroizing::new)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadSecret {
    identity: crate::CredentialIdentity,
    source_reference: Option<String>,
    #[serde(deserialize_with = "secret_string")]
    value: Zeroizing<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadBundle {
    records: Vec<ReadSecret>,
    missing: Vec<crate::CredentialIdentity>,
}
pub fn decode_secrets(bytes: &[u8]) -> Result<SecretBundle> {
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(BackupFailure::limit_exceeded());
    }
    let bundle: ReadBundle =
        serde_json::from_slice(bytes).map_err(|_| BackupFailure::invalid_archive())?;
    let mut identities = std::collections::BTreeSet::new();
    let mut records = Vec::new();
    for record in bundle.records {
        if record
            .source_reference
            .as_ref()
            .is_some_and(|r| uuid::Uuid::parse_str(r).map_or(true, |v| v.to_string() != *r))
            || record.value.is_empty()
            || record.value.len() > 65536
            || !identities.insert(record.identity.clone())
        {
            return Err(BackupFailure::invalid_archive());
        }
        records.push(SecretRecord {
            identity: record.identity,
            source_reference: record.source_reference,
            value: record.value,
        });
    }
    for identity in &bundle.missing {
        if !identities.insert(identity.clone()) {
            return Err(BackupFailure::invalid_archive());
        }
    }
    Ok(SecretBundle {
        records,
        missing: bundle.missing,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticated_credentials_reject_wrong_password_and_modified_context() {
        let e = EncryptionV1::fresh().unwrap();
        let text = b"fixture-secret-89";
        let context = b"archive-context";
        let encrypted = encrypt_credentials(text, "fixture password 89", &e, context).unwrap();
        assert!(!encrypted.windows(text.len()).any(|w| w == text));
        assert_eq!(
            decrypt_credentials(&encrypted, "fixture password 89", &e, context)
                .unwrap()
                .as_slice(),
            text
        );
        assert!(decrypt_credentials(&encrypted, "another password 89", &e, context).is_err());
        assert!(decrypt_credentials(&encrypted, "fixture password 89", &e, b"different").is_err());
        let mut corrupt = encrypted;
        corrupt[0] ^= 1;
        assert!(decrypt_credentials(&corrupt, "fixture password 89", &e, context).is_err());
        let mut bad = e.clone();
        bad.memory_kib = u32::MAX;
        assert!(decrypt_credentials(b"x", "fixture password 89", &bad, context).is_err());
        assert!(encrypt_credentials(text, "short", &e, context).is_err());
        let next = EncryptionV1::fresh().unwrap();
        assert_ne!(next.salt, e.salt);
        assert_ne!(next.nonce, e.nonce);
    }
    #[test]
    fn backup_crypto_redacts_strings_without_damaging_utf8() {
        let mut value = serde_json::json!({"prompt":"español fixture-managed-secret"});
        redact_portable_value(
            &mut value,
            &[zeroize::Zeroizing::new("fixture-managed-secret".into())],
        )
        .unwrap();
        assert_eq!(value["prompt"], "español [REDACTED]");
    }
}
