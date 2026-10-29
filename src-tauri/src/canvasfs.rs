// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* canvas-core: the file I/O of the Canvas core plugin (JSON Canvas 1.0,
   https://jsoncanvas.org/spec/1.0/). Canvas is an APP feature, not a
   community plugin, so it works in restricted mode — and therefore gets its
   own narrow surface instead of plugin_vfs (which an untrusted vault answers
   with EPERM):

     list            every regular `*.canvas` file in the vault, vault-relative,
                     sorted; hidden components (.obsidian, .trash, ...) and
                     symlinks are never walked or listed
     media           every regular audio / video file (MEDIA_EXTS), same walk, sorted (goal cv-journeys:
                     the explorer rows and file model of the files a card plays)
     read  <path>    the file's text (UTF-8, at most CANVAS_MAX bytes)
     write <path>    atomic replace of an EXISTING canvas file
     create <path>   a NEW canvas file (EEXIST if anything is there); the
                     parent folder must exist
     rename <path>   to the .canvas path in data (EEXIST if taken; parent must exist)
     trash <path>    move into .trash/ (never over an earlier delete); returns where
     links           {canvas: [file-card targets]} for every listed canvas
     relink          data {from,to}: every file card naming `from` now names `to`
                     (only those string tokens change); returns the canvases written

   Every path goes through vaultfs::plugin_rel (no absolute / dot / NUL /
   backslash / percent-encoded forms) and must name a `.canvas` leaf with no
   hidden component, so this command can never touch a note, a config file
   or anything outside the vault. The bytes are never reformatted here:
   whatever text the view hands over is what lands on disk, and a read
   returns the file exactly as stored (byte-stable round trip). */

use crate::vaultfs::{self, Vault};
use serde_json::{json, Value};
use std::io;
use std::path::{Path, PathBuf};

/// the largest canvas file read or written (a canvas is JSON text)
pub const CANVAS_MAX: u64 = 16 << 20;
pub const EXT: &str = ".canvas";
/// the vault trash folder a deleted canvas moves to (same as a deleted note)
pub const TRASH: &str = ".trash";

fn err(op: &str, path: &str, why: &str) -> String {
    format!("{why}, canvas {op} '{path}'")
}

/// vault-relative path of a canvas file, or the refusal
pub fn canvas_rel(op: &str, path: &str) -> Result<PathBuf, String> {
    let rel = vaultfs::plugin_rel(path).map_err(|e| vaultfs::plugin_err(op, path, &e))?;
    let comps: Vec<&std::ffi::OsStr> = rel.iter().collect();
    let Some(leaf) = comps.last().and_then(|c| c.to_str()) else {
        return Err(err(op, path, "EINVAL: not a canvas path"));
    };
    if comps.iter().any(|c| c.as_encoded_bytes().first() == Some(&b'.')) {
        return Err(err(op, path, "EPERM: hidden path component"));
    }
    if !leaf.ends_with(EXT) || leaf.len() == EXT.len() {
        return Err(err(op, path, "EINVAL: not a .canvas file"));
    }
    Ok(rel)
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<String>, depth: usize, keep: fn(&str) -> bool) {
    if depth >= vaultfs::MAX_WALK_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(n) = name.to_str() else { continue };
        if n.starts_with('.') {
            continue;
        }
        // DirEntry::file_type does not follow symlinks: a link is neither
        let Ok(t) = e.file_type() else { continue };
        let p = e.path();
        if t.is_dir() {
            walk(&p, base, out, depth + 1, keep);
        } else if t.is_file() && keep(n) {
            if let Ok(rel) = p.strip_prefix(base) {
                if let Some(s) = rel.to_str() {
                    out.push(s.to_string());
                }
            }
        }
    }
}

fn is_canvas(n: &str) -> bool {
    n.ends_with(EXT) && n.len() > EXT.len()
}

/// goal cv-journeys: the audio / video types a file card plays (ui/plugins/canvas-cards.js AUD + VID). Images and
/// PDFs are listed by list_images (index::IMG_EXTS + EMBED_ONLY_EXTS); these are in no other listing.
pub const MEDIA_EXTS: [&str; 11] = ["mp3", "wav", "m4a", "ogg", "3gp", "flac", "mp4", "webm", "ogv", "mov", "mkv"];

fn is_media(n: &str) -> bool {
    n.rsplit_once('.').is_some_and(|(s, e)| !s.is_empty() && MEDIA_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

fn list_by(root: &Path, keep: fn(&str) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(c) = root.canonicalize() {
        walk(&c, &c, &mut out, 0, keep);
    }
    out.sort();
    out
}

pub fn list(root: &Path) -> Vec<String> {
    list_by(root, is_canvas)
}

/// every regular audio / video file in the vault (same walk: no hidden component, no symlink), sorted
pub fn media(root: &Path) -> Vec<String> {
    list_by(root, is_media)
}

fn exists(v: &Vault, rel: &Path) -> io::Result<bool> {
    match v.p_stat(rel) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// `text` with the "file" value of every file card (an object directly in the root's "nodes" array whose
/// "type" is "file") equal to `from` replaced by `to`; None when the text is not JSON or nothing matched.
/// A byte splice of those string tokens only: whitespace, key order, number spelling and every other
/// value stay exactly as they were.
fn relink_text(text: &str, from: &str, to: &str) -> Option<String> {
    serde_json::from_str::<serde::de::IgnoredAny>(text).ok()?;
    struct Obj {
        key: Option<String>,
        want_key: bool,
        card: bool,
        typ: Option<String>,
        file: Option<(usize, usize, String)>,
    }
    enum F {
        O(Obj),
        A,
    }
    let b = text.as_bytes();
    let (mut st, mut cuts, mut i): (Vec<F>, Vec<(usize, usize)>, usize) = (Vec::new(), Vec::new(), 0);
    while i < b.len() {
        match b[i] {
            b'{' => {
                let card = st.len() == 2
                    && matches!(st[1], F::A)
                    && matches!(&st[0], F::O(o) if o.key.as_deref() == Some("nodes"));
                st.push(F::O(Obj { key: None, want_key: true, card, typ: None, file: None }));
            }
            b'[' => st.push(F::A),
            b'}' => {
                if let Some(F::O(o)) = st.pop() {
                    if let (true, Some("file"), Some((s, e, f))) = (o.card, o.typ.as_deref(), o.file) {
                        if f == from {
                            cuts.push((s, e));
                        }
                    }
                }
            }
            b']' => {
                st.pop();
            }
            b':' => {
                if let Some(F::O(o)) = st.last_mut() {
                    o.want_key = false;
                }
            }
            b',' => {
                if let Some(F::O(o)) = st.last_mut() {
                    o.want_key = true;
                    o.key = None;
                }
            }
            b'"' => {
                let s = i;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                let e = i + 1;
                let val: String = serde_json::from_str(text.get(s..e)?).ok()?;
                if let Some(F::O(o)) = st.last_mut() {
                    if o.want_key {
                        o.key = Some(val);
                    } else if o.card && o.key.as_deref() == Some("type") {
                        o.typ = Some(val);
                    } else if o.card && o.key.as_deref() == Some("file") {
                        o.file = Some((s, e, val));
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    if cuts.is_empty() {
        return None;
    }
    let rep = serde_json::to_string(to).ok()?;
    let mut out = String::with_capacity(text.len() + cuts.len() * rep.len());
    let mut at = 0;
    for (s, e) in cuts {
        out.push_str(&text[at..s]);
        out.push_str(&rep);
        at = e;
    }
    out.push_str(&text[at..]);
    Some(out)
}

/// the empty JSON Canvas document: an object with no keys, or only an empty "nodes" and/or "edges" array
fn is_empty_doc(b: &[u8]) -> bool {
    match serde_json::from_slice::<Value>(b) {
        Ok(Value::Object(m)) => m.iter().all(|(k, v)| (k == "nodes" || k == "edges") && v.as_array().is_some_and(|a| a.is_empty())),
        _ => false,
    }
}

pub fn op(root: &Path, op: &str, path: &str, data: Option<&str>) -> Result<Value, String> {
    if op == "list" {
        return Ok(json!(list(root)));
    }
    if op == "media" {
        return Ok(json!(media(root)));
    }
    if op == "links" {
        // every canvas -> the vault paths its file cards point at (CV-141: a file card is a link of the canvas)
        let v = Vault::open(root).map_err(|x| vaultfs::plugin_err(op, path, &x))?;
        let mut m = serde_json::Map::new();
        for c in list(root) {
            let Ok(b) = v.p_read(Path::new(&c), CANVAS_MAX) else { continue };
            let Ok(doc) = serde_json::from_slice::<Value>(&b) else { continue };
            let mut fs: Vec<String> = doc
                .get("nodes")
                .and_then(Value::as_array)
                .map(|ns| {
                    ns.iter()
                        .filter(|n| n.get("type").and_then(Value::as_str) == Some("file"))
                        .filter_map(|n| n.get("file").and_then(Value::as_str).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            fs.sort();
            fs.dedup();
            if !fs.is_empty() {
                m.insert(c, json!(fs));
            }
        }
        return Ok(Value::Object(m));
    }
    if op == "relink" {
        // CV-143: data = {"from": <vault path>, "to": <vault path>}; every listed canvas whose FILE cards name
        // `from` gets those "file" values set to `to`, every other byte as it was. Called only after the
        // user's R34 consent (main.js runUpdateLinks0). Returns the canvases written, sorted.
        let d = data.ok_or_else(|| err(op, path, "EINVAL: missing data"))?;
        let m: Value = serde_json::from_str(d).map_err(|_| err(op, path, "EINVAL: data is not {from,to}"))?;
        let (Some(from), Some(to)) = (m.get("from").and_then(Value::as_str), m.get("to").and_then(Value::as_str)) else {
            return Err(err(op, path, "EINVAL: data is not {from,to}"));
        };
        for p in [from, to] {
            vaultfs::plugin_rel(p).map_err(|x| vaultfs::plugin_err(op, p, &x))?;
        }
        let v = Vault::open(root).map_err(|x| vaultfs::plugin_err(op, path, &x))?;
        let mut wrote = Vec::new();
        if from == to {
            return Ok(json!(wrote));
        }
        for c in list(root) {
            let Ok(rel) = canvas_rel(op, &c) else { continue };
            let Ok(b) = v.p_read(&rel, CANVAS_MAX) else { continue };
            let Ok(text) = String::from_utf8(b) else { continue };
            let Some(out) = relink_text(&text, from, to) else { continue };
            if out.len() as u64 > CANVAS_MAX {
                return Err(err(op, &c, "EFBIG: canvas too large"));
            }
            v.p_write(&rel, out.as_bytes(), false).map_err(|x| vaultfs::plugin_err(op, &c, &x))?;
            wrote.push(c);
        }
        return Ok(json!(wrote));
    }
    let rel = canvas_rel(op, path)?;
    let e = |x: io::Error| vaultfs::plugin_err(op, path, &x);
    let v = Vault::open(root).map_err(e)?;
    let body = || -> Result<&[u8], String> {
        let d = data.ok_or_else(|| err(op, path, "EINVAL: missing data"))?;
        if d.len() as u64 > CANVAS_MAX {
            return Err(err(op, path, "EFBIG: canvas too large"));
        }
        Ok(d.as_bytes())
    };
    match op {
        "read" => {
            let b = v.p_read(&rel, CANVAS_MAX).map_err(e)?;
            String::from_utf8(b).map(Value::String).map_err(|_| err(op, path, "EILSEQ: not UTF-8 text"))
        }
        "write" => {
            let b = body()?;
            match v.p_stat(&rel).map_err(e)? {
                s if s.kind == "file" => {}
                _ => return Err(err(op, path, "EISDIR: not a file")),
            }
            v.p_write(&rel, b, false).map(|_| Value::Null).map_err(e)
        }
        "create" => {
            // a new canvas holding the empty document is made as an EMPTY file (0 bytes); the board writes {} when
            // it is first left (CVR-A-004)
            let b = body()?;
            let b: &[u8] = if is_empty_doc(b) { b"" } else { b };
            if exists(&v, &rel).map_err(e)? {
                return Err(err(op, path, "EEXIST: file already exists"));
            }
            v.p_write(&rel, b, false).map(|_| Value::Null).map_err(e)
        }
        "rename" => {
            // data = the new vault-relative .canvas path; never over an existing entry, parent must exist
            let to = data.ok_or_else(|| err(op, path, "EINVAL: missing data"))?;
            let trel = canvas_rel(op, to)?;
            match v.p_stat(&rel).map_err(e)? {
                s if s.kind == "file" => {}
                _ => return Err(err(op, path, "EISDIR: not a file")),
            }
            if exists(&v, &trel).map_err(e)? {
                return Err(err(op, to, "EEXIST: file already exists"));
            }
            v.p_rename(&rel, &trel).map(|_| Value::Null).map_err(e)
        }
        "trash" => {
            // move to <vault>/.trash/<stem>.canvas (stem.1.canvas, ... if taken); returns the trash path
            let stem = rel.file_name().and_then(|s| s.to_str()).map(|s| s[..s.len() - EXT.len()].to_string()).unwrap_or_default();
            let tdir = Path::new(TRASH);
            v.mkdir_p(tdir).map_err(e)?;
            v.trash(&rel, tdir, &stem, &EXT[1..]).map(|d| Value::String(d.display().to_string())).map_err(e)
        }
        _ => Err(format!("ENOSYS: canvas op {op:?} does not exist")),
    }
}

/* canvas_attach: a NEW image file inside the vault, written from base64 bytes
   (export as image, pasted / dropped images). The byte twin of `create` above,
   and in the same class: vault-confined, never trust-gated, never an overwrite.

     canvas_attach {path, dataB64} -> the vault-relative path written

   path      vault-relative, through vaultfs::plugin_rel (no absolute / dot / NUL /
             backslash / percent-encoded forms), no hidden component, a leaf
             ending in one of ATTACH_EXT (case-insensitive); the parent folder
             must exist; nothing may be there yet (a symlink included: O_EXCL)
   dataB64   standard base64 (padding optional), at most ATTACH_MAX bytes decoded

   Named errors (the code is the text before ':'): EINVAL (bad path, bad
   extension, empty or non-base64 data), EPERM (hidden component, vault escape),
   EEXIST (taken: the caller picks a free name), EFBIG (over ATTACH_MAX), ENOENT
   (no parent folder). A failed write removes the partial file. */

/// the largest file canvas_attach writes
pub const ATTACH_MAX: u64 = 64 << 20;
/// the leaf extensions canvas_attach accepts (lower case, compared case-insensitively)
pub const ATTACH_EXT: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

pub fn attach(root: &Path, path: &str, data_b64: &str) -> Result<String, String> {
    use std::io::Write;
    let op = "attach";
    let rel = vaultfs::plugin_rel(path).map_err(|e| vaultfs::plugin_err(op, path, &e))?;
    let comps: Vec<&std::ffi::OsStr> = rel.iter().collect();
    let Some(leaf) = comps.last().and_then(|c| c.to_str()) else {
        return Err(err(op, path, "EINVAL: not a file path"));
    };
    if comps.iter().any(|c| c.as_encoded_bytes().first() == Some(&b'.')) {
        return Err(err(op, path, "EPERM: hidden path component"));
    }
    let ext = leaf.rsplit_once('.').map(|(s, x)| (s, x.to_ascii_lowercase()));
    if !matches!(&ext, Some((s, x)) if !s.is_empty() && ATTACH_EXT.contains(&x.as_str())) {
        return Err(err(op, path, "EINVAL: not an image file (png, jpg, jpeg, gif, webp)"));
    }
    if data_b64.is_empty() {
        return Err(err(op, path, "EINVAL: empty data"));
    }
    if data_b64.len() as u64 > ATTACH_MAX.div_ceil(3) * 4 {
        return Err(err(op, path, "EFBIG: image too large"));
    }
    let bytes = vaultfs::b64_decode(data_b64).ok_or_else(|| err(op, path, "EINVAL: data is not base64"))?;
    if bytes.is_empty() {
        return Err(err(op, path, "EINVAL: empty data"));
    }
    if bytes.len() as u64 > ATTACH_MAX {
        return Err(err(op, path, "EFBIG: image too large"));
    }
    let e = |x: io::Error| vaultfs::plugin_err(op, path, &x);
    let v = Vault::open(root).map_err(e)?;
    let mut f = v.create_new(&rel, false).map_err(|x| {
        if x.kind() == io::ErrorKind::AlreadyExists {
            err(op, path, "EEXIST: file already exists")
        } else {
            e(x)
        }
    })?;
    if let Err(x) = f.write_all(&bytes).and_then(|_| f.sync_all()) {
        drop(f);
        let _ = v.unlink(&rel);
        return Err(e(x));
    }
    Ok(rel.to_string_lossy().into_owned())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn fresh(tag: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("canvasfs-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (root, out) = (base.join("vault"), base.join("outside"));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&out).unwrap();
        (root, out)
    }

    #[test]
    fn canvasfs_read_is_byte_exact_and_write_replaces() {
        let (root, _) = fresh("rw");
        // odd spacing, key order, trailing newline: read hands it back verbatim
        let raw = "{\"nodes\":[ {\"id\":\"a\",\"type\":\"text\",\"text\":\"h\u{e9}\",\"x\":0,\"y\":0,\"width\":1,\"height\":1,\"zz\":7}],\n \"edges\":[]}\n";
        fs::write(root.join("A.canvas"), raw).unwrap();
        assert_eq!(op(&root, "read", "A.canvas", None).unwrap(), Value::String(raw.into()));
        op(&root, "write", "A.canvas", Some("{}")).unwrap();
        assert_eq!(fs::read_to_string(root.join("A.canvas")).unwrap(), "{}");
    }

    #[test]
    fn canvasfs_create_new_only_and_write_existing_only() {
        let (root, _) = fresh("create");
        fs::create_dir_all(root.join("sub")).unwrap();
        op(&root, "create", "sub/N.canvas", Some("{\"nodes\":[],\"edges\":[]}")).unwrap();
        // the empty document is created as a 0-byte file (CVR-A-004)
        assert_eq!(fs::read_to_string(root.join("sub/N.canvas")).unwrap(), "");
        op(&root, "create", "sub/M.canvas", Some("{\"nodes\":[{\"id\":\"a\"}],\"edges\":[]}")).unwrap();
        assert_eq!(fs::read_to_string(root.join("sub/M.canvas")).unwrap(), "{\"nodes\":[{\"id\":\"a\"}],\"edges\":[]}");
        let e = op(&root, "create", "sub/N.canvas", Some("{}")).unwrap_err();
        assert!(e.starts_with("EEXIST"), "{e}");
        assert_eq!(fs::read_to_string(root.join("sub/N.canvas")).unwrap(), "");
        assert!(op(&root, "write", "Ghost.canvas", Some("{}")).is_err());
        assert!(!root.join("Ghost.canvas").exists());
        // the parent is not created
        assert!(op(&root, "create", "nodir/X.canvas", Some("{}")).is_err());
        assert!(!root.join("nodir").exists());
    }

    #[test]
    fn canvasfs_refuses_non_canvas_hidden_and_escaping_paths() {
        let (root, out) = fresh("refuse");
        fs::write(root.join("Note.md"), "keep").unwrap();
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        fs::write(out.join("V.canvas"), "outside").unwrap();
        for p in ["Note.md", "Note.md.bak", ".canvas", ".obsidian/x.canvas", ".hidden.canvas", "../outside/V.canvas",
                  "/etc/x.canvas", "a/../B.canvas", "a\\b.canvas", "%2e%2e/x.canvas", "x.canvas\0", ""] {
            assert!(op(&root, "read", p, None).is_err(), "read {p:?}");
            assert!(op(&root, "write", p, Some("{}")).is_err(), "write {p:?}");
            assert!(op(&root, "create", p, Some("{}")).is_err(), "create {p:?}");
        }
        assert_eq!(fs::read_to_string(root.join("Note.md")).unwrap(), "keep");
        assert!(!root.join(".obsidian/x.canvas").exists());
        assert_eq!(fs::read_to_string(out.join("V.canvas")).unwrap(), "outside");
        assert!(op(&root, "unlink", "A.canvas", None).unwrap_err().starts_with("ENOSYS"));
    }

    #[test]
    fn canvasfs_symlinks_are_never_followed() {
        let (root, out) = fresh("link");
        fs::write(out.join("T.canvas"), "outside").unwrap();
        symlink(out.join("T.canvas"), root.join("L.canvas")).unwrap();
        symlink(&out, root.join("d")).unwrap();
        assert!(op(&root, "read", "L.canvas", None).is_err());
        assert!(op(&root, "write", "L.canvas", Some("pwn")).is_err());
        assert!(op(&root, "read", "d/T.canvas", None).is_err());
        assert!(op(&root, "write", "d/T.canvas", Some("pwn")).is_err());
        assert!(op(&root, "create", "d/U.canvas", Some("pwn")).is_err());
        assert_eq!(fs::read_to_string(out.join("T.canvas")).unwrap(), "outside");
        assert!(!out.join("U.canvas").exists());
        assert_eq!(list(&root), Vec::<String>::new());
    }

    #[test]
    fn canvasfs_list_finds_canvas_files_only() {
        let (root, _) = fresh("list");
        fs::create_dir_all(root.join("b/c")).unwrap();
        fs::create_dir_all(root.join(".trash")).unwrap();
        for p in ["Z.canvas", "b/c/Y.canvas", "b/n.md", ".trash/old.canvas", "b/.h.canvas", "b/x.canvas.md"] {
            fs::write(root.join(p), "{}").unwrap();
        }
        fs::create_dir_all(root.join("dir.canvas")).unwrap();
        assert_eq!(list(&root), vec!["Z.canvas".to_string(), "b/c/Y.canvas".to_string()]);
        assert_eq!(op(&root, "list", "", None).unwrap(), json!(["Z.canvas", "b/c/Y.canvas"]));
    }

    #[test]
    fn canvasfs_media_lists_audio_and_video_only() {
        let (root, _) = fresh("media");
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::create_dir_all(root.join(".trash")).unwrap();
        for p in ["s.mp3", "a/b/C.MP4", "a/x.png", "a/n.md", ".trash/t.mp3", "a/.h.mp4", ".mp3", "a/v.mp4.md", "B.canvas"] {
            fs::write(root.join(p), "x").unwrap();
        }
        fs::create_dir_all(root.join("dir.webm")).unwrap();
        assert_eq!(op(&root, "media", "", None).unwrap(), json!(["a/b/C.MP4", "s.mp3"]));
        assert_eq!(list(&root), vec!["B.canvas".to_string()]);
    }

    #[test]
    fn canvasfs_rejects_non_utf8_and_oversized() {
        let (root, _) = fresh("big");
        fs::write(root.join("B.canvas"), [0xff, 0xfe, 0x00]).unwrap();
        assert!(op(&root, "read", "B.canvas", None).unwrap_err().starts_with("EILSEQ"));
        let big = "x".repeat(CANVAS_MAX as usize + 1);
        assert!(op(&root, "write", "B.canvas", Some(&big)).unwrap_err().starts_with("EFBIG"));
        assert_eq!(fs::read(root.join("B.canvas")).unwrap(), vec![0xff, 0xfe, 0x00]);
        assert!(op(&root, "write", "B.canvas", None).is_err());
    }
    #[test]
    fn canvasfs_rename_keeps_bytes_and_never_overwrites() {
        let (root, out) = fresh("rename");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("A.canvas"), "{\"nodes\":[]}\n").unwrap();
        fs::write(root.join("Taken.canvas"), "keep").unwrap();
        op(&root, "rename", "A.canvas", Some("sub/B.canvas")).unwrap();
        assert!(!root.join("A.canvas").exists());
        assert_eq!(fs::read_to_string(root.join("sub/B.canvas")).unwrap(), "{\"nodes\":[]}\n");
        assert!(op(&root, "rename", "sub/B.canvas", Some("Taken.canvas")).unwrap_err().starts_with("EEXIST"));
        assert_eq!(fs::read_to_string(root.join("Taken.canvas")).unwrap(), "keep");
        for to in ["B.md", ".trash/B.canvas", "../outside/B.canvas", "nodir/B.canvas"] {
            assert!(op(&root, "rename", "sub/B.canvas", Some(to)).is_err(), "rename to {to:?}");
        }
        assert!(op(&root, "rename", "sub/B.canvas", None).is_err());
        assert!(root.join("sub/B.canvas").exists());
        assert!(fs::read_dir(&out).unwrap().next().is_none());
    }

    #[test]
    fn canvasfs_relink_rewrites_file_cards_only_byte_exact() {
        let (root, out) = fresh("relink");
        // odd spacing, a subpath, an escaped spelling of the same path, a text card and an x-key naming it too
        let src = "{\"nodes\":[ {\"id\":\"f1\" , \"type\":\"file\",\"file\":\"Note.md\",\"subpath\":\"#H\",\"x\":1.50},\n\
                   {\"file\":\"No\\u0074e.md\",\"type\":\"file\",\"id\":\"f2\"},{\"id\":\"f3\",\"type\":\"file\",\"file\":\"Other.md\"},\n\
                   {\"id\":\"t\",\"type\":\"text\",\"text\":\"Note.md\",\"file\":\"Note.md\"},{\"id\":\"g\",\"type\":\"group\",\"x-o\":{\"type\":\"file\",\"file\":\"Note.md\"}}],\n\
                   \"edges\":[],\"x-root\":{\"nodes\":[{\"type\":\"file\",\"file\":\"Note.md\"}]}}";
        let want = src.replacen("\"file\":\"Note.md\",\"subpath\"", "\"file\":\"d/Re named.md\",\"subpath\"", 1).replacen("\"No\\u0074e.md\"", "\"d/Re named.md\"", 1);
        fs::write(root.join("S.canvas"), src).unwrap();
        fs::write(root.join("Bad.canvas"), "{\"nodes\":[{\"type\":\"file\",\"file\":\"Note.md\"}").unwrap();
        fs::write(root.join("U.canvas"), "{\"nodes\":[{\"id\":\"a\",\"type\":\"file\",\"file\":\"Other.md\"}]}").unwrap();
        fs::write(out.join("T.canvas"), "{\"nodes\":[{\"id\":\"a\",\"type\":\"file\",\"file\":\"Note.md\"}]}").unwrap();
        symlink(out.join("T.canvas"), root.join("L.canvas")).unwrap();
        let d = r#"{"from":"Note.md","to":"d/Re named.md"}"#;
        assert_eq!(op(&root, "relink", "", Some(d)).unwrap(), json!(["S.canvas"]));
        assert_eq!(fs::read_to_string(root.join("S.canvas")).unwrap(), want);
        assert_eq!(op(&root, "relink", "", Some(d)).unwrap(), json!([]));   // nothing left to point
        assert_eq!(fs::read_to_string(root.join("Bad.canvas")).unwrap(), "{\"nodes\":[{\"type\":\"file\",\"file\":\"Note.md\"}");
        assert_eq!(fs::read_to_string(root.join("U.canvas")).unwrap(), "{\"nodes\":[{\"id\":\"a\",\"type\":\"file\",\"file\":\"Other.md\"}]}");
        assert!(fs::read_to_string(out.join("T.canvas")).unwrap().contains("\"Note.md\""));
        for bad in [None, Some("{}"), Some("[]"), Some(r#"{"from":"/etc/x","to":"a.md"}"#), Some(r#"{"from":"a.md","to":"../a.md"}"#)] {
            assert!(op(&root, "relink", "", bad).is_err(), "relink {bad:?}");
        }
        let v: Value = serde_json::from_str(&fs::read_to_string(root.join("S.canvas")).unwrap()).unwrap();
        assert_eq!(v["nodes"][0]["file"], json!("d/Re named.md"));
        assert_eq!(v["nodes"][3]["file"], json!("Note.md"));
    }

    #[test]
    fn canvasfs_trash_moves_into_dot_trash() {
        let (root, out) = fresh("trash");
        fs::write(root.join("D.canvas"), "one").unwrap();
        assert_eq!(op(&root, "trash", "D.canvas", None).unwrap(), json!(".trash/D.canvas"));
        fs::write(root.join("D.canvas"), "two").unwrap();
        assert_eq!(op(&root, "trash", "D.canvas", None).unwrap(), json!(".trash/D.1.canvas"));
        assert!(!root.join("D.canvas").exists());
        assert_eq!(fs::read_to_string(root.join(".trash/D.canvas")).unwrap(), "one");
        assert_eq!(fs::read_to_string(root.join(".trash/D.1.canvas")).unwrap(), "two");
        fs::write(out.join("T.canvas"), "outside").unwrap();
        symlink(out.join("T.canvas"), root.join("L.canvas")).unwrap();
        assert!(op(&root, "trash", "L.canvas", None).is_err());
        assert!(op(&root, "trash", "Gone.canvas", None).is_err());
        assert_eq!(fs::read_to_string(out.join("T.canvas")).unwrap(), "outside");
    }

    #[test]
    fn canvasfs_links_lists_file_cards() {
        let (root, _) = fresh("links");
        fs::write(root.join("S.canvas"), r#"{"nodes":[{"id":"f","type":"file","file":"Note.md"},{"id":"g","type":"file","file":"Note.md"},{"id":"t","type":"text","text":"Other.md"}]}"#).unwrap();
        fs::write(root.join("Bad.canvas"), "not json").unwrap();
        fs::write(root.join("E.canvas"), "{\"nodes\":[]}").unwrap();
        assert_eq!(op(&root, "links", "", None).unwrap(), json!({"S.canvas": ["Note.md"]}));
    }

    const PNG1: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 1, 2, 0xff];

    #[test]
    fn canvasfs_attach_writes_new_bytes_exactly() {
        let (root, _) = fresh("attach");
        fs::create_dir_all(root.join("sub")).unwrap();
        let b = vaultfs::b64_encode(PNG1);
        assert_eq!(attach(&root, "Board.png", &b).unwrap(), "Board.png");
        assert_eq!(fs::read(root.join("Board.png")).unwrap(), PNG1);
        for p in ["sub/a.JPG", "sub/b.jpeg", "c.gif", "d.webp", "Board 1.png"] {
            assert_eq!(attach(&root, p, &b).unwrap(), p);
            assert_eq!(fs::read(root.join(p)).unwrap(), PNG1, "{p}");
        }
        // padding is optional
        assert!(attach(&root, "np.png", b.trim_end_matches('=')).is_ok());
        assert_eq!(fs::read(root.join("np.png")).unwrap(), PNG1);
    }

    #[test]
    fn canvasfs_attach_never_overwrites() {
        let (root, _) = fresh("attach-ex");
        fs::write(root.join("Board.png"), "keep").unwrap();
        fs::create_dir_all(root.join("dir.png")).unwrap();
        let b = vaultfs::b64_encode(PNG1);
        assert!(attach(&root, "Board.png", &b).unwrap_err().starts_with("EEXIST"));
        assert_eq!(fs::read_to_string(root.join("Board.png")).unwrap(), "keep");
        assert!(attach(&root, "dir.png", &b).is_err());
        assert!(root.join("dir.png").is_dir());
        // the parent is not created
        assert!(attach(&root, "nodir/x.png", &b).is_err());
        assert!(!root.join("nodir").exists());
    }

    #[test]
    fn canvasfs_attach_refuses_traversal_absolute_hidden_and_symlinks() {
        let (root, out) = fresh("attach-esc");
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        fs::write(out.join("T.png"), "outside").unwrap();
        symlink(out.join("T.png"), root.join("L.png")).unwrap();
        symlink(out.join("Dangling.png"), root.join("D.png")).unwrap();
        symlink(&out, root.join("d")).unwrap();
        let b = vaultfs::b64_encode(PNG1);
        for p in ["../outside/U.png", "a/../U.png", "/etc/U.png", out.join("U.png").to_str().unwrap(), "%2e%2e/U.png",
                  "a\\U.png", "U.png\0", "", ".obsidian/U.png", ".U.png", "d/U.png", "d/T.png", "L.png", "D.png", "C:/U.png"] {
            assert!(attach(&root, p, &b).is_err(), "attach {p:?}");
        }
        assert_eq!(fs::read_to_string(out.join("T.png")).unwrap(), "outside");
        assert!(!out.join("U.png").exists());
        assert!(!out.join("Dangling.png").exists());
        assert!(!root.join(".obsidian/U.png").exists());
        assert!(attach(&root, "../outside/U.png", &b).unwrap_err().starts_with("EPERM"));
        assert!(attach(&root, ".obsidian/U.png", &b).unwrap_err().starts_with("EPERM"));
    }

    #[test]
    fn canvasfs_attach_refuses_bad_ext_bad_base64_and_oversize() {
        let (root, _) = fresh("attach-bad");
        let b = vaultfs::b64_encode(PNG1);
        for p in ["Note.md", "x.canvas", "x.svg", "x.png.md", "png", "x.", "x.pngx", "noext"] {
            assert!(attach(&root, p, &b).unwrap_err().starts_with("EINVAL"), "ext {p:?}");
            assert!(!root.join(p).exists(), "{p}");
        }
        for d in ["", "====", "not base64!", "QUJD\nREVG", "data:image/png;base64,QUJD"] {
            assert!(attach(&root, "b.png", d).unwrap_err().starts_with("EINVAL"), "data {d:?}");
        }
        assert!(!root.join("b.png").exists());
        let big = "A".repeat((ATTACH_MAX.div_ceil(3) * 4) as usize + 4);
        assert!(attach(&root, "big.png", &big).unwrap_err().starts_with("EFBIG"));
        // exactly over the decoded cap (4 chars -> 3 bytes; cap+3 bytes)
        let over = "A".repeat(((ATTACH_MAX + 3) / 3 * 4) as usize);
        assert!(over.len() as u64 <= ATTACH_MAX.div_ceil(3) * 4 + 4);
        assert!(attach(&root, "big.png", &over).unwrap_err().starts_with("EFBIG"));
        assert!(!root.join("big.png").exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    }
}
