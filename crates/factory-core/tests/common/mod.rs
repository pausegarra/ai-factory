#![allow(dead_code)]
use factory_core::{Credentials, Factory, Result};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
};

#[derive(Default)]
pub struct MemoryCredentials {
    pub values: Mutex<HashMap<String, String>>,
    pub gets: Mutex<HashMap<String, usize>>,
    pub fail_set: AtomicBool,
    pub fail_get: AtomicBool,
    pub fail_delete: AtomicBool,
    pub corrupt_write: AtomicBool,
}
impl Credentials for MemoryCredentials {
    fn get(&self, key: &str) -> Result<Option<String>> {
        *self.gets.lock().unwrap().entry(key.into()).or_default() += 1;
        anyhow::ensure!(
            !self.fail_get.load(Ordering::SeqCst),
            "fixture backend error with secret text"
        );
        Ok(self.values.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, value: &str) -> Result<()> {
        anyhow::ensure!(
            !self.fail_set.load(Ordering::SeqCst),
            "fixture backend error with secret text"
        );
        self.values.lock().unwrap().insert(
            key.into(),
            if self.corrupt_write.load(Ordering::SeqCst) {
                "corrupted".into()
            } else {
                value.into()
            },
        );
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        anyhow::ensure!(
            !self.fail_delete.load(Ordering::SeqCst),
            "fixture backend error with secret text"
        );
        self.values.lock().unwrap().remove(key);
        Ok(())
    }
}

pub fn backend(root: &std::path::Path) -> Arc<MemoryCredentials> {
    static STORES: OnceLock<Mutex<HashMap<PathBuf, Arc<MemoryCredentials>>>> = OnceLock::new();
    STORES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap()
        .entry(root.into())
        .or_default()
        .clone()
}
pub fn open(root: PathBuf) -> Result<Arc<Factory>> {
    open_with_api(root, "https://api.github.com")
}
pub fn open_with_api(root: PathBuf, base: &str) -> Result<Arc<Factory>> {
    let credentials = backend(&root);
    Factory::open_with_credentials(root, base, credentials)
}

pub fn seed_codex_account(factory: &Factory, id: &str) {
    let root = PathBuf::from(factory.snapshot().unwrap().service.app_data_dir);
    let home = root.join("agent-accounts/codex").join(id);
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"fixture-only"}}"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [
            root.join("agent-accounts"),
            root.join("agent-accounts/codex"),
            home.clone(),
        ] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::fs::set_permissions(
            home.join("auth.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let db = rusqlite::Connection::open(root.join("factory.sqlite3")).unwrap();
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_accounts(id TEXT PRIMARY KEY,json TEXT NOT NULL)",
    )
    .unwrap();
    db.execute("INSERT OR REPLACE INTO agent_accounts VALUES(?1,?2)",rusqlite::params![id,serde_json::json!({"id":id,"agent":"codex","label":"Fixture Codex","email":"fixture@example.test","status":"ready","error":null}).to_string()]).unwrap();
}

pub fn bind_fixture_codex(factory: &Factory, settings: &mut factory_core::Settings) {
    let id = "9ab14479-4d99-41c7-b347-e5b3acffb316";
    seed_codex_account(factory, id);
    for profile in &settings.token_profiles {
        settings
            .codex_bindings
            .insert(profile.id.clone(), id.into());
    }
}

#[cfg(unix)]
pub fn remote_git(root: &std::path::Path, source: &std::path::Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let actual = std::process::Command::new("which")
        .arg("git")
        .output()
        .unwrap();
    let actual = String::from_utf8(actual.stdout).unwrap().trim().to_string();
    let wrapper = root.join("git-wrapper");
    std::fs::write(
        &wrapper,
        format!(
            r#"#!/bin/sh
if [ "$1" = clone ]; then
  printf '%s\n' "$@" >> '{root}/clone-args.log'
  previous=''; last=''
  for arg do previous="$last"; last="$arg"; done
  [ "$previous" = 'https://github.com/acme/repo.git' ] || exit 61
  [ "$GIT_CONFIG_COUNT" = 4 ] || exit 62
  case "$GIT_CONFIG_VALUE_0" in 'AUTHORIZATION: basic '*) ;; *) exit 63;; esac
  exec '{actual}' clone --no-hardlinks -- '{source}' "$last"
fi
if [ "$1" = fetch ]; then
  for arg do
    case "$arg" in
      *:refs/ai-factory/*) target="${{arg#*:}}"; exec '{actual}' update-ref "$target" HEAD;;
    esac
  done
  '{actual}' rev-parse HEAD > .git/FETCH_HEAD; exit $?
fi
exec '{actual}' "$@"
"#,
            root = root.display(),
            source = source.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    wrapper
}

pub mod notifications;
