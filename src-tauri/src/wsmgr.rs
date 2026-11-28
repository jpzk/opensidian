// SPDX-License-Identifier: GPL-3.0-or-later
/* #94 WORKSPACE MANAGER — named layouts, Obsidian 1.13.7 core plugin "Workspaces" parity.

   THE FILE: <vault>/.obsidian/workspaces.json, stock's shape:
     {"workspaces": {"<name>": {main, left, right, left-ribbon?, active, ...}}, "active": "<name>"}
   Each entry is the layout object the R28 writer puts in workspace.json, MINUS `lastOpenFiles`
   (stock's getLayout() does not carry it: the recent-files list is a property of the session,
   not of a saved layout). Every key this module does not own — at the top level and inside an
   entry (a plugin's own key) — rides through untouched (serde_json
   preserve_order keeps the file order too).

   workspace.json (the LAST-SESSION layout, main.rs WS_REL) is NOT touched here: a load makes the
   saved layout the live one, and the ordinary R28 writer then records it there exactly as it
   records any other layout change.

   SAFETY, the same guarantees as workspace.json:
     - every write is vaultfs::write_atomic_durable (vault-dir-fd relative, `.obsidian` walked
       O_NOFOLLOW, RANDOM O_EXCL dot-temp, fsync, renameat, parent fsync) — a kill -9 at any
       instant leaves the old file or the new one, never half of one, and never a write outside
       the vault through a symlinked `.obsidian`;
     - the caller names the vault it serialized FROM and a disagreeing vault is refused (the
       vaultbleed rule of write_workspace): saved workspaces cannot leak between vaults;
     - a file that EXISTS but is not a workspaces document (corrupt, half-synced, hand-edited)
       is never overwritten: a read shows it as empty, a mutation refuses with WSS_CORRUPT. A
       user's saved layouts are not something a typo in a text editor may cost them;
     - the read refuses a symlinked `.obsidian` or file (never reads outside the vault) and
       caps the size.
   Read-modify-write is serialized by one process-wide lock: two mutations from one window can
   not interleave and lose one another. */
use serde_json::{Map, Value};
#[cfg(test)]
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Mutex;

pub const WSS_REL: &str = ".obsidian/workspaces.json";
/// a saved-layouts file bigger than this is not one this app wrote or will rewrite
pub const WSS_MAX_BYTES: u64 = 8 << 20;
/// the name is shown in a list and typed in a box; stock imposes no limit, a sane cap stops a paste accident
pub const WSS_NAME_MAX: usize = 200;
pub const WSS_CORRUPT: &str = "workspaces.json is not readable as a workspaces file; left untouched";
pub const WSS_TOO_BIG: &str = "workspaces.json would exceed the size limit; layout not saved (delete some workspaces)";

static LOCK: Mutex<()> = Mutex::new(());

/// what is on disk. Absent = no file (fresh), Doc = a parsed workspaces document, Corrupt = a file
/// that exists and must not be overwritten.
#[derive(Debug)]
pub enum OnDisk {
    Absent,
    Doc(Value),
    Corrupt,
}

fn is_doc(v: &Value) -> bool {
    match v {
        Value::Object(m) => match m.get("workspaces") {
            None => true, // stock writes {"workspaces":{}, ...}; a bare {} is an empty document too
            Some(w) => w.is_object(),
        },
        _ => false,
    }
}

/// read <root>/.obsidian/workspaces.json without following a symlink anywhere below the vault root
/// (#250: by the vaultfs fd walk; the type and size checks are on the opened fd)
pub fn read_in(root: &Path) -> OnDisk {
    let s = match crate::vaultfs::read_string(root, Path::new(WSS_REL), WSS_MAX_BYTES) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return OnDisk::Absent,
        // a symlinked / non-dir .obsidian, a symlinked / non-regular / oversized leaf: not ours
        Err(_) => return OnDisk::Corrupt,
    };
    match serde_json::from_str::<Value>(&s) {
        Ok(v) if is_doc(&v) => OnDisk::Doc(v),
        _ => OnDisk::Corrupt,
    }
}

/// the name rule: trimmed, non-empty, <= WSS_NAME_MAX chars, no control characters
pub fn clean_name(n: &str) -> Result<String, String> {
    let t = n.trim();
    if t.is_empty() {
        return Err("workspace name is empty".into());
    }
    if t.chars().count() > WSS_NAME_MAX {
        return Err(format!("workspace name is longer than {WSS_NAME_MAX} characters"));
    }
    if t.chars().any(char::is_control) {
        return Err("workspace name contains a control character".into());
    }
    Ok(t.to_string())
}

fn wmap(doc: &mut Value) -> &mut Map<String, Value> {
    let o = doc.as_object_mut().expect("is_doc checked");
    let w = o.entry("workspaces").or_insert_with(|| Value::Object(Map::new()));
    w.as_object_mut().expect("is_doc checked")
}

/// {"names": [...in file order], "active": "<name>" | ""} — `active` only if it names an entry
pub fn list(doc: &OnDisk) -> Value {
    let (names, active): (Vec<String>, String) = match doc {
        OnDisk::Doc(v) => {
            let names: Vec<String> = v
                .get("workspaces")
                .and_then(Value::as_object)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            let a = v.get("active").and_then(Value::as_str).unwrap_or("");
            let a = if names.iter().any(|n| n == a) { a.to_string() } else { String::new() };
            (names, a)
        }
        _ => (Vec::new(), String::new()),
    };
    serde_json::json!({ "names": names, "active": active, "corrupt": matches!(doc, OnDisk::Corrupt) })
}

pub fn get(doc: &OnDisk, name: &str) -> Option<Value> {
    match doc {
        OnDisk::Doc(v) => v.get("workspaces")?.get(name).cloned().filter(Value::is_object),
        _ => None,
    }
}

/// the pure ops: each takes the document it read and returns the document to write
pub fn op_save(mut doc: Value, name: &str, layout: &Value) -> Result<Value, String> {
    let name = clean_name(name)?;
    let Value::Object(lay) = layout else { return Err("layout is not an object".into()) };
    if !lay.get("main").is_some_and(Value::is_object) {
        return Err("layout has no main area".into());
    }
    let mut e = lay.clone();
    e.shift_remove("lastOpenFiles"); // stock's getLayout() does not save it (session state, not layout)
    // keys we do not own that an older save of this name carried (a plugin's key; a stale mtime is dropped)
    // survive an overwrite: the new layout replaces the layout, not the entry's foreign keys
    if let Some(Value::Object(old)) = wmap(&mut doc).get(&name) {
        for (k, v) in old {
            if !e.contains_key(k) && !matches!(k.as_str(), "main" | "left" | "right" | "left-ribbon" | "active" | "mtime") {
                e.insert(k.clone(), v.clone());
            }
        }
    }
    wmap(&mut doc).insert(name.clone(), Value::Object(e)); // existing key: replaced IN PLACE (order kept)
    doc.as_object_mut().unwrap().insert("active".into(), Value::String(name));
    Ok(doc)
}

pub fn op_delete(mut doc: Value, name: &str) -> Result<Value, String> {
    if wmap(&mut doc).shift_remove(name).is_none() {
        return Err(format!("no workspace named {name:?}"));
    }
    let o = doc.as_object_mut().unwrap();
    if o.get("active").and_then(Value::as_str) == Some(name) {
        o.insert("active".into(), Value::String(String::new()));
    }
    Ok(doc)
}

pub fn op_rename(mut doc: Value, from: &str, to: &str) -> Result<Value, String> {
    let to = clean_name(to)?;
    if from == to {
        return Ok(doc);
    }
    let w = wmap(&mut doc);
    if !w.contains_key(from) {
        return Err(format!("no workspace named {from:?}"));
    }
    if w.contains_key(&to) {
        return Err(format!("a workspace named {to:?} already exists"));
    }
    // rebuild to keep the entry at its position (a rename is not a move to the end of the list)
    let old = std::mem::take(w);
    for (k, v) in old {
        if k == from { w.insert(to.clone(), v); } else { w.insert(k, v); }
    }
    let o = doc.as_object_mut().unwrap();
    if o.get("active").and_then(Value::as_str) == Some(from) {
        o.insert("active".into(), Value::String(to));
    }
    Ok(doc)
}

pub fn op_activate(mut doc: Value, name: &str) -> Result<Value, String> {
    if !wmap(&mut doc).contains_key(name) {
        return Err(format!("no workspace named {name:?}"));
    }
    doc.as_object_mut().unwrap().insert("active".into(), Value::String(name.to_string()));
    Ok(doc)
}

/// read, apply `op`, write durably — under the process lock. A Corrupt file is refused, never replaced.
pub fn mutate_in(root: &Path, op: impl FnOnce(Value) -> Result<Value, String>) -> Result<Value, String> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let doc = match read_in(root) {
        OnDisk::Absent => serde_json::json!({ "workspaces": {}, "active": "" }),
        OnDisk::Doc(v) => v,
        OnDisk::Corrupt => return Err(WSS_CORRUPT.into()),
    };
    let out = op(doc)?;
    let bytes = serde_json::to_vec_pretty(&out).map_err(|e| e.to_string())?;
    // read_in treats a file over the cap as Corrupt and then refuses every op: never write one ourselves.
    // The old file stays in place (still readable); delete/rename never grow it, so they keep working.
    if bytes.len() as u64 > WSS_MAX_BYTES {
        return Err(WSS_TOO_BIG.into());
    }
    crate::vaultfs::write_atomic_durable(root, Path::new(WSS_REL), &bytes).map_err(|e| e.to_string())?;
    Ok(list(&OnDisk::Doc(out)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("os-wsmgr-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(p.join(".obsidian")).unwrap();
        p
    }
    fn lay(f: &str) -> Value {
        json!({"main": {"id": "m", "type": "split", "children": [{"id": "t", "type": "tabs", "children": [
            {"id": "l1", "type": "leaf", "state": {"type": "markdown", "state": {"file": f, "mode": "source", "source": false}}}]}],
            "direction": "vertical"}, "left": {"id": "L", "type": "split", "children": []}, "active": "l1",
            "lastOpenFiles": [f]})
    }
    fn names(r: &Value) -> Vec<String> {
        r["names"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn save_load_rename_delete_roundtrip() {
        let root = tmp("rt");
        let r = mutate_in(&root, |d| op_save(d, "A", &lay("a.md"))).unwrap();
        assert_eq!(names(&r), ["A"]);
        assert_eq!(r["active"], "A");
        mutate_in(&root, |d| op_save(d, "B", &lay("b.md"))).unwrap();
        let OnDisk::Doc(_) = read_in(&root) else { panic!("written file must parse") };
        let a = get(&read_in(&root), "A").unwrap();
        assert_eq!(a["main"]["children"][0]["children"][0]["state"]["state"]["file"], "a.md");
        assert!(a.get("lastOpenFiles").is_none(), "a saved layout carries no lastOpenFiles (stock getLayout)");
        // overwrite keeps position, replaces the layout
        let r = mutate_in(&root, |d| op_save(d, "A", &lay("c.md"))).unwrap();
        assert_eq!(names(&r), ["A", "B"]);
        assert_eq!(get(&read_in(&root), "A").unwrap()["active"], "l1");
        assert_eq!(get(&read_in(&root), "A").unwrap()["main"]["children"][0]["children"][0]["state"]["state"]["file"], "c.md");
        // rename keeps position and follows active
        let r = mutate_in(&root, |d| op_rename(d, "A", "Z")).unwrap();
        assert_eq!(names(&r), ["Z", "B"]);
        assert_eq!(r["active"], "Z");
        assert!(mutate_in(&root, |d| op_rename(d, "Z", "B")).is_err(), "rename onto an existing name is refused");
        let r = mutate_in(&root, |d| op_delete(d, "Z")).unwrap();
        assert_eq!(names(&r), ["B"]);
        assert_eq!(r["active"], "", "deleting the active workspace clears active");
        let r = mutate_in(&root, |d| op_activate(d, "B")).unwrap();
        assert_eq!(r["active"], "B");
        let left: Vec<_> = fs::read_dir(root.join(".obsidian")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, ["workspaces.json"], "no temp left beside the file");
    }

    #[test]
    fn stock_file_loads_and_foreign_keys_survive() {
        let root = tmp("stock");
        let stock = r#"{"workspaces":{"Writing":{"main":{"id":"a","type":"split","children":[],"direction":"vertical"},"left":{"id":"b","type":"split","children":[]},"left-ribbon":{"hiddenItems":{"canvas:Create new canvas":false}},"active":"x","mtime":"2025-01-01T00:00:00+01:00","plug":1}},"active":"Writing","zz":true}"#;
        fs::write(root.join(WSS_REL), stock).unwrap();
        let r = list(&read_in(&root));
        assert_eq!(names(&r), ["Writing"]);
        assert_eq!(r["active"], "Writing");
        mutate_in(&root, |d| op_save(d, "Writing", &lay("a.md"))).unwrap();
        let OnDisk::Doc(v) = read_in(&root) else { panic!() };
        assert_eq!(v["zz"], true, "unknown top-level key kept");
        assert_eq!(v["workspaces"]["Writing"]["plug"], 1, "an entry's foreign key survives an overwrite");
    }

    #[test]
    fn corrupt_is_never_overwritten() {
        let root = tmp("bad");
        fs::write(root.join(WSS_REL), "{not json").unwrap();
        assert!(matches!(read_in(&root), OnDisk::Corrupt));
        assert_eq!(list(&read_in(&root))["names"], json!([]));
        assert_eq!(mutate_in(&root, |d| op_save(d, "A", &lay("a.md"))).unwrap_err(), WSS_CORRUPT);
        assert_eq!(fs::read_to_string(root.join(WSS_REL)).unwrap(), "{not json");
        fs::write(root.join(WSS_REL), "[1,2]").unwrap();
        assert!(mutate_in(&root, |d| op_delete(d, "A")).is_err());
        assert_eq!(fs::read_to_string(root.join(WSS_REL)).unwrap(), "[1,2]");
    }

    #[test]
    fn save_never_writes_a_file_over_the_cap() {
        // codeant #110: a save that grew the file past WSS_MAX_BYTES made every later read Corrupt
        let root = tmp("cap");
        let mk = |pad: &str| json!({"workspaces": {"big": {"main": {"id": "m", "type": "split", "children": []}, "pad": pad}}, "active": "big"});
        let l0 = serde_json::to_vec_pretty(&mk("")).unwrap().len() as u64;
        let near = mk(&"x".repeat((WSS_MAX_BYTES - l0 - 50) as usize));
        let before = serde_json::to_vec_pretty(&near).unwrap();
        assert!(before.len() as u64 <= WSS_MAX_BYTES);
        fs::write(root.join(WSS_REL), &before).unwrap();
        assert!(matches!(read_in(&root), OnDisk::Doc(_)));
        assert_eq!(mutate_in(&root, |d| op_save(d, "A", &lay("a.md"))).unwrap_err(), WSS_TOO_BIG);
        assert_eq!(fs::read(root.join(WSS_REL)).unwrap(), before, "refused save leaves the file byte-identical");
        assert!(matches!(read_in(&root), OnDisk::Doc(_)), "still readable, not Corrupt");
        let r = mutate_in(&root, |d| op_delete(d, "big")).unwrap(); // shrinking ops keep working
        assert_eq!(names(&r), Vec::<String>::new());
        assert_eq!(names(&mutate_in(&root, |d| op_save(d, "A", &lay("a.md"))).unwrap()), ["A"]);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_obsidian_is_refused() {
        let root = tmp("sym");
        let out = tmp("symout");
        fs::remove_dir_all(root.join(".obsidian")).unwrap();
        std::os::unix::fs::symlink(out.join(".obsidian"), root.join(".obsidian")).unwrap();
        assert!(mutate_in(&root, |d| op_save(d, "A", &lay("a.md"))).is_err());
        assert!(!out.join(WSS_REL).exists(), "nothing written through the symlink");
    }

    #[test]
    #[cfg(unix)] // symlinks, FIFOs or mode bits
    fn read250_leaf_symlink_devzero_fifo_and_oversize_are_not_read() {
        let root = tmp("r250");
        let out = tmp("r250out");
        let doc = r#"{"workspaces":{"secret":{}},"active":"secret"}"#;
        fs::write(out.join("w.json"), doc).unwrap();
        let p = root.join(WSS_REL);
        let fifo = |p: &Path| {
            let c = std::ffi::CString::new(p.to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        };
        for case in ["outside", "devzero", "fifo", "over"] {
            let _ = fs::remove_file(&p);
            match case {
                "outside" => std::os::unix::fs::symlink(out.join("w.json"), &p).unwrap(),
                "devzero" => std::os::unix::fs::symlink("/dev/zero", &p).unwrap(),
                "fifo" => fifo(&p),
                _ => {
                    // a VALID document one byte over the cap (trailing JSON whitespace)
                    let mut b = doc.as_bytes().to_vec();
                    b.resize(WSS_MAX_BYTES as usize + 1, b' ');
                    fs::write(&p, b).unwrap()
                }
            }
            let r = root.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || tx.send(matches!(read_in(&r), OnDisk::Corrupt)).unwrap());
            let corrupt = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap_or_else(|_| panic!("{case}: read_in hung"));
            assert!(corrupt, "{case}: must not be read as a document");
        }
        // a regular file is still read
        let _ = fs::remove_file(&p);
        fs::write(&p, doc).unwrap();
        assert!(matches!(read_in(&root), OnDisk::Doc(_)));
    }

    #[test]
    fn names_and_layout_validated() {
        assert!(clean_name("  ").is_err());
        assert!(clean_name("a\nb").is_err());
        assert!(clean_name(&"x".repeat(WSS_NAME_MAX + 1)).is_err());
        assert_eq!(clean_name("  Writing ").unwrap(), "Writing");
        let d = json!({"workspaces": {}});
        assert!(op_save(d.clone(), "A", &json!([1])).is_err());
        assert!(op_save(d, "A", &json!({"left": {}})).is_err());
    }
}
