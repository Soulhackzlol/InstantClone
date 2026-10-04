//! What integrations keep between runs, in its own file next to the config:
//! the Twitch logins, the counters and how often each integration ran.
//!
//! Kept apart from the settings on purpose. Tokens change on their own
//! every few hours and counters on every crash; routing those through the
//! settings would rewrite the whole config and race the dashboard's own
//! saves. This file has one writer (the integrations engine), is written
//! atomically, and never leaves the machine.

use crate::json::{self, Value};
use crate::sync::Mutex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const FILE_NAME: &str = "instantclone.integrations-data.json";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TwitchAccount {
    pub login: String,
    pub user_id: String,
    pub access: String,
    pub refresh: String,
    /// When the access token runs out, in Unix milliseconds.
    pub expires_at_ms: u64,
}

impl TwitchAccount {
    fn to_json(&self) -> Value {
        json::obj([
            ("login", json::str(&self.login)),
            ("user_id", json::str(&self.user_id)),
            ("access", json::str(&self.access)),
            ("refresh", json::str(&self.refresh)),
            ("expires_at_ms", Value::Num(self.expires_at_ms as f64)),
        ])
    }

    fn from_json(v: &Value) -> Option<TwitchAccount> {
        let account = TwitchAccount {
            login: v.str_or("login", "").to_string(),
            user_id: v.str_or("user_id", "").to_string(),
            access: v.str_or("access", "").to_string(),
            refresh: v.str_or("refresh", "").to_string(),
            expires_at_ms: v.u64_or("expires_at_ms", 0),
        };
        (!account.login.is_empty() && !account.refresh.is_empty()).then_some(account)
    }
}

#[derive(Default)]
pub struct Accounts {
    pub main: Option<TwitchAccount>,
    pub bot: Option<TwitchAccount>,
}

pub struct Store {
    path: PathBuf,
    pub accounts: Mutex<Accounts>,
    pub counters: Arc<Mutex<BTreeMap<String, i64>>>,
    /// A counter changed since the file was last written.
    pub counters_dirty: std::sync::atomic::AtomicBool,
    /// Runs per integration id, for `{uses}`.
    pub uses: Mutex<BTreeMap<String, u64>>,
    /// Serialises writes so two savers never interleave.
    write_lock: Mutex<()>,
}

impl Store {
    /// Load from `dir`, starting empty if the file is missing or unreadable.
    /// A file that doesn't parse is kept beside it (`.bad`) rather than
    /// overwritten by the first save, so the logins in it can be recovered.
    pub fn open(dir: &Path) -> Store {
        let path = dir.join(FILE_NAME);
        let doc = match std::fs::read_to_string(&path) {
            Ok(text) => json::parse(&text).unwrap_or_else(|_| {
                let _ = std::fs::rename(&path, path.with_extension("json.bad"));
                Value::Null
            }),
            Err(_) => Value::Null,
        };
        let account = |key: &str| doc.path(key).and_then(TwitchAccount::from_json);
        let counters = match doc.get("counters") {
            Some(Value::Obj(fields)) => fields
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as i64)))
                .collect(),
            _ => BTreeMap::new(),
        };
        let uses = match doc.get("uses") {
            Some(Value::Obj(fields)) => fields
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as u64)))
                .collect(),
            _ => BTreeMap::new(),
        };
        Store {
            path,
            accounts: Mutex::new(Accounts {
                main: account("twitch.main"),
                bot: account("twitch.bot"),
            }),
            counters: Arc::new(Mutex::new(counters)),
            counters_dirty: Default::default(),
            uses: Mutex::new(uses),
            write_lock: Mutex::new(()),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let _guard = self.write_lock.lock();
        let doc = {
            let accounts = self.accounts.lock();
            let twitch = json::obj([
                (
                    "main",
                    accounts
                        .main
                        .as_ref()
                        .map_or(Value::Null, TwitchAccount::to_json),
                ),
                (
                    "bot",
                    accounts
                        .bot
                        .as_ref()
                        .map_or(Value::Null, TwitchAccount::to_json),
                ),
            ]);
            let counters = Value::Obj(
                self.counters
                    .lock()
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::Num(*v as f64)))
                    .collect(),
            );
            let uses = Value::Obj(
                self.uses
                    .lock()
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::Num(*v as f64)))
                    .collect(),
            );
            json::obj([("twitch", twitch), ("counters", counters), ("uses", uses)])
        };
        // On disk before the rename: after a power cut the file is the old
        // one or the new one, never a truncated one.
        let tmp = self.path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut file, doc.to_json().as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_reloads_accounts_and_counters() {
        let dir = std::env::temp_dir().join(format!("ic-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir);
        store.accounts.lock().main = Some(TwitchAccount {
            login: "texaz".into(),
            user_id: "42".into(),
            access: "a".into(),
            refresh: "r".into(),
            expires_at_ms: 123,
        });
        store.counters.lock().insert("crashes".into(), 3);
        store.uses.lock().insert("i1".into(), 41);
        store.save().unwrap();

        let back = Store::open(&dir);
        assert_eq!(back.accounts.lock().main.as_ref().unwrap().login, "texaz");
        assert!(back.accounts.lock().bot.is_none());
        assert_eq!(back.counters.lock().get("crashes"), Some(&3));
        assert_eq!(back.uses.lock().get("i1"), Some(&41));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_starts_empty_and_is_kept_aside() {
        let dir = std::env::temp_dir().join(format!("ic-store-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE_NAME), "{not json").unwrap();
        let store = Store::open(&dir);
        assert!(store.accounts.lock().main.is_none());
        assert!(store.counters.lock().is_empty());
        store.save().unwrap();
        let kept = dir.join(FILE_NAME).with_extension("json.bad");
        assert_eq!(std::fs::read_to_string(kept).unwrap(), "{not json");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
