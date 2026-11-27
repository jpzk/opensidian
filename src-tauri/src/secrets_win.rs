// SPDX-License-Identifier: GPL-3.0-or-later
/* secrets_win — app.secretStorage on Windows, in the Credential Manager.

   The contract is secrets.rs's (read it first): the OS keyring is the ONLY
   store, never a file of ours, and there is no fallback. On Windows the user's
   keyring is the Credential Manager ("Windows Credentials" in the control
   panel): each entry is encrypted with the user's logon key by the system
   (DPAPI) and is readable only inside that user's session.

   One generic credential per (vault, id):
     TargetName  opensidian:<canonical vault path>:<id>
     UserName    opensidian
     Comment     the same human label the Secret Service item carries
     Blob        the value, UTF-8
     Persist     LOCAL_MACHINE — kept on this computer, never roamed to another
                 by a domain profile
   An id is [a-z0-9-] (secrets::valid_id), so it holds no ':' and the id of an
   entry is whatever follows the LAST ':' of its target name; the vault path may
   contain one (`C:\...`). Listing a vault is CredEnumerateW with the filter
   `opensidian:<vault>:*`.

   ONE LIMIT, stated to the caller rather than worked around: a generic
   credential's blob is at most CRED_MAX_CREDENTIAL_BLOB_SIZE (2560) bytes,
   where the Secret Service takes our full MAX_VALUE. A longer value is refused
   with an error that says so. It is not split over several entries: a secret
   that is half written is worse than one that is refused. */

use crate::secrets::{label, Op, APP};
use std::collections::BTreeMap;
use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
use windows_sys::Win32::Security::Credentials::*;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn failed(what: &str) -> String {
    format!("secret storage: {what}: {}", std::io::Error::last_os_error())
}

fn prefix(vault: &str) -> String {
    format!("{APP}:{vault}:")
}

/// a NUL-terminated UTF-16 string the system handed us
/// SAFETY: `p` is null or points at a NUL-terminated buffer that outlives the call
unsafe fn from_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

fn set(vault: &str, key: &str, value: &str) -> Result<(), String> {
    let max = CRED_MAX_CREDENTIAL_BLOB_SIZE as usize;
    if value.len() > max {
        return Err(format!("secret storage: value too large for the Windows Credential Manager ({} bytes, max {max})", value.len()));
    }
    let (mut target, mut user, mut comment) = (wide(&format!("{}{key}", prefix(vault))), wide(APP), wide(&label(vault, key)));
    let mut blob = value.as_bytes().to_vec();
    let c = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: target.as_mut_ptr(),
        Comment: comment.as_mut_ptr(),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: user.as_mut_ptr(),
        ..Default::default()
    };
    // SAFETY: every pointer in `c` names a live, NUL-terminated (or sized) buffer
    let ok = unsafe { CredWriteW(&c, 0) };
    blob.fill(0); // our copy of the value does not linger in freed memory
    if ok == 0 { Err(failed("write")) } else { Ok(()) }
}

fn delete(vault: &str, key: &str) -> Result<(), String> {
    let target = wide(&format!("{}{key}", prefix(vault)));
    // SAFETY: NUL-terminated target name
    if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 && unsafe { GetLastError() } != ERROR_NOT_FOUND {
        return Err(failed("delete"));
    }
    Ok(()) // deleting what is not there is not an error (as on the Secret Service)
}

/// {id: value} of this vault's entries; `values` false = ids only, no blob is copied
fn load(vault: &str, values: bool) -> Result<BTreeMap<String, String>, String> {
    let pre = prefix(vault);
    let filter = wide(&format!("{pre}*"));
    let (mut n, mut list): (u32, *mut *mut CREDENTIALW) = (0, std::ptr::null_mut());
    // SAFETY: out-parameters of the documented shapes; the array is freed below
    if unsafe { CredEnumerateW(filter.as_ptr(), 0, &mut n, &mut list) } == 0 {
        return if unsafe { GetLastError() } == ERROR_NOT_FOUND { Ok(BTreeMap::new()) } else { Err(failed("list")) };
    }
    let mut out = BTreeMap::new();
    let mut bad = None;
    for i in 0..n as usize {
        // SAFETY: `list` holds `n` valid pointers until CredFree
        let c = unsafe { &**list.add(i) };
        let target = unsafe { from_wide(c.TargetName) };
        // the filter is matched without case by the system; the id is what follows the vault
        let Some(id) = target.get(pre.len()..).filter(|id| crate::secrets::valid_id(id) && target.len() == pre.len() + id.len()) else {
            continue;
        };
        let value = if values {
            // SAFETY: the blob is CredentialBlobSize bytes long
            let bytes = if c.CredentialBlob.is_null() { &[][..] } else { unsafe { std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize) } };
            match String::from_utf8(bytes.to_vec()) {
                Ok(v) => v,
                Err(_) => {
                    bad = Some("secret storage: a stored value is not UTF-8".to_string());
                    break;
                }
            }
        } else {
            String::new()
        };
        out.insert(id.to_string(), value);
    }
    // SAFETY: the buffer CredEnumerateW allocated, freed once
    unsafe { CredFree(list as *const core::ffi::c_void) };
    match bad {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

/// one op against the Credential Manager. `_addr` is the Secret Service's bus
/// address on Linux and means nothing here.
pub fn run(_addr: Option<&str>, vault: &str, op: Op) -> Result<serde_json::Value, String> {
    match op {
        Op::Load => Ok(serde_json::json!({ "secrets": load(vault, true)? })),
        Op::Ids => Ok(serde_json::json!({ "ids": load(vault, false)?.into_keys().collect::<Vec<_>>() })),
        Op::Set(k, v) => set(vault, k, v).map(|_| serde_json::Value::Null),
        Op::Delete(k) => delete(vault, k).map(|_| serde_json::Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real store, under a vault name nothing else uses, cleaned up at the
    /// end. A session with no credential store (some service accounts) skips
    /// loudly rather than pass silently.
    #[test]
    fn secrets_win_round_trips_per_vault_and_leaves_nothing_behind() {
        // the vault key as the app makes it on Windows: canonical, so with the \\?\ prefix
        // (its '?' goes into the CredEnumerateW filter, which must still match it literally)
        let v = format!(r"\\?\C:\opensidian-test\vault-{}-{:x}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let other = format!("{v}-other");
        if let Err(e) = run(None, &v, Op::Set("probe", "x")) {
            eprintln!("secrets_win test SKIPPED: this session has no usable Credential Manager ({e})");
            return;
        }
        let r = (|| -> Result<(), String> {
            run(None, &v, Op::Set("openai-key", "sk-é-ü 🔑"))?;
            run(None, &v, Op::Set("empty", ""))?;
            run(None, &other, Op::Set("openai-key", "theirs"))?;
            assert_eq!(run(None, &v, Op::Ids)?, serde_json::json!({"ids": ["empty", "openai-key", "probe"]}));
            assert_eq!(run(None, &v, Op::Load)?, serde_json::json!({"secrets": {"empty": "", "openai-key": "sk-é-ü 🔑", "probe": "x"}}));
            assert_eq!(run(None, &other, Op::Load)?, serde_json::json!({"secrets": {"openai-key": "theirs"}}), "another vault sees only its own");
            run(None, &v, Op::Set("openai-key", "replaced"))?;
            assert_eq!(run(None, &v, Op::Load)?["secrets"]["openai-key"], "replaced");
            let big = "x".repeat(CRED_MAX_CREDENTIAL_BLOB_SIZE as usize + 1);
            let e = run(None, &v, Op::Set("big", &big)).unwrap_err();
            assert!(e.contains("too large for the Windows Credential Manager"), "{e}");
            assert!(run(None, &v, Op::Load)?["secrets"].get("big").is_none(), "a refused value is not half stored");
            run(None, &v, Op::Delete("openai-key"))?;
            run(None, &v, Op::Delete("openai-key"))?; // twice: not an error
            assert_eq!(run(None, &v, Op::Ids)?, serde_json::json!({"ids": ["empty", "probe"]}));
            Ok(())
        })();
        for (vault, id) in [(&v, "probe"), (&v, "empty"), (&v, "openai-key"), (&v, "big"), (&other, "openai-key")] {
            let _ = run(None, vault, Op::Delete(id));
        }
        r.unwrap();
        assert_eq!(run(None, &v, Op::Ids).unwrap(), serde_json::json!({"ids": []}), "nothing left in the user's store");
        assert_eq!(run(None, &other, Op::Ids).unwrap(), serde_json::json!({"ids": []}));
    }
}
