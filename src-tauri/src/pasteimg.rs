// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* gh-paste (GitHub #19 "not possible to paste image / screenshot").

   Stock 1.13.7, measured on the box with the pinned AppImage (harness
   docs/gh-paste/README.md): Ctrl+V of a PNG on the clipboard writes the bytes,
   unchanged, as `Pasted image <YYYYMMDDHHmmss>.png` (local time) in the
   attachment folder (`attachmentFolderPath`: "" or "/" = vault root, "att/sub"
   = that vault folder, "./" = the note's folder, "./img" = a folder under the
   note's folder) and inserts `![[Pasted image ….png]]` at the cursor, nothing
   around it (`![](Pasted%20image%20….png)` when useMarkdownLinks is on). Two
   pastes in the same second: the second is `Pasted image <ts> 1.png`. A
   clipboard that offers only image/jpeg pastes NOTHING on stock (Chromium reads
   image/png from the X clipboard only), so PASTE_TYPES is png alone.

   This is the second user-driven write of arbitrary bytes into the vault (the
   first is attach_drop), and it inherits the same rules:
     TYPE  — the MIME must be one we paste AND the bytes must carry its magic;
             a text/html blob labelled image/png is refused at the door.
     CAP   — MAX_PASTE_BYTES, checked on the base64 length BEFORE decoding and
             on the decoded bytes.
     DEST  — the folder is a safe_rel path (no `..`, no hidden component, no
             absolute path), its deepest existing ancestor must canonicalize
             inside the vault BEFORE mkdir, and vaultfs walks/creates every
             component with O_NOFOLLOW, so a symlinked attachment folder is
             refused, never written through.
     NO OVER — the file is created with O_CREAT|O_EXCL|O_NOFOLLOW (create_new):
             a taken name — or a symlink planted at it — is skipped to the next
             ` <n>` candidate, never written into or over.            */
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::{cand_names, note_path_in, note_rel, safe_rel, vaultfs};

/// the paste cap: the same 32 MB the image server and drops use
pub const MAX_PASTE_BYTES: u64 = crate::MAX_IMG_BYTES;

/// clipboard MIME -> (extension, magic). Stock pastes PNG only (module doc).
const PASTE_TYPES: [(&str, &str, &[u8]); 1] = [("image/png", "png", b"\x89PNG\r\n\x1a\n")];

#[derive(Debug, PartialEq, serde::Serialize)]
pub struct Pasted {
    /// the embed to insert at the cursor
    pub text: String,
    /// the vault-relative path that was written
    pub path: String,
}

#[derive(Debug, PartialEq)]
pub enum PasteErr {
    NoNote,
    NotImage(String),
    BadMagic,
    Empty,
    TooBig(u64),
    BadStamp,
    BadAttachDir,
    NoFreeName,
    Io(String),
}

impl PasteErr {
    pub fn say(&self) -> String {
        match self {
            PasteErr::NoNote => "open a note first".into(),
            PasteErr::NotImage(m) => format!("{m}: not an image type Opensidian pastes"),
            PasteErr::BadMagic => "the clipboard image is not a PNG (its content does not match its type)".into(),
            PasteErr::Empty => "the clipboard image is empty".into(),
            PasteErr::TooBig(n) => format!("the clipboard image ({} MB) is over the {} MB limit", n / 1048576, MAX_PASTE_BYTES / 1048576),
            PasteErr::BadStamp => "invalid clock".into(),
            PasteErr::BadAttachDir => "the attachment folder is not inside the vault".into(),
            PasteErr::NoFreeName => "no free file name left in the attachment folder".into(),
            PasteErr::Io(e) => format!("the pasted image could not be saved ({e})"),
        }
    }
}

/// the local wall clock as `YYYYMMDDHHmmss` (moment's format in stock's name).
/// The Tm crosses the IPC, so every field is range-checked: the stamp becomes
/// part of a file name and must be exactly 14 digits.
pub fn stamp(t: &crate::datefmt::Tm) -> Result<String, PasteErr> {
    let ok = (0..=9999).contains(&t.y) && (1..=12).contains(&t.mo) && (1..=31).contains(&t.d) && t.h < 24 && t.mi < 60 && t.s < 60;
    if !ok {
        return Err(PasteErr::BadStamp);
    }
    Ok(format!("{:04}{:02}{:02}{:02}{:02}{:02}", t.y, t.mo, t.d, t.h, t.mi, t.s))
}

/// attachmentFolderPath -> the vault-relative folder ("" = the vault root) for
/// a note at `note_rel` (`sub/N.md`). None = a value we refuse (traversal,
/// hidden, absolute beyond the stock "/").
pub fn rel_dir(cfg: &str, note_rel: &Path) -> Option<PathBuf> {
    let w = cfg.trim();
    if w.is_empty() || w == "/" || w == "." {
        return Some(PathBuf::new());
    }
    let (base, rest) = match w.strip_prefix("./") {
        Some(r) => (note_rel.parent().map(Path::to_path_buf).unwrap_or_default(), r.trim_end_matches('/')),
        None => (PathBuf::new(), w.trim_matches('/')),
    };
    if rest.is_empty() {
        return Some(base);
    }
    let r = safe_rel(rest)?;
    Some(base.join(r))
}

/// encodeURI for the markdown-link form (stock: `![](Pasted%20image%20….png)`),
/// minus `(` `)` which would end the link target
fn enc_uri(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*';/?:@&=+$,#".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// THE function: write `bytes` (clipboard type `mime`) into the attachment
/// folder of `note` under stock's name and return the embed. `md_links` is
/// useMarkdownLinks; `taken(base)` says whether another vault file already has
/// that basename (then the link carries the full path, stock's "shortest").
pub fn paste_into(
    root: &Path,
    note: &str,
    mime: &str,
    bytes: &[u8],
    stamp: &str,
    attach_cfg: &str,
    md_links: bool,
    taken: &dyn Fn(&str) -> bool,
) -> Result<Pasted, PasteErr> {
    let Some((_, ext, magic)) = PASTE_TYPES.iter().find(|(m, _, _)| *m == mime) else {
        return Err(PasteErr::NotImage(mime.chars().filter(|c| !c.is_control()).take(64).collect()));
    };
    if bytes.is_empty() {
        return Err(PasteErr::Empty);
    }
    if bytes.len() as u64 > MAX_PASTE_BYTES {
        return Err(PasteErr::TooBig(bytes.len() as u64));
    }
    if !bytes.starts_with(magic) {
        return Err(PasteErr::BadMagic);
    }
    if stamp.len() != 14 || !stamp.bytes().all(|b| b.is_ascii_digit()) {
        return Err(PasteErr::BadStamp);
    }
    let nrel = note_rel(note).ok_or(PasteErr::NoNote)?;
    // the note must EXIST (a regular file; note_path_in already refused a symlinked leaf):
    // "./" forms resolve against its folder, and a paste into no note is refused
    if note.is_empty() || !note_path_in(root, note, false).is_some_and(|p| p.is_file()) {
        return Err(PasteErr::NoNote);
    }
    let dir = rel_dir(attach_cfg, &nrel).ok_or(PasteErr::BadAttachDir)?;
    let croot = root.canonicalize().map_err(|_| PasteErr::BadAttachDir)?;
    // DEST: the deepest EXISTING ancestor is inside the vault before any mkdir
    let full = root.join(&dir);
    let mut a = full.as_path();
    while !a.exists() {
        a = a.parent().ok_or(PasteErr::BadAttachDir)?;
    }
    if !a.canonicalize().map_err(|_| PasteErr::BadAttachDir)?.starts_with(&croot) {
        return Err(PasteErr::BadAttachDir);
    }
    let vfs = vaultfs::Vault::open(root).map_err(|e| PasteErr::Io(e.to_string()))?;
    if !dir.as_os_str().is_empty() {
        // O_NOFOLLOW per component: a symlinked folder is ELOOP, not followed
        vfs.mkdir_p(&dir).map_err(|_| PasteErr::BadAttachDir)?;
    }
    for name in cand_names(&format!("Pasted image {stamp}.{ext}")) {
        let rel = dir.join(&name);
        let mut f = match vfs.create_new(&rel, false) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            #[cfg(unix)]
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => continue, // a symlink AT the name: skip it
            Err(e) => return Err(PasteErr::Io(e.to_string())),
        };
        if let Err(e) = f.write_all(bytes).and_then(|_| f.sync_all()) {
            drop(f);
            let _ = vfs.unlink(&rel); // reason: best effort — a half-written file must not stay; the error below is what matters
            return Err(PasteErr::Io(e.to_string()));
        }
        let path = rel.to_string_lossy().to_string();
        let target = if taken(&name) { path.clone() } else { name.clone() };
        let text = if md_links { format!("![]({})", enc_uri(&target)) } else { format!("![[{target}]]") };
        return Ok(Pasted { text, path });
    }
    Err(PasteErr::NoFreeName)
}

/// app.json -> (attachmentFolderPath, useMarkdownLinks)
pub fn app_cfg(root: &Path) -> (String, bool) {
    let v = std::fs::read_to_string(root.join(".obsidian/app.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_default();
    (
        v.get("attachmentFolderPath").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        v.get("useMarkdownLinks").and_then(|x| x.as_bool()).unwrap_or(false),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDRtest-bytes";

    fn vault(tag: &str) -> PathBuf {
        let r = std::env::temp_dir().join(format!("opensidian-paste-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&r); // reason: a leftover from an earlier run, absent is fine
        fs::create_dir_all(r.join("dir")).unwrap();
        fs::write(r.join("N.md"), "# N\n").unwrap();
        fs::write(r.join("dir/M.md"), "# M\n").unwrap();
        r
    }
    fn none(_: &str) -> bool {
        false
    }
    fn paste(r: &Path, note: &str, cfg: &str) -> Result<Pasted, PasteErr> {
        paste_into(r, note, "image/png", PNG, "20261026130118", cfg, false, &none)
    }

    #[test]
    fn ghpaste_png_stock_name_bytes_and_embed() {
        let r = vault("png");
        let p = paste(&r, "N", "").unwrap();
        assert_eq!(p.text, "![[Pasted image 20261026130118.png]]");
        assert_eq!(p.path, "Pasted image 20261026130118.png");
        assert_eq!(fs::read(r.join(&p.path)).unwrap(), PNG);
        // same second: stock's ` 1` suffix, the first file untouched
        let q = paste(&r, "N", "").unwrap();
        assert_eq!(q.text, "![[Pasted image 20261026130118 1.png]]");
        assert_eq!(fs::read(r.join("Pasted image 20261026130118.png")).unwrap(), PNG);
        let m = paste_into(&r, "N", "image/png", PNG, "20261026130118", "", true, &none).unwrap();
        assert_eq!(m.text, "![](Pasted%20image%2020261026130118%202.png)");
        let _ = fs::remove_dir_all(&r); // reason: test cleanup
    }

    #[test]
    fn ghpaste_attachment_folder_forms() {
        let n = Path::new("dir/M.md");
        assert_eq!(rel_dir("", n), Some(PathBuf::new()));
        assert_eq!(rel_dir("/", n), Some(PathBuf::new()));
        assert_eq!(rel_dir("att/sub", n), Some(PathBuf::from("att/sub")));
        assert_eq!(rel_dir("./", n), Some(PathBuf::from("dir")));
        assert_eq!(rel_dir("./img", n), Some(PathBuf::from("dir/img")));
        let r = vault("dirs");
        assert_eq!(paste(&r, "N", "att/sub").unwrap().path, "att/sub/Pasted image 20261026130118.png");
        assert_eq!(paste(&r, "dir/M", "./img").unwrap().path, "dir/img/Pasted image 20261026130118.png");
        // shortest link: a basename taken elsewhere -> the full path
        let t = paste_into(&r, "N", "image/png", PNG, "20261026130119", "att", false, &|_| true).unwrap();
        assert_eq!(t.text, "![[att/Pasted image 20261026130119.png]]");
        let _ = fs::remove_dir_all(&r); // reason: test cleanup
    }

    #[test]
    fn ghpaste_traversal_refused() {
        assert_eq!(rel_dir("../out", Path::new("N.md")), None);
        assert_eq!(rel_dir("./../../x", Path::new("dir/M.md")), None);
        assert_eq!(rel_dir("a/../../b", Path::new("N.md")), None);
        assert_eq!(rel_dir(".obsidian", Path::new("N.md")), None);
        let r = vault("trav");
        assert_eq!(paste(&r, "N", "../escape"), Err(PasteErr::BadAttachDir));
        assert_eq!(paste(&r, "../N", ""), Err(PasteErr::NoNote));
        assert_eq!(paste(&r, "Nope", ""), Err(PasteErr::NoNote));
        assert!(!r.parent().unwrap().join("escape").exists());
        // the stamp is part of the name: anything but 14 digits is refused
        assert_eq!(paste_into(&r, "N", "image/png", PNG, "../../../x0000", "", false, &none), Err(PasteErr::BadStamp));
        let _ = fs::remove_dir_all(&r); // reason: test cleanup
    }

    #[cfg(unix)]
    #[test]
    fn ghpaste_symlink_refused() {
        let r = vault("sym");
        let out = std::env::temp_dir().join(format!("opensidian-paste-out-{}", std::process::id()));
        let _ = fs::remove_dir_all(&out); // reason: leftover from an earlier run
        fs::create_dir_all(&out).unwrap();
        // a symlinked attachment folder pointing out of the vault
        std::os::unix::fs::symlink(&out, r.join("att")).unwrap();
        assert_eq!(paste(&r, "N", "att"), Err(PasteErr::BadAttachDir));
        assert_eq!(paste(&r, "N", "att/deeper"), Err(PasteErr::BadAttachDir));
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0, "nothing written outside the vault");
        // a symlink planted AT the stock name: skipped, never written through
        let tgt = out.join("victim.png");
        std::os::unix::fs::symlink(&tgt, r.join("Pasted image 20261026130118.png")).unwrap();
        let p = paste(&r, "N", "").unwrap();
        assert_eq!(p.path, "Pasted image 20261026130118 1.png");
        assert!(!tgt.exists(), "the planted symlink was followed");
        let _ = fs::remove_dir_all(&r); // reason: test cleanup
        let _ = fs::remove_dir_all(&out); // reason: test cleanup
    }

    #[test]
    fn ghpaste_non_image_refused() {
        let r = vault("mime");
        for m in ["text/plain", "text/html", "image/svg+xml", "application/octet-stream", "image/jpeg", ""] {
            assert!(matches!(paste_into(&r, "N", m, PNG, "20261026130118", "", false, &none), Err(PasteErr::NotImage(_))), "{m}");
        }
        // labelled png, but the bytes are not: refused by magic
        assert_eq!(paste_into(&r, "N", "image/png", b"<svg onload=x>", "20261026130118", "", false, &none), Err(PasteErr::BadMagic));
        assert_eq!(paste_into(&r, "N", "image/png", b"", "20261026130118", "", false, &none), Err(PasteErr::Empty));
        let left: Vec<_> = fs::read_dir(&r).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 2, "a refused paste wrote nothing: {left:?}");
        let _ = fs::remove_dir_all(&r); // reason: test cleanup
    }

    #[test]
    fn ghpaste_cap() {
        let r = vault("cap");
        let mut big = PNG.to_vec();
        big.resize(MAX_PASTE_BYTES as usize + 1, 0);
        assert_eq!(paste_into(&r, "N", "image/png", &big, "20261026130118", "", false, &none), Err(PasteErr::TooBig(MAX_PASTE_BYTES + 1)));
        assert!(!r.join("Pasted image 20261026130118.png").exists());
        big.truncate(MAX_PASTE_BYTES as usize);
        assert!(paste_into(&r, "N", "image/png", &big, "20261026130118", "", false, &none).is_ok(), "exactly the cap is allowed");
        let _ = fs::remove_dir_all(&r); // reason: test cleanup
    }

    #[test]
    fn ghpaste_stamp_checked() {
        let t = |y, mo, d, h, mi, s| crate::datefmt::Tm { y, mo, d, h, mi, s, ms: 0, off: 0, epoch_ms: 0 };
        assert_eq!(stamp(&t(2026, 10, 26, 13, 1, 18)).unwrap(), "20261026130118");
        assert_eq!(stamp(&t(2026, 1, 2, 3, 4, 5)).unwrap(), "20260102030405");
        assert_eq!(stamp(&t(-1, 1, 1, 0, 0, 0)), Err(PasteErr::BadStamp));
        assert_eq!(stamp(&t(10000, 1, 1, 0, 0, 0)), Err(PasteErr::BadStamp));
        assert_eq!(stamp(&t(2026, 13, 1, 0, 0, 0)), Err(PasteErr::BadStamp));
        assert_eq!(stamp(&t(2026, 1, 1, 24, 0, 0)), Err(PasteErr::BadStamp));
    }
}
