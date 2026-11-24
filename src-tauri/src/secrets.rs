// SPDX-License-Identifier: GPL-3.0-or-later
/*! cp-secrets — app.secretStorage backed by the OS keyring.

The plugin API's SecretStorage (typings 1.11.4: setSecret / getSecret /
listSecrets) keeps its secrets in the freedesktop Secret Service
(org.freedesktop.secrets on the session bus: gnome-keyring, KWallet's
ksecretd, KeePassXC). Rules, each one pinned by a test below or by the
`secrets` gate phase:

- NEVER plaintext on disk from us: no JSON in the vault, nothing in
  ~/.opensidian.json. The keyring is the only store. There is no fallback:
  with no Secret Service every op fails with an `unsupported:` error naming
  the cause, and nothing pretends to have stored anything.
- One keyring item per (vault, id), attributes
  {app: "opensidian", vault: <canonical vault path>, key: <id>}. The vault
  is the SAME canonical key restricted mode is keyed by (plugins::trust_key).
  Ids are app-wide strings, as in stock: any plugin in the vault reads any id
  (SecretComponent lets the user pick an id another plugin set). The optional
  `plugin` attribute is not written: the sync JS API cannot name its caller
  honestly, and a guessed attribute would split one id into two items.
- Only a TRUSTED vault (restricted mode off, flag in ~/.opensidian.json,
  never in the vault) gets an answer. Restricted mode runs no plugin code, so
  a call from an untrusted vault is refused before the bus is touched.
- Transport: a "plain" Secret Service session. The value crosses the
  per-user session bus socket unencrypted, which is what the bus's own
  access control protects; nothing is written anywhere by this process.
- A locked collection/item is unlocked through the service's own prompt
  (Prompt.Prompt + Completed), at most PROMPT_WAIT; dismissed = an error.
*/

use std::collections::HashMap;

pub const APP: &str = "opensidian";
/// longest id (stock 1.13.7 accepts 64 chars, refuses 65: apioracle cpsecret.idlen) and the
/// longest value we pass to the keyring (a plugin API key is ~100 bytes)
pub const MAX_ID: usize = 64;
pub const MAX_VALUE: usize = 64 * 1024;

/// the error a restricted-mode vault gets; the bus is never touched
pub const REFUSED: &str = "EPERM: restricted mode, secret storage is off";

#[cfg_attr(windows, allow(dead_code))] // Windows has a keyring of its own (secrets_win.rs)
fn unsupported(cause: impl std::fmt::Display) -> String {
    format!("unsupported: secret storage needs a Secret Service (org.freedesktop.secrets) on the session bus: {cause}")
}
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn failed(what: &str, e: impl std::fmt::Display) -> String {
    format!("secret storage: {what}: {e}")
}

/// ids as the JS side accepts them, plus a length cap. The JS side owns
/// stock's exact rule; this is the backend's own floor (no attribute can
/// carry anything but [a-z0-9-]).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// the attribute set an item carries; `key: None` = every item of this vault
pub fn attrs<'a>(vault: &'a str, key: Option<&'a str>) -> HashMap<&'static str, &'a str> {
    let mut a = HashMap::from([("app", APP), ("vault", vault)]);
    if let Some(k) = key {
        a.insert("key", k);
    }
    a
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn label(vault: &str, key: &str) -> String {
    format!("Opensidian secret \"{key}\" ({vault})")
}

pub enum Op<'a> {
    Load,
    /// this vault's ids only, sorted: Settings ▸ Keychain lists ids and never needs a value
    Ids,
    Set(&'a str, &'a str),
    Delete(&'a str),
}

/// parse + validate one request; Err before any bus traffic
pub fn parse<'a>(op: &str, id: Option<&'a str>, value: Option<&'a str>) -> Result<Op<'a>, String> {
    let id_of = || -> Result<&'a str, String> {
        let id = id.ok_or("secret storage: id missing")?;
        if valid_id(id) { Ok(id) } else { Err(format!("secret storage: invalid id {id:?}")) }
    };
    match op {
        "load" => Ok(Op::Load),
        "ids" => Ok(Op::Ids),
        "set" => {
            let id = id_of()?;
            let v = value.ok_or("secret storage: value missing")?;
            if v.len() > MAX_VALUE {
                return Err(format!("secret storage: value too large ({} bytes, max {MAX_VALUE})", v.len()));
            }
            Ok(Op::Set(id, v))
        }
        "delete" => Ok(Op::Delete(id_of()?)),
        _ => Err(format!("secret storage: unknown op {op:?}")),
    }
}

#[cfg(target_os = "linux")]
mod bus {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use dbus::arg::{PropMap, RefArg, Variant};
    use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
    use dbus::blocking::Connection;
    use dbus::message::MatchRule;
    use dbus::Path as DPath;

    const BUS: &str = "org.freedesktop.secrets";
    const SVC_PATH: &str = "/org/freedesktop/secrets";
    const SVC: &str = "org.freedesktop.Secret.Service";
    const COLL: &str = "org.freedesktop.Secret.Collection";
    const ITEM: &str = "org.freedesktop.Secret.Item";
    const PROMPT: &str = "org.freedesktop.Secret.Prompt";
    const SESSION: &str = "org.freedesktop.Secret.Session";
    const CALL: Duration = Duration::from_secs(10);
    const PROMPT_WAIT: Duration = Duration::from_secs(120);

    /// the session-bus connection; no bus = unsupported (named)
    fn connect(addr: Option<&str>) -> Result<Connection, String> {
        match addr {
            Some(a) => Connection::new_address(a),
            None if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() => Connection::new_session(),
            // no address: the systemd user bus socket, if there is one. Never
            // libdbus's X11 autolaunch, which would START a bus (with no Secret
            // Service on it) as a side effect of a plugin reading a secret.
            None => match std::env::var_os("XDG_RUNTIME_DIR").map(|d| std::path::PathBuf::from(d).join("bus")) {
                Some(s) if s.exists() => Connection::new_address(&format!("unix:path={}", s.display())),
                _ => return Err(unsupported("no D-Bus session bus (DBUS_SESSION_BUS_ADDRESS unset, no $XDG_RUNTIME_DIR/bus)")),
            },
        }
        .map_err(|e| unsupported(format!("cannot connect to the D-Bus session bus ({})", e.message().unwrap_or("no reason given"))))
    }

    struct Svc {
        conn: Connection,
        session: DPath<'static>,
    }

    impl Drop for Svc {
        fn drop(&mut self) {
            let _ = self.conn.with_proxy(BUS, self.session.clone(), CALL).method_call::<(), _, _, _>(SESSION, "Close", ());
        }
    }

    fn is_root(p: &DPath) -> bool {
        &**p == "/"
    }

    impl Svc {
        fn open(addr: Option<&str>) -> Result<Svc, String> {
            let conn = connect(addr)?;
            let r: Result<(Variant<Box<dyn RefArg>>, DPath<'static>), _> =
                conn.with_proxy(BUS, SVC_PATH, CALL).method_call(SVC, "OpenSession", ("plain", Variant("")));
            let (_, session) = r.map_err(|e| {
                unsupported(format!(
                    "{} ({})",
                    e.name().unwrap_or("org.freedesktop.DBus.Error.Failed"),
                    e.message().unwrap_or("no Secret Service answered")
                ))
            })?;
            Ok(Svc { conn, session })
        }

        fn svc(&self) -> dbus::blocking::Proxy<'_, &Connection> {
            self.conn.with_proxy(BUS, SVC_PATH, CALL)
        }

        /// run a prompt the service handed back; Ok(()) once it completed, not dismissed
        fn prompt(&self, p: &DPath<'static>) -> Result<(), String> {
            if is_root(p) {
                return Ok(());
            }
            let done: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
            let d = done.clone();
            let rule = MatchRule::new_signal(PROMPT, "Completed").with_path(p.clone());
            let tok = self
                .conn
                .add_match(rule, move |(dismissed, _): (bool, Variant<Box<dyn RefArg>>), _, _| {
                    *d.lock().unwrap() = Some(dismissed);
                    false
                })
                .map_err(|e| failed("prompt", e))?;
            self.conn
                .with_proxy(BUS, p.clone(), CALL)
                .method_call::<(), _, _, _>(PROMPT, "Prompt", ("",))
                .map_err(|e| failed("prompt", e))?;
            let end = Instant::now() + PROMPT_WAIT;
            let r = loop {
                if let Some(dismissed) = *done.lock().unwrap() {
                    break if dismissed { Err("secret storage: the keyring unlock prompt was dismissed".to_string()) } else { Ok(()) };
                }
                if Instant::now() >= end {
                    let _ = self.conn.with_proxy(BUS, p.clone(), CALL).method_call::<(), _, _, _>(PROMPT, "Dismiss", ());
                    break Err("secret storage: the keyring unlock prompt timed out".to_string());
                }
                self.conn.process(Duration::from_millis(250)).map_err(|e| failed("prompt", e))?;
            };
            let _ = self.conn.remove_match(tok);
            r
        }

        fn unlock(&self, objs: Vec<DPath<'static>>) -> Result<(), String> {
            if objs.is_empty() {
                return Ok(());
            }
            let (_, p): (Vec<DPath<'static>>, DPath<'static>) =
                self.svc().method_call(SVC, "Unlock", (objs,)).map_err(|e| failed("unlock", e))?;
            self.prompt(&p)
        }

        /// every item matching `a`, unlocked (prompting if the service asks)
        fn search(&self, a: &HashMap<&str, &str>) -> Result<Vec<DPath<'static>>, String> {
            let (mut open, locked): (Vec<DPath<'static>>, Vec<DPath<'static>>) =
                self.svc().method_call(SVC, "SearchItems", (a.clone(),)).map_err(|e| failed("search", e))?;
            if !locked.is_empty() {
                self.unlock(locked.clone())?;
                open.extend(locked);
            }
            Ok(open)
        }

        fn value(&self, item: &DPath<'static>) -> Result<String, String> {
            let ((_, _, bytes, _),): ((DPath<'static>, Vec<u8>, Vec<u8>, String),) = self
                .conn
                .with_proxy(BUS, item.clone(), CALL)
                .method_call(ITEM, "GetSecret", (self.session.clone(),))
                .map_err(|e| failed("read", e))?;
            String::from_utf8(bytes).map_err(|_| "secret storage: a stored value is not UTF-8".to_string())
        }

        fn key_of(&self, item: &DPath<'static>) -> Result<String, String> {
            let a: HashMap<String, String> =
                self.conn.with_proxy(BUS, item.clone(), CALL).get(ITEM, "Attributes").map_err(|e| failed("read", e))?;
            a.get("key").cloned().ok_or_else(|| "secret storage: an item has no key attribute".to_string())
        }

        fn default_collection(&self) -> Result<DPath<'static>, String> {
            let (c,): (DPath<'static>,) =
                self.svc().method_call(SVC, "ReadAlias", ("default",)).map_err(|e| failed("default collection", e))?;
            if is_root(&c) {
                return Err(unsupported("the Secret Service has no default collection (create a keyring first)"));
            }
            Ok(c)
        }

        fn set(&self, vault: &str, key: &str, value: &str) -> Result<(), String> {
            let coll = self.default_collection()?;
            self.unlock(vec![coll.clone()])?;
            let a: HashMap<String, String> = attrs(vault, Some(key)).into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            let mut props: PropMap = HashMap::new();
            props.insert(format!("{ITEM}.Label"), Variant(Box::new(label(vault, key))));
            props.insert(format!("{ITEM}.Attributes"), Variant(Box::new(a)));
            let secret = (self.session.clone(), Vec::<u8>::new(), value.as_bytes().to_vec(), "text/plain; charset=utf8");
            let (_, p): (DPath<'static>, DPath<'static>) = self
                .conn
                .with_proxy(BUS, coll, CALL)
                .method_call(COLL, "CreateItem", (props, secret, true))
                .map_err(|e| failed("write", e))?;
            self.prompt(&p)
        }

        fn delete(&self, vault: &str, key: &str) -> Result<(), String> {
            for it in self.search(&attrs(vault, Some(key)))? {
                let (p,): (DPath<'static>,) =
                    self.conn.with_proxy(BUS, it, CALL).method_call(ITEM, "Delete", ()).map_err(|e| failed("delete", e))?;
                self.prompt(&p)?;
            }
            Ok(())
        }

        /// this vault's ids (sorted, deduplicated), no value is read
        fn ids(&self, vault: &str) -> Result<Vec<String>, String> {
            let mut out = std::collections::BTreeSet::new();
            for it in self.search(&attrs(vault, None))? {
                out.insert(self.key_of(&it)?);
            }
            Ok(out.into_iter().collect())
        }

        /// {id: value} of this vault's items (ids sorted)
        fn load(&self, vault: &str) -> Result<std::collections::BTreeMap<String, String>, String> {
            let mut out = std::collections::BTreeMap::new();
            for it in self.search(&attrs(vault, None))? {
                out.insert(self.key_of(&it)?, self.value(&it)?);
            }
            Ok(out)
        }
    }

    /// one op against the Secret Service at `addr` (None = the session bus)
    pub fn run(addr: Option<&str>, vault: &str, op: Op) -> Result<serde_json::Value, String> {
        let s = Svc::open(addr)?;
        match op {
            Op::Load => Ok(serde_json::json!({ "secrets": s.load(vault)? })),
            Op::Ids => Ok(serde_json::json!({ "ids": s.ids(vault)? })),
            Op::Set(k, v) => s.set(vault, k, v).map(|_| serde_json::Value::Null),
            Op::Delete(k) => s.delete(vault, k).map(|_| serde_json::Value::Null),
        }
    }
}
#[cfg(target_os = "linux")]
pub use bus::run;

/// macOS: the login Keychain (macos::keychain), same ops, same shapes as the Secret Service store
#[cfg(target_os = "macos")]
pub fn run(_addr: Option<&str>, vault: &str, op: Op) -> Result<serde_json::Value, String> {
    use crate::macos::keychain as kc;
    match op {
        Op::Load => Ok(serde_json::json!({ "secrets": kc::load(vault)? })),
        Op::Ids => Ok(serde_json::json!({ "ids": kc::ids(vault)? })),
        Op::Set(k, v) => kc::set(vault, k, v).map(|_| serde_json::Value::Null),
        Op::Delete(k) => kc::delete(vault, k).map(|_| serde_json::Value::Null),
    }
}

/// Windows: the user's Credential Manager (secrets_win.rs), same ops, same shapes
#[cfg(windows)]
pub use crate::secrets_win::run;

/// no OS keyring we speak elsewhere: unsupported, never a fallback store
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn run(_addr: Option<&str>, _vault: &str, _op: Op) -> Result<serde_json::Value, String> {
    Err(unsupported("this platform has no freedesktop Secret Service"))
}

/// trust gate + parse, before any bus traffic
pub fn admit<'a>(trusted: bool, op: &str, id: Option<&'a str>, value: Option<&'a str>) -> Result<Op<'a>, String> {
    if !trusted {
        return Err(REFUSED.into());
    }
    parse(op, id, value)
}

/// op "load" -> {secrets: {id: value}} | "ids" -> {ids: [id]} (no values) | "set" (id, value) -> null | "delete" (id) -> null.
/// Off the main thread: a keyring prompt can wait for the user.
#[tauri::command]
pub(crate) async fn secrets_op(
    v: tauri::State<'_, crate::Vault>,
    op: String,
    id: Option<String>,
    value: Option<String>,
    otel: Option<crate::perf::Ctx>,
) -> Result<serde_json::Value, String> {
    let root = crate::cur_vault(&v).ok_or("no vault open")?;
    // #127 (review #129): ONE key (canonical path + identity) for both the trust check and the
    // keyring namespace — a root symlink retargeted in between cannot get the check of the old target
    let key = crate::plugins::trust_key(&root);
    let trusted = crate::plugins::is_trusted_key(&crate::cfg_value(), &key);
    if let Err(e) = admit(trusted, &op, id.as_deref(), value.as_deref()) {
        eprintln!("[secrets] {op}: {e}");
        return Err(e);
    }
    let vault = key.path;
    let what = op.clone();
    let r = crate::span_timed!(otel => "secrets_op", tauri::async_runtime::spawn_blocking(move || {
        let op = parse(&op, id.as_deref(), value.as_deref())?;
        run(None, &vault, op)
    })
    .await);
    let r = r.map_err(|e| e.to_string())?;
    // the op and its error only — never an id's value
    match &r {
        Ok(_) => eprintln!("[secrets] {what}: ok"),
        Err(e) => eprintln!("[secrets] {what}: {e}"),
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_lowercase_alnum_dash_and_capped() {
        // stock accepts leading / trailing / doubled dashes (apioracle cpsecret.ids)
        for ok in ["a", "openai-key", "k1", "a-b-c", "0", "-a", "a-", "a--b", &"a".repeat(MAX_ID)] {
            assert!(valid_id(ok), "{ok}");
        }
        for bad in ["", "A", "a b", "a_b", "a/b", "ä", "a\0", "a.b", &"a".repeat(MAX_ID + 1)] {
            assert!(!valid_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn attributes_name_app_vault_and_key() {
        let a = attrs("/v", Some("k"));
        assert_eq!(a.len(), 3);
        assert_eq!((a["app"], a["vault"], a["key"]), ("opensidian", "/v", "k"));
        let all = attrs("/v", None);
        assert_eq!(all.len(), 2);
        assert!(!all.contains_key("key") && !all.contains_key("plugin"));
    }

    #[test]
    fn untrusted_vault_is_refused_before_parsing() {
        for op in ["load", "ids", "set", "delete", "bogus"] {
            assert_eq!(admit(false, op, Some("k"), Some("v")).err().as_deref(), Some(REFUSED));
        }
        assert!(matches!(admit(true, "load", None, None), Ok(Op::Load)));
        assert!(matches!(admit(true, "ids", None, None), Ok(Op::Ids)));
        assert!(matches!(admit(true, "set", Some("k"), Some("v")), Ok(Op::Set("k", "v"))));
    }

    #[test]
    fn bad_requests_fail_before_the_bus() {
        assert!(parse("set", Some("K"), Some("v")).is_err());
        assert!(parse("set", None, Some("v")).is_err());
        assert!(parse("set", Some("k"), None).is_err());
        assert!(parse("set", Some("k"), Some(&"x".repeat(MAX_VALUE + 1))).is_err());
        assert!(parse("delete", None, None).is_err());
        assert!(parse("drop", Some("k"), None).is_err());
    }

    /// no Secret Service reachable: every op is an `unsupported:` error that
    /// names the cause, and nothing is stored anywhere (no fallback exists).
    #[test]
    #[cfg(target_os = "linux")] // a Secret Service bus address; macOS uses the Keychain
    fn no_service_is_unsupported_not_a_fake_success() {
        let dir = std::env::temp_dir().join(format!("ossec-nobus-{}", std::process::id()));
        let addr = format!("unix:path={}/no-such-bus", dir.display());
        for op in [Op::Load, Op::Ids, Op::Set("k", "v"), Op::Delete("k")] {
            let e = run(Some(&addr), "/v", op).unwrap_err();
            assert!(e.starts_with("unsupported: secret storage needs a Secret Service"), "{e}");
            assert!(e.contains("cannot connect"), "{e}");
        }
        assert!(!dir.exists());
    }
}
