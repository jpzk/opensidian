#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// rustidian, an Obsidian-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
// This program comes with ABSOLUTELY NO WARRANTY. It is free software, and you
// are welcome to redistribute it under the terms of the GNU GPL version 3 or
// (at your option) any later version. See LICENSE, or <https://www.gnu.org/licenses/>.
/* lost-write: a dropped Result is how a failed save becomes a reported
   success. rustc warned about exactly that in link_mention for weeks and
   nobody read the build log — a warning nobody reads is not a safety net.
   DENY, so the next one is a build error. Where ignoring is genuinely right,
   write `let _ = ...` WITH a reason next to it. */
#![deny(unused_must_use)]
use pulldown_cmark::{html, Event, LinkType, Options, Parser, Tag, TagEnd};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use tauri::State;

mod index;
mod outline;
mod palette;
mod perf;
mod sandbox;
mod settings;
mod srcmode;
mod themefs;
mod watcher;
use index::{link_parts, links_in, resolve, tag_spans, Graph, Index};

/* perf-index: root + in-memory Index (src/index.rs). The index replaces the
   perf-lp NoteCache: names, contents, links and backlink edges live in RAM,
   built once per vault open and kept == disk by write_note/rename_note.
   search/graph/backlinks/render never touch the filesystem. */
struct Vault {
    root: Mutex<Option<PathBuf>>,
    index: Mutex<Index>,
}

fn cur_vault(v: &State<Vault>) -> Option<PathBuf> {
    v.root.lock().unwrap().clone()
}

/// sorted note list from the index (empty when no vault open)
fn cur_notes(v: &State<Vault>) -> Vec<String> {
    v.index.lock().unwrap().names().to_vec()
}

/// open a vault: swap root + rebuild the index (one walk, one read per note)
fn open_vault(v: &State<Vault>, p: &Path) {
    let ix = span_timed!("index_build", Index::build(p), serde_json::json!({"notes": 0}));
    *v.index.lock().unwrap() = ix;
    *v.root.lock().unwrap() = Some(p.to_path_buf());
}
/// component-wise traversal check: only plain, non-hidden components allowed
fn safe_rel(name: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in Path::new(name).components() {
        match c {
            Component::Normal(s) if !s.to_string_lossy().starts_with('.') => out.push(s),
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// S2 vault confinement: canonical parent must live under the canonical root
/// and the leaf must not be a symlink. `create` makes missing parents (what
/// write/rename need) — but only after the deepest EXISTING ancestor proved
/// to be inside the vault, so mkdir never walks through a symlinked dir.
/// Returns the canonical path; keys stay the relative names, so the
/// index==disk invariant is untouched.
fn note_path_in(root: &Path, name: &str, create: bool) -> Option<PathBuf> {
    let rel = safe_rel(name)?;
    let croot = root.canonicalize().ok()?;
    let dir = root.join(rel.parent().unwrap_or(Path::new("")));
    if create {
        let mut a = dir.as_path();
        while !a.exists() {
            a = a.parent()?;
        }
        if !a.canonicalize().ok()?.starts_with(&croot) {
            return None;
        }
        fs::create_dir_all(&dir).ok()?;
    }
    let cdir = dir.canonicalize().ok()?;
    if !cdir.starts_with(&croot) {
        return None;
    }
    let p = cdir.join(format!("{}.md", rel.file_name()?.to_string_lossy()));
    if fs::symlink_metadata(&p).map(|m| m.is_symlink()).unwrap_or(false) {
        return None;
    }
    Some(p)
}

fn note_path(v: &State<Vault>, name: &str, create: bool) -> Option<PathBuf> {
    note_path_in(&cur_vault(v)?, name, create)
}

/* ---- R29 image embeds: the vault-contained byte server ------------------
   The attacker's input here is a .md file that arrived by sync/clone/share,
   so a rendered document decides which path we read. The containment rule is
   ours (docs/requirements.md R29.5) and lives in `img_path_in` below, the
   sibling of `note_path_in`: same shape, same two-sided canonicalize, so the
   two are read together. Landlock is NOT the guard — sandbox.rs:46 allowlists
   /etc and /usr for reading and sandbox.rs:94 reports it unsupported on this
   kernel; it is defence in depth that is currently absent.                */

/// R29.8: what v1 serves — stock's list minus `svg` (scriptable, and we have
/// no sanitizer), `bmp` and `avif` (scope only). ext -> Content-Type.
const IMG_TYPES: [(&str, &str); 5] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
];

/// the scheme our images are served on; the CSP names it exactly (R29 / S1)
const IMG_SCHEME: &str = "rustidian-img";

/// R29.10: the label the R29.4 banner carries in place of a REMOTE target.
/// Both engines paint the same sentence (ui/editor.js `imgEl` sets it as
/// data-miss, style.css paints it) — R29.7 says the two renderers agree, and
/// that includes this box. A constant, never the URL: see `image_html`.
const REMOTE_IMG_LABEL: &str = "remote image";

/// S5-style cap: an image bigger than this is not served (a synced vault must
/// not be able to make us allocate a multi-GB buffer inside the webview IPC).
const MAX_IMG_BYTES: u64 = 32 * 1024 * 1024;

/// percent-decode a markdown image target (R29.3: `my%20pic.png` -> `my pic.png`).
/// Strict: a `%` not followed by two hex digits is not a valid escape and the
/// whole target is refused rather than half-decoded. Decoding happens BEFORE
/// any containment check — decode-after-validate is exactly how `%2e%2e%2f`
/// becomes the escape.
fn pct_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hi = (*b.get(i + 1)? as char).to_digit(16)?;
            let lo = (*b.get(i + 2)? as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// R29.5 containment: a vault-relative image target -> the canonical file to
/// read, or None (serve nothing, say nothing — stock does not leak whether an
/// out-of-vault file exists, see R29.5).
///
/// Order matters, every step earns its place:
///  1. percent-decode first, so `%2e%2e%2f` is a plain `../` to the check below;
///  2. `safe_rel` refuses `..`, absolute paths, hidden components and (via
///     `Component::Normal`) anything with a NUL, and a scheme'd target like
///     `file:///etc/passwd` never reaches here (the renderer keeps schemes out);
///  3. extension allowlist — R29.8, lowercased;
///  4. canonicalize BOTH sides and compare, because a prefix test on the raw
///     string is not containment. Canonicalizing the ROOT too is what keeps a
///     genuinely symlinked vault (req S2) working;
///  5. the LEAF must not be a symlink — that is the symlink-inside-the-vault-
///     pointing-out case, which canonicalizing the dir alone would not catch.
fn img_path_in(root: &Path, target: &str) -> Option<PathBuf> {
    let dec = pct_decode(target)?;
    if dec.contains('\0') {
        return None;
    }
    let rel = safe_rel(&dec)?;
    let ext = rel.extension()?.to_str()?.to_ascii_lowercase();
    if !IMG_TYPES.iter().any(|(e, _)| *e == ext) {
        return None;
    }
    let croot = root.canonicalize().ok()?;
    let cdir = root.join(rel.parent().unwrap_or(Path::new(""))).canonicalize().ok()?;
    if !cdir.starts_with(&croot) {
        return None;
    }
    let p = cdir.join(rel.file_name()?);
    let m = fs::symlink_metadata(&p).ok()?;
    if m.is_symlink() || !m.is_file() || m.len() > MAX_IMG_BYTES {
        return None;
    }
    Some(p)
}

/// R29: bytes + Content-Type for a contained image, or None. The type comes
/// from OUR allowlist, never from the file, and the response is served
/// `nosniff` so a mislabeled blob cannot be re-interpreted as script.
fn serve_image(root: &Path, target: &str) -> Option<(&'static str, Vec<u8>)> {
    let p = img_path_in(root, target)?;
    let ext = p.extension()?.to_str()?.to_ascii_lowercase();
    let mime = IMG_TYPES.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)?;
    Some((mime, fs::read(&p).ok()?))
}

/// percent-ENCODE a vault-relative path for the `rustidian-img:` URL. Unreserved
/// RFC3986 characters and `/` survive; everything else (space, `#`, `?`, `%`,
/// non-ASCII) becomes %XX, so `pct_decode` on the serving side gets the exact
/// bytes back. Without this a note named `a#b.png` would lose everything after
/// the `#` to URL fragment parsing, and `a?b.png` to the query.
fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// R29.1/R29.2: the html for ONE image embed, from the index's image list.
///
/// Resolution goes through `index::resolve` — the SAME resolver `[[note]]`
/// links use — for BOTH syntaxes, so `![[pic.png]]` and `![](sub/pic.png)`
/// cannot disagree about what a name means (two resolvers for one syntax is
/// how this codebase grew its last bug). The `src` is the path the INDEX
/// holds, never the raw target: the index only ever contains real, non-hidden,
/// non-symlinked vault members, so a target that resolves to nothing gets no
/// URL at all — and the byte server re-checks containment independently.
///
/// Unresolved -> R29.4's banner, verbatim wording, target escaped. R29.5 makes
/// an out-of-vault target take this same branch, so an escape is
/// indistinguishable from a typo and leaks nothing about the filesystem.
///
/// R29.10 rides the same branch: a target carrying ANY scheme (`https://…`,
/// `http://…`, `file://…`) is never resolved and never minted — one rule, one
/// banner. The label is the literal `remote image`, NOT the target, so the URL
/// itself never reaches the DOM: an attribute a note controls is a beacon the
/// moment some later CSS/JS reads it.
fn image_html(imgs: &[String], target: &str, alt: &str) -> String {
    if url_scheme(target).is_some() {
        return format!(
            "<span class=\"imgmiss\">\u{201c}{REMOTE_IMG_LABEL}\u{201d} could not be found.</span>"
        );
    }
    let dec = pct_decode(target).unwrap_or_else(|| target.to_string());
    match index::resolve(imgs, &dec).map(|i| &imgs[i]) {
        Some(rel) => format!(
            "<img class=\"vimg\" src=\"{IMG_SCHEME}://localhost/{}\" alt=\"{}\">",
            pct_encode(rel),
            esc(alt)
        ),
        None => format!(
            "<span class=\"imgmiss\">\u{201c}{}\u{201d} could not be found.</span>",
            esc(target)
        ),
    }
}

/// is this wikilink target an image embed (`![[pic.png]]`) rather than a NOTE
/// embed (`![[Second Note]]`, out of scope — requirements.md:410)?
fn is_img_target(target: &str) -> bool {
    target
        .rsplit_once('.')
        .is_some_and(|(_, e)| index::IMG_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/* ---- R31 drop-to-attach (feedback #18) ---------------------------------
   A drop is the FIRST user-driven write of arbitrary bytes into the vault, so
   everything R29 built (read-side containment) is only half of what is needed
   here. The whole behaviour lives in `attach_drop` below: it takes the vault,
   the note being edited and the dropped paths, copies what it is allowed to
   copy and RETURNS the text to insert. The Tauri command and the
   WindowEvent::DragDrop handler are pass-throughs, because an OS drop cannot
   be synthesized in a test (xdotool has no XDND) — the tests and the smoke
   phase drive this function, and only the wry->handler transport is untested.

   Order of the checks is the design, each earns its place:
     S1 SOURCE  — a file manager can hand us anything: canonicalize, and refuse
                  a symlink / dir / fifo / socket / device outright.
     S3 NAME    — the basename is ATTACKER-CONTROLLED TEXT, not a name just
                  because the OS produced it: safe_rel + NUL + control chars.
     S4 TYPE    — the EXTENSION allowlist decides what is copied (IMG_TYPES),
                  never a content sniff; img_path_in decides what is served.
     S2 DEST    — the attachment dir is canonicalized against the canonical
                  vault root (the note_path_in shape), so vault/attachments ->
                  /home/user cannot turn a drop into a write outside the vault.
     S5 NO OVER — create_new(), i.e. O_EXCL: never exists()-then-write, and
                  never an overwrite. Data loss outranks stock parity (rule of
                  order): the collision gets a new name, the old file stays.
     CAP        — MAX_IMG_BYTES on the source metadata BEFORE the copy, and
                  again on the stream, so a file that grows mid-copy cannot
                  smuggle bytes past the cap.                              */

/// R31.1 the ONE name an OS drop reaches the UI under. Not `tauri://drag-drop`
/// (Tauri's own, which also fires for hover/leave and carries a pointer
/// position the editor has no use for): a name we own, carrying exactly the
/// paths, so the seam between "the OS dropped something" and "rustidian
/// attaches it" is one grep away for whoever reads this next.
const DROP_EVENT: &str = "drop-files";

/// R31.5 why a single dropped file was refused. Typed, because the UI must say
/// WHY: a drop that silently does nothing is indistinguishable from a bug, and
/// (R31.8) a REMOTE drag must not be described as a missing file.
#[derive(Debug, PartialEq, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Refused {
    Remote,          // a URL, not a file: R29.10 says no note may cause a fetch
    NotAFile,        // dir, fifo, socket, device, or gone
    Symlink,         // S1: a symlinked source is refused, never followed
    BadName,         // S3: traversal, NUL, control chars, hidden, non-UTF8
    BadExt(String),  // S4: not one of IMG_TYPES
    TooBig(u64),     // > MAX_IMG_BYTES
    Unreadable,      // R31.12: outside the sandbox's drop-source folders (EACCES)
    NoFreeName,      // 1000 collisions deep: refuse rather than loop
    Io(String),      // the copy itself failed (ENOSPC, EACCES from landlock, ...)
}

impl Refused {
    /// the sentence the UI shows. Must describe the CAUSE — the R29.10 refusal
    /// used to reuse R29.4's "could not be found", which is an error that
    /// misdescribes itself.
    fn say(&self, name: &str) -> String {
        match self {
            Refused::Remote => "remote images are disabled".into(),
            Refused::NotAFile => format!("{name}: not a regular file"),
            Refused::Symlink => format!("{name}: symlinks are not copied"),
            Refused::BadName => format!("{name}: unsafe file name"),
            Refused::BadExt(e) => format!("{name}: .{e} is not an image rustidian can show"),
            Refused::TooBig(n) => format!("{name}: {} MB is over the {} MB limit", n / 1048576, MAX_IMG_BYTES / 1048576),
            Refused::Unreadable => format!(
                "{name}: rustidian may only read dropped files from {} (sandbox)",
                sandbox::DROP_READ_DIRS.iter().map(|d| format!("~/{d}")).collect::<Vec<_>>().join(", ")
            ),
            Refused::NoFreeName => format!("{name}: no free file name left in the attachment folder"),
            Refused::Io(e) => format!("{name}: could not be copied ({e})"),
        }
    }
}

/// R31 whole-drop failure: nothing was copied and there is nothing to insert.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum DropErr {
    Empty,           // a drop carrying no path at all
    NoNote,          // no note open, or the target name is not a vault note
    BadAttachDir,    // S2: the attachment folder is not inside the vault
}

impl DropErr {
    fn say(&self) -> String {
        match self {
            DropErr::Empty => "nothing to attach".into(),
            DropErr::NoNote => "open a note first".into(),
            DropErr::BadAttachDir => "the attachment folder is not inside the vault".into(),
        }
    }
}

/// R31 what a drop did: the text to insert at the cursor, what landed in the
/// vault (so the index can learn the new images), and one sentence per refusal.
#[derive(Debug, Default, PartialEq, serde::Serialize)]
struct Attached {
    text: String,          // "" = insert nothing
    copied: Vec<String>,   // vault-relative paths, in drop order
    refused: Vec<String>,  // human sentences, already formatted
}

/// R31.3 the attachment folder: stock's default is the VAULT ROOT (recon
/// 2026-09-12: `.obsidian/app.json` is `{}` and the file lands at the root).
/// `attachmentFolderPath` is honoured when it is a plain vault-relative folder;
/// a note-relative `./sub` value is NOT supported in v1 and falls back to the
/// root with a visible notice, never silently. Returns (dir, notice).
fn attach_dir(root: &Path) -> Result<(PathBuf, Option<String>), DropErr> {
    let croot = root.canonicalize().map_err(|_| DropErr::BadAttachDir)?;
    let cfg = fs::read_to_string(root.join(".obsidian/app.json")).unwrap_or_default();
    let want = serde_json::from_str::<serde_json::Value>(&cfg)
        .ok()
        .and_then(|v| v.get("attachmentFolderPath")?.as_str().map(str::to_string))
        .unwrap_or_default();
    let want = want.trim();
    if want.is_empty() || want == "/" || want == "." {
        return Ok((croot, None));
    }
    let Some(rel) = safe_rel(want) else {
        return Ok((croot, Some(format!("attachmentFolderPath {want:?} is not supported — attaching to the vault root"))));
    };
    // S2, the note_path_in shape: the deepest EXISTING ancestor must already be
    // inside the vault before anything is created, so mkdir never walks through
    // a symlinked directory.
    let dir = root.join(&rel);
    let mut a = dir.as_path();
    while !a.exists() {
        a = a.parent().ok_or(DropErr::BadAttachDir)?;
    }
    if !a.canonicalize().map_err(|_| DropErr::BadAttachDir)?.starts_with(&croot) {
        return Err(DropErr::BadAttachDir);
    }
    fs::create_dir_all(&dir).map_err(|_| DropErr::BadAttachDir)?;
    let cdir = dir.canonicalize().map_err(|_| DropErr::BadAttachDir)?;
    if !cdir.starts_with(&croot) || !cdir.is_dir() {
        return Err(DropErr::BadAttachDir);
    }
    Ok((cdir, None))
}

/// S3: the dropped basename -> a name we are willing to create, or None.
/// `safe_rel` already refuses `..`, absolute paths, hidden components and NUL;
/// this adds "exactly one component" and "no control characters".
fn attach_name(p: &Path) -> Option<String> {
    let name = p.file_name()?.to_str()?;
    if name.contains('\0') || name.chars().any(|c| c.is_control()) {
        return None;
    }
    let rel = safe_rel(name)?;
    (rel.components().count() == 1 && rel.as_os_str() == name).then(|| name.to_string())
}

/// R31.4 collision: stock appends ` <n>` to the STEM, keeping the extension
/// (recon: `Pasted image ... .png` -> `... 1.png` -> `... 2.png`). Never an
/// overwrite (S5), so this is a create_new() loop, not an exists() test.
fn free_dest(dir: &Path, name: &str) -> Result<(PathBuf, String, fs::File), Refused> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 0..1000 {
        let cand = if n == 0 { name.to_string() } else { format!("{stem} {n}{ext}") };
        match fs::OpenOptions::new().write(true).create_new(true).open(dir.join(&cand)) {
            Ok(f) => return Ok((dir.join(&cand), cand, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Refused::Io(e.to_string())),
        }
    }
    Err(Refused::NoFreeName)
}

/// copy at most MAX_IMG_BYTES + 1 bytes; Err leaves NOTHING behind.
fn copy_capped(src: &Path, dst: &Path, mut out: fs::File) -> Result<u64, Refused> {
    use std::io::{Read, Write};
    // free_dest already created `dst` with O_EXCL, so EVERY exit from here on
    // must unlink it — a failed drop that leaves an empty `cat.png` in the
    // vault is worse than the refusal it reports.
    let mut f = match fs::File::open(src) {
        Ok(f) => f,
        Err(e) => {
            let _ = fs::remove_file(dst);
            // R31.12: the sandbox only grants READ on the drop-source folders,
            // so EACCES here is the expected answer for a file anywhere else.
            // Say THAT, not "could not be copied (os error 13)" — a refusal
            // that misdescribes its cause is the bug trap (g) is about.
            return Err(if e.kind() == std::io::ErrorKind::PermissionDenied {
                Refused::Unreadable
            } else {
                Refused::Io(e.to_string())
            });
        }
    };
    let mut buf = [0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                let _ = fs::remove_file(dst);
                return Err(Refused::Io(e.to_string()));
            }
        };
        total += n as u64;
        if total > MAX_IMG_BYTES {
            let _ = fs::remove_file(dst);           // a file that GREW mid-copy
            return Err(Refused::TooBig(total));
        }
        if let Err(e) = out.write_all(&buf[..n]) {
            let _ = fs::remove_file(dst);
            return Err(Refused::Io(e.to_string()));
        }
    }
    if let Err(e) = out.sync_all() {
        let _ = fs::remove_file(dst);
        return Err(Refused::Io(e.to_string()));
    }
    Ok(total)
}

/// R31 THE function. Copies every droppable image into the vault's attachment
/// folder and returns the markdown to insert at the cursor. Never moves, never
/// symlinks, never overwrites. The caller inserts the text and teaches the
/// index the new images (R29.11: the index is what both renderers resolve
/// against, so a copied file that is not in the index would not paint).
fn attach_drop(root: &Path, note: &str, paths: &[PathBuf]) -> Result<Attached, DropErr> {
    if paths.is_empty() {
        return Err(DropErr::Empty);
    }
    if note.is_empty() || note_path_in(root, note, false).is_none() {
        return Err(DropErr::NoNote);
    }
    let (dir, notice) = attach_dir(root)?;
    let croot = root.canonicalize().map_err(|_| DropErr::BadAttachDir)?;
    let rel_dir = dir.strip_prefix(&croot).ok().map(|d| d.display().to_string()).unwrap_or_default();
    let mut out = Attached::default();
    out.refused.extend(notice);
    let mut links: Vec<String> = Vec::new();
    for p in paths {
        let shown = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.display().to_string());
        match attach_one(&dir, p) {
            Ok(name) => {
                links.push(format!("![[{name}]]"));
                out.copied.push(if rel_dir.is_empty() { name.clone() } else { format!("{rel_dir}/{name}") });
            }
            Err(r) => out.refused.push(r.say(&shown)),
        }
    }
    out.text = links.join("\n");
    Ok(out)
}

/// one dropped path -> the name it got in the attachment folder
fn attach_one(dir: &Path, p: &Path) -> Result<String, Refused> {
    let raw = p.to_string_lossy();
    // R31.8 / R29.10: a browser drag delivers a URL, never a file. Downloading
    // it would be a network fetch caused by a note, which is forbidden — and
    // the refusal says exactly that instead of "could not be found".
    if raw.contains("://") || raw.starts_with("http:") || raw.starts_with("https:") || raw.starts_with("data:") {
        return Err(Refused::Remote);
    }
    let name = attach_name(p).ok_or(Refused::BadName)?;
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    if !IMG_TYPES.iter().any(|(e, _)| *e == ext) {
        return Err(Refused::BadExt(ext));
    }
    // S1: symlink_metadata, so a symlinked source is REFUSED and not followed
    let m = fs::symlink_metadata(p).map_err(|_| Refused::NotAFile)?;
    if m.is_symlink() {
        return Err(Refused::Symlink);
    }
    if !m.is_file() {
        return Err(Refused::NotAFile);
    }
    if m.len() > MAX_IMG_BYTES {
        return Err(Refused::TooBig(m.len()));
    }
    let src = p.canonicalize().map_err(|_| Refused::NotAFile)?;
    if !fs::symlink_metadata(&src).map(|m| m.is_file()).unwrap_or(false) {
        return Err(Refused::NotAFile);
    }
    let (dst, name, fh) = free_dest(dir, &name)?;
    copy_capped(&src, &dst, fh)?;
    Ok(name)
}

/* R31 THE WRAPPER — 7 statements, no behaviour. An OS drop is delivered by
   wry/GTK to Rust and re-emitted by Tauri itself as `tauri://drag-drop`
   (tauri-2.11.5 manager/webview.rs:722), so the DOM never sees a DataTransfer
   with files and there is nothing to intercept in JS: ui/main.js listens for
   that event and calls this command. `async` so a 32 MB copy runs on the
   async runtime instead of the event loop (rule of order: no lag the user can
   feel). The index learns the copied images here because BOTH renderers
   resolve `![[x.png]]` against `Index::images()` (R29.11) — a file copied but
   not indexed would not paint. */
#[tauri::command]
async fn attach_files(
    v: State<'_, Vault>,
    note: String,
    paths: Vec<PathBuf>,
    otel: Option<perf::Ctx>,
) -> Result<Attached, String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    let n = paths.len();
    let a = span_timed!(otel => "attach_files", attach_drop(&root, &note, &paths), serde_json::json!({"files": n}))
        .map_err(|e| e.say())?;
    let mut ix = v.index.lock().unwrap();
    for rel in &a.copied {
        ix.add_image(rel);
    }
    Ok(a)
}

/// S5: read a note off disk, "" when it is oversized (never part of the vault)
fn read_capped(p: &Path) -> Option<String> {
    let m = fs::symlink_metadata(p).ok()?;
    if m.len() > index::MAX_NOTE_BYTES {
        index::warn_oversized(p);
        return None;
    }
    fs::read_to_string(p).ok()
}

fn walk_dirs(dir: &Path, base: &Path, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            if let Ok(rel) = p.strip_prefix(base) {
                out.push(rel.display().to_string());
            }
            walk_dirs(&p, base, out);
        }
    }
}

#[tauri::command]
fn list_folders(v: State<Vault>) -> Vec<String> {
    span_timed!("list_folders", {
        let Some(root) = cur_vault(&v) else { return vec![] };
        let mut out = Vec::new();
        walk_dirs(&root, &root, &mut out);
        out.sort();
        out
    })
}

#[tauri::command]
fn create_dir(v: State<Vault>, name: String) -> Result<(), String> {
    span_timed!("create_dir", {
        let root = cur_vault(&v).ok_or("no vault open")?;
        let rel = safe_rel(&name).ok_or("invalid folder name")?;
        // perf-index: an empty dir holds no notes -> index unchanged.
        // S2: mkdir via note_path_in (create) so it never crosses a symlinked dir
        note_path_in(&root, &format!("{}/x", rel.display()), true)
            .map(|_| ())
            .ok_or_else(|| "outside vault".to_string())
    })
}

#[tauri::command]
fn list_notes(v: State<Vault>, otel: Option<perf::Ctx>) -> Vec<String> {
    span_timed!(otel => "list_notes", cur_notes(&v))
}

/// R29.6/R29.7: the same image list the Rust renderer resolves against, handed
/// to live preview. ONE source of truth for "does this embed resolve?", so the
/// two engines cannot disagree about a name (the `extlink` phase exists because
/// they disagreed once before).
#[tauri::command]
fn list_images(v: State<Vault>, otel: Option<perf::Ctx>) -> Vec<String> {
    span_timed!(otel => "list_images", v.index.lock().unwrap().images().to_vec())
}

#[tauri::command]
fn read_note(v: State<Vault>, name: String, otel: Option<perf::Ctx>) -> String {
    span_timed!(otel =>
        "read_note",
        note_path(&v, &name, false)
            .and_then(|p| read_capped(&p))
            .unwrap_or_default()
    )
}

// F1 (dataloss-audit): the Result is the product. A save that did not land
// MUST reach the UI — a swallowed ENOSPC/EROFS costs every edit of a session.
#[tauri::command]
fn write_note(v: State<Vault>, name: String, content: String, otel: Option<perf::Ctx>) -> Result<(), String> {
    let bytes = content.len();
    span_timed!(otel => "write_note", write_note_inner(&v, &name, &content), serde_json::json!({"bytes": bytes}))
}

fn write_note_inner(v: &State<Vault>, name: &str, content: &str) -> Result<(), String> {
    let root = cur_vault(v).ok_or("no vault open")?;
    // R11: lock BEFORE the write — the watcher reads+compares under this
    // lock, so it never sees our bytes on disk without them in the index
    let mut ix = v.index.lock().unwrap();
    write_note_in(&root, &mut ix, name, content)
}

/* F1/F3 dataloss: the pure core of a note save, so the durability and the
   error path are testable without Tauri. Every failure is RETURNED (F1) and
   the index is upserted only after the bytes are on disk (index == disk). */
fn write_note_in(root: &Path, ix: &mut Index, name: &str, content: &str) -> Result<(), String> {
    let rel = safe_rel(name).ok_or("invalid name")?;
    // S2: parents created + confined inside note_path_in (None = outside vault)
    let p = note_path_in(root, name, true).ok_or("outside vault")?;
    write_atomic(&p, content)?;
    // index == disk: reparse this note, patch its outgoing edges
    // (a NEW key triggers a full in-memory edge rebuild inside upsert)
    ix.upsert(&rel.display().to_string(), content);
    Ok(())
}

/* F3 dataloss: durable replace, std only. A truncating fs::write releases the
   old bytes at open(2) time, so a crash or ENOSPC mid-write leaves a zero-byte
   or half-written note; and without fsync even a returned write is only page
   cache. Write a SIBLING temp (same directory => same filesystem => the rename
   is atomic; ".tmp" is not ".md", so notes_of and the watcher never index it),
   fsync it, then rename over the target. Failure leaves the OLD file intact. */
fn write_atomic(p: &Path, content: &str) -> Result<(), String> {
    use std::io::Write;
    let tmp = p.with_extension("md.tmp");
    let r = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, p)
    })();
    if r.is_err() {
        let _ = fs::remove_file(&tmp); // never leave a stray temp behind
    }
    r.map_err(|e| e.to_string())
}

/// F4 dataloss: the error a create returns when the note is already there.
/// The UI matches on it to OPEN the existing note instead of clobbering it.
const EXISTS: &str = "exists";

/* F4 dataloss: creation must never replace an existing note with a stub.
   create_new(2) is the kernel's atomic exists-check, so unlike a JS-side
   `if (!notesCache.includes(n))` there is no window in which another writer
   (git checkout, sync client, the 1000ms-stale index) can land a real file
   between the check and the truncate. */
#[tauri::command]
fn create_note(v: State<Vault>, name: String, content: Option<String>, otel: Option<perf::Ctx>) -> Result<(), String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    let mut ix = v.index.lock().unwrap();
    // feedback #20: an ABSENT content is an EMPTY note, not a seeded one. The
    // default lives here as well as at ui/main.js createNote so that neither
    // side can re-mint "# name" on its own; a new note is zero bytes on disk.
    let content = content.unwrap_or_default();
    span_timed!(otel => "create_note", create_note_in(&root, &mut ix, &name, &content))
}

fn create_note_in(root: &Path, ix: &mut Index, name: &str, content: &str) -> Result<(), String> {
    use std::io::Write;
    let rel = safe_rel(name).ok_or("invalid name")?;
    let p = note_path_in(root, name, true).ok_or("outside vault")?;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists { EXISTS.to_string() } else { e.to_string() }
        })?;
    f.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;
    ix.upsert(&rel.display().to_string(), content);
    Ok(())
}


/* m5 F2 rename: fs::rename old.md -> new.md inside root. Parents created,
   overwrite refused. Wikilinks updated vault-wide after the move —
   perf-index: the rewrite runs over the in-memory index (no vault read);
   only notes whose text changed are written back. Pure-ish core for tests.

   R34: this is now the COMPOSITE of the two halves below. F2 keeps its
   one-shot semantics; the title-rename path calls move_note_in, shows the
   counted prompt, and only then calls update_links_in. */
fn rename_in(root: &Path, ix: &mut Index, old: &str, new: &str) -> Result<(), String> {
    move_note_in(root, ix, old, new)?;
    update_links_in(root, ix, old, new);
    Ok(())
}

/* R34.1/R34.3: the MOVE, and NOTHING else. Not one inbound [[old]] is
   touched — rewriting them is a separate, consented call. Returns the blast
   radius (links, files) counted BEFORE the move, which is the sentence the
   Update links modal states.

   Collision is refused by the KERNEL, not by a stat: create_new(2) claims the
   target name atomically (O_EXCL), so unlike `if np.exists()` there is no
   window in which another writer (sync client, git checkout, a second
   rename) can land a real file between the check and the move. Only after
   the claim is ours does rename(2) run — it overwrites exactly one file, the
   zero-byte placeholder we just created. A crash between the two leaves that
   placeholder behind; an empty note is a visible, recoverable state, a
   clobbered one is not. */
fn move_note_in(root: &Path, ix: &mut Index, old: &str, new: &str) -> Result<(usize, usize), String> {
    let orel = safe_rel(old).ok_or("invalid name")?;
    let nrel = safe_rel(new).ok_or("invalid name")?;
    // S2: both ends confined to the vault (symlinked source/parent -> refused)
    let op = note_path_in(root, old, false).ok_or("invalid name")?;
    if !op.is_file() {
        return Err("no such note".into());
    }
    let np = note_path_in(root, new, true).ok_or("invalid name")?;
    // rename is rare and rewrites text vault-wide: resync the index from disk
    // FIRST so a note another writer dropped in since boot (smoke seeds one;
    // LATER: file watcher) gets its [[old]] links rewritten too. One walk per
    // rename — the hot paths (search/graph/backlinks) stay disk-free.
    *ix = Index::build(root);
    let (okey, nkey) = (orel.display().to_string(), nrel.display().to_string());
    // counted BEFORE the move, off the freshly-resynced index
    let radius = ix.rename_blast(&okey, &nkey);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&np)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                "target exists".to_string()
            } else {
                e.to_string()
            }
        })?;
    if let Err(e) = fs::rename(&op, &np) {
        let _ = fs::remove_file(&np); // never leave a stray claim behind
        return Err(e.to_string());
    }
    // index==disk invariant: if the key is somehow missing, seed it from the
    // moved file rather than dropping the note
    let fallback = if ix.content(&okey).is_none() { fs::read_to_string(&np).ok() } else { None };
    ix.move_key(&okey, &nkey, fallback);
    // R9.8: the note's name changed, so its BOOKMARK follows it — here, after
    // the rename is a fact on disk, and never before: everything above this
    // line can still return Err, and a refused rename must leave
    // .obsidian/bookmarks.json byte-for-byte unchanged. See rename_bookmark_in.
    // This is also the reason the fix is not in rename_in: the move_note
    // command calls move_note_in DIRECTLY, so a note MOVED to another folder
    // changes its vault-relative name by exactly this code and keeps its
    // bookmark for free.
    rename_bookmark_in(root, &okey, &nkey);
    Ok(radius)
}

/* R34.3: the CONSENTED half — rewrite every inbound [[old]] to [[new]],
   preserving alias, #anchor and embed form (index::rewrite_links). Returns
   how many files were written. Never called without the user's answer. */
fn update_links_in(root: &Path, ix: &mut Index, old: &str, new: &str) -> usize {
    let okey = safe_rel(old).map(|p| p.display().to_string()).unwrap_or_default();
    let nkey = safe_rel(new).map(|p| p.display().to_string()).unwrap_or_default();
    let mut wrote = 0;
    for (n, c) in ix.rewrite_to(&okey, &nkey) {
        // S2: never write through a symlink swapped in since the walk
        if let Some(p) = note_path_in(root, &n, false) {
            if fs::write(&p, c).is_ok() {
                wrote += 1;
            }
        }
    }
    wrote
}

/// R34.3: what the Update links modal states — "This will affect {links}
/// link[s] in {files} file[s]". files==0 means stock shows NO modal.
#[derive(serde::Serialize)]
struct Blast {
    links: usize,
    files: usize,
}

/* R34.1: the title-rename backend. The file moves NOW (stock renames on
   Enter, before any question is asked) and the caller is handed the blast
   radius to put in the prompt. Inbound links are untouched until the caller
   answers with update_links. */
#[tauri::command]
fn move_note(v: State<Vault>, old: String, new: String, otel: Option<perf::Ctx>) -> Result<Blast, String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    let mut ix = v.index.lock().unwrap();
    let (links, files) = span_timed!(otel => "move_note", move_note_in(&root, &mut ix, &old, &new))?;
    Ok(Blast { links, files })
}

/* R34.3: the answer to the prompt. Separate command on purpose — a rename
   that rewrote links without this call could not be told from one that did,
   and the phase's negative control is exactly that difference. */
#[tauri::command]
fn update_links(v: State<Vault>, old: String, new: String, otel: Option<perf::Ctx>) -> Result<usize, String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    let mut ix = v.index.lock().unwrap();
    Ok(span_timed!(otel => "update_links", update_links_in(&root, &mut ix, &old, &new)))
}

/* R34.8 — CONSENT IS REMEMBERED IN THE VAULT, not in the session. Measured on
   stock: answering `Always update` wrote `.obsidian/app.json` =
   {"alwaysUpdateLinks": true} (31 bytes, committed as
   docs/recon-title-rename/D-app.json-after-always-update) and the SECOND
   rename in the same session raised no modal while still rewriting all four
   links. So the prompt is conditional on this flag, and the flag outlives the
   process — which is why it is read off disk rather than cached in the UI. */
fn link_consent_in(root: &Path) -> bool {
    let cfg = fs::read_to_string(root.join(".obsidian/app.json")).unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&cfg)
        .ok()
        .and_then(|v| v.get("alwaysUpdateLinks").and_then(|b| b.as_bool()))
        .unwrap_or(false)
}

/* The write half. app.json is the USER's file and holds keys we do not own
   (attachmentFolderPath, R31.3), so this is a MERGE into the parsed object and
   never a fresh document. An app.json we cannot parse, or one that is not an
   object, is REFUSED rather than replaced: losing someone's vault config to
   remember a checkbox is the same trade as overwriting a note on collision,
   and it goes the same way. The write is temp+fsync+rename, so a crash cannot
   leave a truncated config either. */
fn set_link_consent_in(root: &Path, on: bool) -> Result<(), String> {
    use std::io::Write;
    let dir = root.join(".obsidian");
    let p = dir.join("app.json");
    let cur = fs::read_to_string(&p).unwrap_or_default();
    let mut v = if cur.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str::<serde_json::Value>(&cur).map_err(|e| format!("app.json is not JSON ({e}) — refusing to overwrite it"))?
    };
    if !v.is_object() {
        return Err("app.json is not a JSON object — refusing to overwrite it".into());
    }
    v.as_object_mut().unwrap().insert("alwaysUpdateLinks".into(), serde_json::Value::Bool(on));
    let body = serde_json::to_string(&v).map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = dir.join("app.json.tmp");
    let r = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, &p)
    })();
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r.map_err(|e| e.to_string())
}

/// R34.8: does the vault already carry the user's answer? `true` -> the UI
/// rewrites links without asking; `false` -> the modal.
#[tauri::command]
fn link_consent(v: State<Vault>) -> Result<bool, String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    Ok(link_consent_in(&root))
}

/// R34.8: record the `Always update` answer in the vault.
#[tauri::command]
fn set_link_consent(v: State<Vault>, on: bool) -> Result<(), String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    set_link_consent_in(&root, on)
}

#[tauri::command]
fn rename_note(v: State<Vault>, old: String, new: String, otel: Option<perf::Ctx>) -> Result<(), String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    let mut ix = v.index.lock().unwrap();
    span_timed!(otel => "rename_note", rename_in(&root, &mut ix, &old, &new))
}

/* R24.7/R24.8 — DELETE, AND NOTHING BUT DELETE. ------------------------------

   R24.8 is the whole rule of this path: *a delete NEVER edits linking notes*.
   So this function is deliberately NOT built on rename_in — it calls neither
   update_links_in nor rewrite_to, and the only path it writes through is the
   deleted note's own. What the user sees instead is R24.9, which is already
   shipped: once the key leaves the index the inbound `[[Target]]` links stop
   resolving and render faded. That faded link IS the notification; rewriting
   the linking notes would destroy the user's text to hide a fact.

   WHERE IT GOES. Stock says "moved to your system trash". This app cannot
   honestly say that: `sandbox.rs::write_roots` grants write access to /tmp,
   /dev, /run, /var/tmp, ~/.cache, ~/.local/share/dev.koto.rustidian, the vault
   and the config file — and to nothing else. ~/.local/share/Trash is NOT in
   that set, so under an enforced Landlock ruleset (which the box enforces) an
   XDG trash move is EACCES, and widening the ruleset to reach a directory full
   of other applications' deleted files is a security decision, not a file-ops
   one. The destination is therefore the vault's own `.trash/` — which is
   stock's other documented option, is inside the one directory we may already
   write, and is on the SAME FILESYSTEM as the note, so the move is one
   rename(2): atomic, never a copy, and it cannot half-delete a note by filling
   a disk. `walk()` (index.rs:374) skips every dot-prefixed entry and
   `safe_rel` refuses one, so the trashed copy is invisible to the explorer,
   the index, search and the graph — it is gone from the vault in every sense
   the user can observe, and still recoverable with a file manager. The
   confirmation names THIS destination; a dialog that says "system trash" while
   writing somewhere else is the same class of lie as a comment that outruns
   its code.

   NOT TOUCHED, ON PURPOSE: `.obsidian/bookmarks.json`. A delete writes the deleted
   note's path and nothing else, and this codebase already tolerates a bookmark
   with no file behind it (see rename_bookmark_in's "the target name may be a
   stale bookmark with no file"). R24 says nothing about bookmarks; the
   conservative reading of R24.8 is that a delete edits no other file AT ALL,
   and a unit test pins the bookmarks file byte-identical across a delete. */
const TRASH_DIR: &str = ".trash";

/// `<vault>/.trash/<base>.md`, suffixed `.1`, `.2`, ... when that name is
/// already taken — deleting two notes that share a basename (`a/Note` and
/// `b/Note`) must not silently overwrite the first one's only remaining copy.
fn trash_dest(tdir: &Path, base: &str) -> PathBuf {
    let first = tdir.join(format!("{base}.md"));
    if !first.exists() {
        return first;
    }
    for i in 1..10_000 {
        let p = tdir.join(format!("{base}.{i}.md"));
        if !p.exists() {
            return p;
        }
    }
    tdir.join(format!("{base}.{}.md", std::process::id()))
}

/// Returns the vault-relative path the note now occupies inside the trash —
/// what the UI reports, and what a test reads the original bytes back from.
fn delete_note_in(root: &Path, ix: &mut Index, name: &str) -> Result<String, String> {
    let rel = safe_rel(name).ok_or("invalid name")?;
    let key = rel.display().to_string();
    // S2: confined to the vault, and never through a symlinked leaf/parent
    let p = note_path_in(root, name, false).ok_or("invalid name")?;
    if !p.is_file() {
        return Err("no such note".into());
    }
    let tdir = root.join(TRASH_DIR);
    fs::create_dir_all(&tdir).map_err(|e| format!("cannot open the vault trash: {e}"))?;
    let base = rel.file_name().ok_or("invalid name")?.to_string_lossy().into_owned();
    let dest = trash_dest(&tdir, &base);
    // one rename(2) inside the vault: the note is either where it was or in the
    // trash, never in neither place and never in two
    fs::rename(&p, &dest).map_err(|e| e.to_string())?;
    // index==disk: the key is gone, so every inbound link stops resolving and
    // R24.9's faded rendering appears. No other note is read, written or parsed.
    ix.remove(&key);
    Ok(dest.strip_prefix(root).unwrap_or(&dest).display().to_string())
}

/// R24.7: the explorer's Delete, behind the confirmation the UI raises. The
/// count the dialog states comes from `backlinks_ctx` (links per line, files
/// per note) — no second counter for this path, and none is added here.
#[tauri::command]
fn delete_note(v: State<Vault>, name: String, otel: Option<perf::Ctx>) -> Result<String, String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    let mut ix = v.index.lock().unwrap();
    span_timed!(otel => "delete_note", delete_note_in(&root, &mut ix, &name))
}

#[tauri::command]
fn vault_get(v: State<Vault>) -> Option<String> {
    cur_vault(&v).map(|p| p.display().to_string())
}

/* R1.6 vault persistence: ~/.rustidian.json {"last": path, "list": [paths]}.
   Written only on explicit open/create — VAULT_DIR boots (probes) never touch it. */
fn cfg_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into())).join(".rustidian.json")
}

/// whole config as a Value — extra keys (sidebar_w, ...) survive rewrites
fn cfg_value() -> serde_json::Value {
    fs::read_to_string(cfg_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn read_cfg() -> (Option<String>, Vec<String>) {
    let v = cfg_value();
    let last = v["last"].as_str().map(String::from);
    let list = v["list"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    (last, list)
}

/// MRU push: dedup, newest first, capped at 8 — pure for testability
fn push_recent(mut list: Vec<String>, path: &str) -> Vec<String> {
    list.retain(|x| x != path);
    list.insert(0, path.to_string());
    list.truncate(8);
    list
}

fn persist_vault(p: &Path) {
    let s = p.display().to_string();
    let (_, list) = read_cfg();
    let list = push_recent(list, &s);
    let mut v = cfg_value(); // keep sidebar_w & future keys
    v["last"] = serde_json::json!(s);
    v["list"] = serde_json::json!(list);
    let _ = fs::write(cfg_path(), v.to_string());
}

/* ux-4: left sidebar width persistence (clamped 150-600, default 200 total
   = 44 ribbon + 156 #side stays when the key is absent) */
#[tauri::command]
fn get_sidebar_w() -> Option<u64> {
    cfg_value()["sidebar_w"].as_u64()
}

#[tauri::command]
fn set_sidebar_w(w: u64) {
    let mut v = cfg_value();
    v["sidebar_w"] = serde_json::json!(w.clamp(150, 600));
    let _ = fs::write(cfg_path(), v.to_string());
}

#[tauri::command]
fn recent_vaults() -> Vec<String> {
    span_timed!("recent_vaults", read_cfg().1.into_iter().filter(|p| Path::new(p).is_dir()).collect())
}

#[tauri::command]
fn set_vault(v: State<Vault>, path: String, otel: Option<perf::Ctx>) -> Result<String, String> {
    span_timed!(otel => "set_vault", set_vault_inner(&v, &path))
}

fn set_vault_inner(v: &State<Vault>, path: &str) -> Result<String, String> {
    let p = PathBuf::from(path.trim());
    if !picker_allows(&p) {
        return Err(format!("not allowed as a vault: {}", p.display())); // S4
    }
    if !p.is_dir() {
        return Err(format!("not a directory: {}", p.display()));
    }
    if !sandbox::allows(&p) {
        persist_vault(&p); // next boot confines to this one instead
        return Err(format!("sandboxed to {} — vault saved, restart rustidian to open it", sandbox::confined_to().unwrap().display()));
    }
    persist_vault(&p);
    open_vault(v, &p);
    Ok(p.display().to_string())
}

/* feedback #20 [R32.6]: the starter note a NEW vault is seeded with. Recon
   against stock 1.13.7 (2026-09-12, shots $RECON/shots5/E0-E9): stock's own
   create-vault flow writes exactly one file, `Welcome.md`, 203 bytes, and it
   carries NO heading — it opens on prose ("This is your new *vault*.") while
   the big "Welcome" on screen is the INLINE TITLE, the filename rendered.
   So the `# Welcome` line rustidian used to seed was redundant the moment the
   inline title landed: it drew the word twice, once from the filename and
   once from bytes we wrote ourselves.
   Seeding CONTENT is deliberate and stays (R1.2) — it is NOT note creation,
   which materializes zero bytes (`create_note`). The prose is rustidian's own;
   only the heading is dropped. Deliberate delta from stock: we keep a trailing
   newline (stock's seed ends without one) because a text file should end in \n. */
const NEW_VAULT_SEED_NAME: &str = "Welcome.md";
const NEW_VAULT_SEED: &str =
    "This is your new vault. Notes are plain Markdown files.\nLink them with [[Wiki Links]].\n";

fn seed_new_vault(dir: &Path) -> std::io::Result<()> {
    fs::write(dir.join(NEW_VAULT_SEED_NAME), NEW_VAULT_SEED)
}

#[tauri::command]
fn create_vault(v: State<Vault>, parent: String, name: String) -> Result<String, String> {
    span_timed!("create_vault", {
        let name = name.trim();
        if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
            return Err("invalid vault name".into());
        }
        let p = PathBuf::from(parent.trim()).join(name);
        if !picker_allows(&p) {
            return Err(format!("not allowed as a vault: {}", p.display())); // S4
        }
        if p.exists() {
            return Err(format!("already exists: {}", p.display()));
        }
        if !sandbox::allows(Path::new(parent.trim())) {
            return Err(format!("sandboxed to {} — create the folder outside rustidian, then pick it and restart", sandbox::confined_to().unwrap().display()));
        }
        fs::create_dir_all(&p).map_err(|e| e.to_string())?;
        seed_new_vault(&p).map_err(|e| e.to_string())?;
        persist_vault(&p);
        open_vault(&v, &p);
        Ok(p.display().to_string())
    })
}

/// otel (R18): frontend spans (ui/otel.js) arrive in ONE batch per 250ms — [{name, traceId, spanId,
/// parentSpanId, startMs, endMs, attrs}] — and land in the same RUSTIDIAN_OTEL file as backend spans,
/// one OTLP/JSON request line per batch. Returns false when telemetry is off so the UI stops sending.
#[tauri::command]
fn log_spans(spans: Vec<serde_json::Value>) -> bool {
    perf::ui_spans(&spans);
    // TRUE unconditionally, and that is the perf-console change. The frontend
    // uses this reply to decide whether to keep measuring at all (ui/otel.js:
    // `on === false` makes every later call a no-op), and the slow-op console
    // warning needs UI spans in EVERY run, not only in runs that set
    // RUSTIDIAN_OTEL. ui_spans() above warns on a breach whether or not a trace
    // file is being written; returning perf::enabled() here would have switched
    // the whole feature off for ordinary users — the ones who actually feel the
    // lag. Cost when untraced: one IPC per 250ms while spans are being produced,
    // and no file I/O.
    true
}

/// graph-webgl: hidden hooks for the graph draw path. RUSTIDIAN_GRAPH_RENDERER=gl|2d forces a
/// renderer (tests); RUSTIDIAN_GRAPH_LOSE_CTX=1 makes the UI lose its WebGL context once the sim
/// settled (smoke graphgl: the 2D fallback must keep drawing).
#[tauri::command]
fn graph_renderer_pref() -> serde_json::Value {
    serde_json::json!({
        "renderer": std::env::var("RUSTIDIAN_GRAPH_RENDERER").ok(),
        "lose_ctx": std::env::var_os("RUSTIDIAN_GRAPH_LOSE_CTX").is_some(),
    })
}

/// R34.18 inline-title TYPE PROBE — a test-only hook, OFF unless RUSTIDIAN_TYPEPROBE=1.
/// It buys the smoke two things it cannot get from outside the webview:
///   1. the census publishes [tty:]/[lpc:] — computed type of the rendered title
///      and of the caret surface, and the scroller's child-list signature;
///   2. three chords (ctrl+alt+shift+1/2/3) perturb the caret surface's type,
///      restore it, and move --font-text-size, so `fast titletype` can show its
///      own assertions going RED and back GREEN inside ONE run.
/// Gating it here (rather than publishing the tokens always) is deliberate: no
/// pre-existing phase's census string changes, so the gate keeps judging the
/// same bytes it judged before.
#[tauri::command]
fn type_probe() -> bool {
    std::env::var("RUSTIDIAN_TYPEPROBE").as_deref() == Ok("1")
}

/// themecsp RIG APPLICATOR — a test-only hook, OFF unless RUSTIDIAN_SMOKE_CSS=<path>.
/// It returns the contents of the named CSS file so the smoke can inject it as an
/// inline <style>: style-src 'unsafe-inline' means the STYLE is ALLOWED TO EXIST,
/// and the property the egress proof falsifies is that nothing inside it can REACH
/// the network. This models the threat (attacker-controlled CSS in the document)
/// with the CHEAPEST possible injector.
///   NOT a CSS loader: no product feature calls this, it is inert without the env
///   var, a shipped build never sets it, and it takes an absolute path the OPERATOR
///   chose (the test rig), never a vault-relative or user-influenced name. The
///   census then publishes body's computed style so a phase can prove a stock-shaped
///   rule won a pixel (crit 5). Precedent: type_probe (RUSTIDIAN_TYPEPROBE), main.rs.
#[tauri::command]
fn smoke_css() -> Option<String> {
    let path = std::env::var("RUSTIDIAN_SMOKE_CSS").ok()?;
    std::fs::read_to_string(&path).ok()
}

/// F2 (dataloss-audit) test hook: the vault-switch race lives INSIDE the save
/// debounce window, so at 250ms it is not mechanically reproducible.
/// RUSTIDIAN_SAVE_MS widens the window for the smoke; every normal run gets
/// the stock 250ms (the env var is absent, and out-of-range values are ignored).
#[tauri::command]
fn save_debounce_ms() -> u64 {
    std::env::var("RUSTIDIAN_SAVE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| (1..=60_000).contains(n))
        .unwrap_or(250)
}

/// S4: the picker commands take arbitrary absolute paths from the webview.
/// Deny system trees and any dot-component (hidden dirs, `..`); /workspace
/// and $HOME stay browsable even when they sit under a denied prefix (e.g.
/// HOME=/root). /, /home, /mnt, /media, /run/media, /tmp remain open.
fn picker_allows(p: &Path) -> bool {
    const DENY: [&str; 8] = ["/proc", "/sys", "/dev", "/etc", "/usr", "/boot", "/root", "/var"];
    if !p.is_absolute() {
        return false;
    }
    if p.components().any(|c| match c {
        Component::Normal(s) => s.to_string_lossy().starts_with('.'),
        Component::RootDir => false,
        _ => true, // `..`, `.`, prefixes
    }) {
        return false;
    }
    if p.starts_with("/workspace") {
        return true;
    }
    if let Ok(h) = std::env::var("HOME") {
        if h.len() > 1 && p.starts_with(&h) {
            return true;
        }
    }
    !DENY.iter().any(|d| p.starts_with(d))
}

#[tauri::command]
fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".into())
}

/// S1: the only way an external URL leaves the app — the webview never
/// navigates (on_navigation + main.js capture). Scheme re-checked here: the
/// DOM is attacker-influenced, the render allowlist is not the trust boundary.
fn check_external(url: &str) -> Result<(), String> {
    if ext_ok(url, false) { Ok(()) } else { Err(format!("blocked scheme: {url}")) }
}

#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    check_external(&url)?;
    // std::process::Command only — no shell, no opener plugin. Missing xdg-open
    // (headless smoke, minimal hosts) is a plain Err, never a fallback.
    std::process::Command::new("xdg-open")
        .arg(&url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("xdg-open: {e}"))
}

#[tauri::command]
fn list_dirs(path: String) -> Vec<String> {
    span_timed!("list_dirs", {
        if !picker_allows(Path::new(&path)) {
            return vec![]; // S4: the picker just shows ".." there
        }
        let mut v: Vec<String> = fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !n.starts_with('.'))
            .collect();
        v.sort();
        v
    })
}


/// html-escape for text content and attribute values (H1 fix)
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// R10.1 link label: alias wins; `[[#H]]` shows "H"; otherwise the raw target
/// (LP keeps the '#', reading view joins with " > " like stock)
fn link_label(note: &str, anchor: &str, alias: &str, reading: bool) -> String {
    if !alias.is_empty() {
        alias.to_string()
    } else if note.is_empty() {
        anchor.trim_start_matches('#').to_string()
    } else if reading {
        format!("{note}{}", anchor.replace('#', " > "))
    } else {
        format!("{note}{anchor}")
    }
}

/// scan coalesced text for [[wikilinks]], emitting escaped anchors (H1 fix).
/// data-note = note part (empty = same note), data-anchor = heading text or
/// ^blockid (no leading '#') so the frontend can scroll after navigating.
fn linkify(buf: &str, notes: &[String], imgs: &[String], reading: bool, evs: &mut Vec<Event>) {
    let mut rest = buf;
    while let Some(i) = rest.find("[[") {
        let Some(j) = rest[i + 2..].find("]]") else { break };
        let l = &rest[i + 2..i + 2 + j];
        let (note, anchor, alias) = link_parts(l);
        // R29.1 `![[pic.png]]`: the bang belongs to the embed, so it must not
        // be emitted as text. Only IMAGE extensions take this branch — a note
        // embed `![[Second Note]]` stays the link it is today (req 410).
        if i > 0 && rest.as_bytes()[i - 1] == b'!' && anchor.is_empty() && is_img_target(note) {
            tagify(&rest[..i - 1], evs);
            let alt = if alias.is_empty() { note } else { alias };
            evs.push(Event::Html(image_html(imgs, note, alt).into()));
            rest = &rest[i + 2 + j + 2..];
            continue;
        }
        tagify(&rest[..i], evs);
        let cls = if note.is_empty() || resolve(notes, l).is_some() {
            "wiki"
        } else {
            "wiki wiki-unresolved"
        };
        evs.push(Event::Html(
            format!(
                "<a href=\"#\" class=\"{cls}\" data-note=\"{}\" data-anchor=\"{}\">{}</a>",
                esc(note),
                esc(anchor.trim_start_matches('#')),
                esc(&link_label(note, anchor, alias, reading))
            )
            .into(),
        ));
        rest = &rest[i + 2 + j + 2..];
    }
    if !rest.is_empty() {
        tagify(rest, evs);
    }
}

/// R10.3 block id: ` ^id` (letters/digits/-) at the very end of a block ->
/// (text without it, id). Stock hides it in reading view, shows a small grey
/// label in LP — both via CSS on span.blockid.
fn split_block_id(s: &str) -> Option<(&str, &str)> {
    let t = s.trim_end();
    let i = t.rfind(" ^")?;
    let id = &t[i + 2..];
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    Some((&t[..i], id))
}

/// tags: inline #tag -> pill anchor (label + data-tag escaped like wikilinks).
/// Only ever fed text between wikilinks, never code (linkify's callers skip
/// code blocks/spans + frontmatter), so `#tag` in code stays literal.
fn tagify(buf: &str, evs: &mut Vec<Event>) {
    let mut at = 0;
    for (a, b) in tag_spans(buf) {
        evs.push(Event::Text(buf[at..a].to_string().into()));
        let t = esc(&buf[a + 1..b]);
        evs.push(Event::Html(format!("<a href=\"#\" class=\"tag\" data-tag=\"{t}\">#{t}</a>").into()));
        at = b;
    }
    if at < buf.len() {
        evs.push(Event::Text(buf[at..].to_string().into()));
    }
}

fn render_md(content: &str, notes: &[String]) -> String {
    render_with(content, notes, &[], false)
}

/// S1 (docs/security-review.md): scheme of a markdown link/image target,
/// lowercased, or None when there is none (relative / fragment / bare word).
fn url_scheme(url: &str) -> Option<String> {
    let i = url.find(':')?;
    let s = &url[..i];
    let mut cs = s.chars();
    let ok = cs.next().is_some_and(|c| c.is_ascii_alphabetic())
        && cs.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    ok.then(|| s.to_ascii_lowercase())
}

/// S1: only these schemes survive as `<a class="ext">`; everything else
/// (javascript:, data:, file:, vbscript:, relative paths — the webview would
/// navigate the app window itself) is rendered as literal text.
///
/// R29.10: `image` is ALWAYS false-ing. No scheme whatsoever mints an `<img>`
/// — a note must not be able to cause a network fetch, so the renderer refuses
/// to mint a remote src at all and the CSP's `img-src` is only the backstop.
/// Two independent layers: widening one of them must not open the door.
/// A schemed image target takes `image_html`'s banner branch instead (R29.4).
fn ext_ok(url: &str, image: bool) -> bool {
    if image {
        return false;
    }
    matches!(url_scheme(url).as_deref(), Some("http" | "https" | "mailto"))
}

/// R29.10: is this image target REMOTE — i.e. would rendering it fetch bytes
/// over the network? http(s) only; `file:`/`data:`/`javascript:` are S1's
/// business (literal text) and are left exactly where they were.
fn is_remote_img(url: &str) -> bool {
    matches!(url_scheme(url).as_deref(), Some("http" | "https"))
}

/// reading=true: heading links join with " > " (R10.1). Block ids are emitted
/// as span.blockid in both modes; CSS decides visibility.
fn render_with(content: &str, notes: &[String], imgs: &[String], reading: bool) -> String {
    // Security (docs/security-review.md H1): .md files are untrusted, so raw
    // HTML events are demoted to text (push_html escapes Text). Wikilinks are
    // linkified at the EVENT level — label and data-note attr escaped — so our
    // anchors are the only HTML that survives. Code blocks/spans untouched.
    // NB: pulldown emits "[" "[" "Ideas" "]" "]" as SEPARATE Text events, so
    // consecutive text is coalesced in `buf` before the wikilink scan.
    let mut evs: Vec<Event> = Vec::new();
    let mut buf = String::new();
    let mut in_code = false;
    let mut demoted: Vec<Option<String>> = Vec::new(); // S1: per open link/image, Some(text tail) when rendered as text
    // R29.2: Some((target, alt so far)) while inside a vault-relative image
    let mut img_alt: Option<(String, String)> = None;
    // pulldown's own ENABLE_WIKILINKS would consume [[..]] before our pass;
    // SMART_PUNCTUATION would curl quotes/apostrophes inside link targets,
    // breaking [[name]] -> filename fidelity
    let mut opts = Options::all();
    opts.remove(Options::ENABLE_WIKILINKS);
    opts.remove(Options::ENABLE_SMART_PUNCTUATION);
    for ev in Parser::new_ext(content, opts) {
        // inside `![alt](rel.png)`: swallow the inner events, keep their text as
        // the alt attribute. Nothing here reaches linkify — an alt is not a place
        // where a wikilink or a tag renders, in either engine.
        if let Some((_, alt)) = img_alt.as_mut() {
            match ev {
                Event::Text(t) | Event::Code(t) => alt.push_str(&t),
                Event::End(TagEnd::Image) => {
                    let (target, alt) = img_alt.take().expect("img_alt set");
                    evs.push(Event::Html(image_html(imgs, &target, &alt).into()));
                }
                _ => {}
            }
            continue;
        }
        match ev {
            // demoted raw html + plain text both join the scan buffer
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) if !in_code => {
                buf.push_str(&t)
            }
            other => {
                if !buf.is_empty() {
                    // R10.3: ` ^id` closing a paragraph / list item / heading
                    let block_end = matches!(
                        other,
                        Event::End(TagEnd::Paragraph | TagEnd::Item | TagEnd::Heading(_))
                    );
                    match if block_end { split_block_id(&buf) } else { None } {
                        Some((text, id)) => {
                            linkify(text, notes, imgs, reading, &mut evs);
                            let id = esc(id);
                            evs.push(Event::Html(
                                format!("<span class=\"blockid\" data-bid=\"{id}\">^{id}</span>").into(),
                            ));
                        }
                        None => linkify(&buf, notes, imgs, reading, &mut evs),
                    }
                    buf.clear();
                }
                match other {
                    Event::Start(Tag::CodeBlock(_) | Tag::MetadataBlock(_)) => in_code = true,
                    Event::End(TagEnd::CodeBlock | TagEnd::MetadataBlock(_)) => in_code = false,
                    // S1: standard links / images — scheme allowlist. Allowed link ->
                    // our own anchor (class ext, rel noopener; main.js routes clicks to
                    // open_external). Allowed image -> pulldown's own <img> (src escaped
                    // by push_html). Anything else -> the literal `[label](url)` as TEXT.
                    Event::Start(Tag::Link { link_type, dest_url, title, .. }) => {
                        if ext_ok(&dest_url, false) {
                            let t = if title.is_empty() { String::new() } else { format!(" title=\"{}\"", esc(&title)) };
                            evs.push(Event::Html(format!("<a href=\"{}\" class=\"ext\" rel=\"noopener\"{t}>", esc(&dest_url)).into()));
                            demoted.push(None);
                        } else {
                            let auto = matches!(link_type, LinkType::Autolink | LinkType::Email);
                            if !auto { evs.push(Event::Text("[".into())); }
                            demoted.push(Some(if auto { String::new() } else { format!("]({dest_url})") }));
                        }
                        continue;
                    }
                    Event::End(TagEnd::Link) => {
                        match demoted.pop().flatten() {
                            Some(tail) => { if !tail.is_empty() { evs.push(Event::Text(tail.into())); } }
                            None => evs.push(Event::Html("</a>".into())),
                        }
                        continue;
                    }
                    // R29.2: a target with NO scheme is a vault-relative image.
                    // Its alt text arrives as the events BETWEEN Start and End,
                    // so capture them (img_alt) instead of letting them reach the
                    // wikilink scan, and emit our own <img> at the End.
                    Event::Start(Tag::Image { ref dest_url, .. })
                        if url_scheme(dest_url).is_none() =>
                    {
                        img_alt = Some((dest_url.to_string(), String::new()));
                        continue;
                    }
                    Event::Start(Tag::Image { ref dest_url, .. }) => {
                        if ext_ok(dest_url, true) {
                            // DEAD in a correct build: `ext_ok(_, true)` is false for
                            // every scheme (R29.10). This is the branch that would mint
                            // pulldown's own `<img src="https://…">` — it is what
                            // docs/negctl-remote-img/ restores, and the moment it lives
                            // the note is a beacon. Kept, and only kept, so the control
                            // has something to turn red.
                            demoted.push(None);
                        } else if is_remote_img(dest_url) {
                            // R29.10: http(s) image -> R29.4's banner, same as a missing
                            // local target. No <img>, and the URL never reaches the DOM.
                            img_alt = Some((dest_url.to_string(), String::new()));
                            continue;
                        } else {
                            // S1 unchanged: any OTHER scheme (file:, data:, javascript:)
                            // stays literal text — `imgsec`'s ZJ-File asserts exactly that.
                            evs.push(Event::Text("![".into()));
                            demoted.push(Some(format!("]({dest_url})")));
                            continue;
                        }
                    }
                    Event::End(TagEnd::Image) => {
                        if let Some(tail) = demoted.pop().flatten() {
                            evs.push(Event::Text(tail.into()));
                            continue;
                        }
                    }
                    _ => {}
                }
                evs.push(other);
            }
        }
    }
    if !buf.is_empty() {
        linkify(&buf, notes, imgs, reading, &mut evs);
    }
    let mut out = String::new();
    html::push_html(&mut out, evs.into_iter());
    // R15.11: pulldown-cmark emits "<input .../>\n" for task markers; that newline is a rendered space (~4px) between the
    // custom checkbox box and the text, which stock does not have (box->text = 16px box + margin only).
    out.replace("type=\"checkbox\"/>\n", "type=\"checkbox\"/>").replace("checked=\"\"/>\n", "checked=\"\"/>")
}

/// R35 SCROLL SYNC: the 1-based SOURCE LINE on which each TOP-LEVEL block of
/// `content` begins, in the same order `render` emits its top-level elements.
///
/// This is the mapping the reading view cannot otherwise have: the rendered
/// HTML carries no line information, and a pixel copy between the two
/// scrollers is wrong by construction (the reading document re-flows to a
/// different height — measured at 0.868x the edit document on the scroll
/// fixture, and NOT uniformly, so no scale factor exists either).
///
/// It is derived from the SAME parser, with the SAME options, that produced
/// the HTML — a second, hand-rolled block splitter in JS would drift from the
/// renderer the first time either changed. Depth 0 only: a nested paragraph
/// inside a list item is not a top-level element.
fn block_lines_of(content: &str) -> Vec<u32> {
    let mut opts = Options::all();
    opts.remove(Options::ENABLE_WIKILINKS);
    opts.remove(Options::ENABLE_SMART_PUNCTUATION);
    // byte offset -> 1-based line, walked once in step with the (monotonic) block starts
    let mut nl: Vec<usize> = vec![0];
    for (i, b) in content.bytes().enumerate() {
        if b == b'\n' {
            nl.push(i + 1);
        }
    }
    let line_of = |off: usize| -> u32 {
        match nl.binary_search(&off) {
            Ok(i) => (i + 1) as u32,
            Err(i) => i as u32, // i = count of line starts <= off
        }
    };
    let mut depth = 0i32;
    let mut out = Vec::new();
    for (ev, range) in Parser::new_ext(content, opts).into_offset_iter() {
        match ev {
            // a metadata block (frontmatter) is parsed but renders to NOTHING,
            // so it must not consume an element slot in the map
            Event::Start(Tag::MetadataBlock(_)) => depth += 1,
            Event::End(TagEnd::MetadataBlock(_)) => depth -= 1,
            Event::Start(_) => {
                if depth == 0 {
                    out.push(line_of(range.start));
                }
                depth += 1;
            }
            Event::End(_) => depth -= 1,
            Event::Rule => {
                if depth == 0 {
                    out.push(line_of(range.start));
                }
            }
            _ => {}
        }
    }
    out
}

#[tauri::command]
fn block_lines(content: String) -> Vec<u32> {
    span_timed!("block_lines", block_lines_of(&content))
}

#[tauri::command]
fn render(v: State<Vault>, content: String, otel: Option<perf::Ctx>) -> String {
    // ONE lock for both lists: two `v.index.lock()` calls in one expression is
    // a deadlock on a non-reentrant Mutex, not a style question.
    let ix = v.index.lock().unwrap();
    span_timed!(otel => "render", render_with(&content, ix.names(), ix.images(), true), serde_json::json!({"bytes": content.len()}))
}

/// pure core of render_blocks: every block rendered against the same note list
fn render_blocks_with(blocks: &[String], notes: &[String], imgs: &[String]) -> Vec<String> {
    blocks.iter().map(|b| render_with(b, notes, imgs, false)).collect()
}

/* perf-lp: live preview renders every block of a note per caret move. One
   IPC round-trip + one note-list lookup for the whole batch instead of
   ~150 render calls each re-walking the vault. */
#[tauri::command]
fn render_blocks(v: State<Vault>, blocks: Vec<String>, otel: Option<perf::Ctx>) -> Vec<String> {
    let n = blocks.len();
    let ix = v.index.lock().unwrap();
    span_timed!(otel => "render_blocks", render_blocks_with(&blocks, ix.names(), ix.images()), serde_json::json!({"blocks": n}))
}

/// R12 source mode: lp rows with every marker revealed (src/srcmode.rs)
#[tauri::command]
fn highlight_blocks(blocks: Vec<String>, otel: Option<perf::Ctx>) -> Vec<String> {
    let n = blocks.len();
    span_timed!(otel => "highlight_blocks", blocks.iter().map(|b| srcmode::highlight_block(b)).collect(), serde_json::json!({"blocks": n}))
}

/// tags: per-note tag list and vault-wide tag -> note count (BTreeMap keeps
/// the JSON object sorted by tag; the UI re-sorts by count)
#[tauri::command]
fn tags(v: State<Vault>, name: String) -> Vec<String> {
    span_timed!("tags", v.index.lock().unwrap().tags(&name).to_vec())
}

#[tauri::command]
fn tag_counts(v: State<Vault>) -> std::collections::BTreeMap<String, usize> {
    span_timed!("tag_counts", v.index.lock().unwrap().tag_counts())
}

#[tauri::command]
fn backlinks(v: State<Vault>, name: String, otel: Option<perf::Ctx>) -> Vec<String> {
    span_timed!(otel => "backlinks", backlinks_inner(&v, &name))
}

fn backlinks_inner(v: &State<Vault>, name: &str) -> Vec<String> {
    // perf-index: inverted edges are maintained in the index — O(1) lookup,
    // no vault read (was: re-read every note per call, 89ms @500 files)
    v.index.lock().unwrap().backlinks(name)
}

/* rsidebar: right sidebar panes served from the index (zero disk reads) */
#[tauri::command]
fn outline(v: State<Vault>, name: String, otel: Option<perf::Ctx>) -> Vec<outline::Heading> {
    span_timed!(otel => "outline", outline::parse(v.index.lock().unwrap().content(&name).unwrap_or("")))
}

#[derive(serde::Serialize)]
struct OutLink {
    text: String,
    /// resolved note name, None = unresolved (ghost)
    target: Option<String>,
}

/// Outgoing links pane: the note's [[links]] in source order, deduped
#[tauri::command]
fn outgoing(v: State<Vault>, name: String) -> Vec<OutLink> {
    span_timed!("outgoing", {
        let ix = v.index.lock().unwrap();
        let names = ix.names();
        let mut out: Vec<OutLink> = Vec::new();
        for l in ix.links(&name) {
            if out.iter().any(|o| o.text == *l) {
                continue;
            }
            out.push(OutLink { text: l.clone(), target: resolve(names, l).map(|j| names[j].clone()) });
        }
        out
    })
}

#[derive(serde::Serialize)]
struct BacklinkCtx {
    note: String,
    /// (0-based line, trimmed line text) for every line linking to the note
    lines: Vec<(u32, String)>,
}

/// Backlinks pane: linking notes + the lines that carry the link
#[tauri::command]
fn backlinks_ctx(v: State<Vault>, name: String) -> Vec<BacklinkCtx> {
    span_timed!("backlinks_ctx", {
        let ix = v.index.lock().unwrap();
        let names = ix.names();
        ix.backlinks(&name)
            .into_iter()
            .map(|src| {
                let lines = ix
                    .content(&src)
                    .unwrap_or("")
                    .lines()
                    .enumerate()
                    .filter(|(_, l)| links_in(l).iter().any(|k| resolve(names, k) == resolve(names, &name)))
                    .map(|(i, l)| (i as u32, l.trim().chars().take(200).collect()))
                    .collect();
                BacklinkCtx { note: src, lines }
            })
            .collect()
    })
}

#[derive(serde::Serialize)]
struct Mention {
    note: String,
    line: u32,
    col: u32,
    len: u32,
    /// the mention's line, trimmed to 200 chars for the pane
    text: String,
}

/// R10.5 Backlinks pane "Unlinked mentions": every other note's plain-text
/// occurrences of this note's basename (case-insensitive), from the index
#[tauri::command]
fn unlinked_mentions(v: State<Vault>, name: String) -> Vec<Mention> {
    span_timed!("unlinked_mentions", {
        let ix = v.index.lock().unwrap();
        let base = name.rsplit('/').next().unwrap_or(&name);
        let mut out = Vec::new();
        for (n, c, _) in ix.docs() {
            if n == name {
                continue;
            }
            for (line, col, len) in index::mentions_in(c, base) {
                let text = c.lines().nth(line as usize).unwrap_or("").trim().chars().take(200).collect();
                out.push(Mention { note: n.to_string(), line, col, len, text });
            }
        }
        out
    })
}

/// Link button: wrap the matched text in [[ ]] in `note` and save it
#[tauri::command]
fn link_mention(v: State<Vault>, note: String, target: String, line: u32, col: u32, len: u32) -> Result<(), String> {
    span_timed!("link_mention", {
        let root = cur_vault(&v).ok_or("no vault open")?;
        let mut ix = v.index.lock().unwrap();
        link_mention_in(&root, &mut ix, &note, &target, line, col, len)
    })
}

/* F1 dataloss: the pure core, so the ERROR path is testable without Tauri.
   This call site used to drop the write's Result on the floor and return
   Ok(()) — a refused write (ENOSPC, EROFS, the sandbox) was reported to the
   user as a saved one and the edit was gone. The Result IS the product. */
fn link_mention_in(
    root: &Path,
    ix: &mut Index,
    note: &str,
    target: &str,
    line: u32,
    col: u32,
    len: u32,
) -> Result<(), String> {
    let base = target.rsplit('/').next().unwrap_or(target);
    let nc = {
        let c = ix.content(note).ok_or("no such note")?;
        index::link_mention(c, base, line, col, len).ok_or("mention moved — refresh the pane")?
    };
    write_note_in(root, ix, note, &nc)
}

/// active right-sidebar tab, persisted as rside_tab in ~/.rustidian.json
#[tauri::command]
fn get_rside_tab() -> Option<String> {
    cfg_value()["rside_tab"].as_str().map(str::to_string)
}

#[tauri::command]
fn set_rside_tab(tab: String) {
    let mut v = cfg_value();
    v["rside_tab"] = serde_json::json!(tab);
    let _ = fs::write(cfg_path(), v.to_string());
}

/* THEME: the chosen mode, persisted as "theme" in ~/.rustidian.json through the
   SAME store as every other ui preference (cfg_value/cfg_path — no new store, no
   new crate). ABSENT is a third state and it is the important one: absent means
   "the user never chose", and the frontend then follows the system signal
   (prefers-color-scheme; notes/theme/detect-recon.md). So absent != "dark" — a
   Rust-side default here would be a hardcoded theme wearing a system-detection
   costume, which is exactly what item 8 rejected the tauri theme API for. */
#[tauri::command]
fn get_theme() -> Option<String> {
    match cfg_value()["theme"].as_str() {
        Some(t @ ("dark" | "light")) => Some(t.to_string()),
        _ => None, // absent, or a hand-edited junk value = no stored choice
    }
}

#[tauri::command]
fn set_theme(theme: String) {
    if theme != "dark" && theme != "light" {
        return; // never let a typo'd mode into the file: it would read back as "no choice"
    }
    let mut v = cfg_value();
    v["theme"] = serde_json::json!(theme);
    let _ = fs::write(cfg_path(), v.to_string());
}

/* THE SECOND THEME AXIS: the PALETTE, persisted as "palette" in
   ~/.rustidian.json. It is deliberately NOT the "theme" key above.

   "theme" is the MODE (dark|light) and it has a meaning this feature must not
   take away: absent = the user has chosen no mode, so prefers-color-scheme
   decides. Storing a palette name there would make "no mode chosen" and "the
   1984 palette" the same state, and the system default would stop working the
   moment anyone picked a palette. Two axes, two keys, and they compose: the
   palette selects WHICH set of colours, the mode selects that set's variant.

   VALIDATION is the same rule set_theme follows, for the same reason — a name
   we do not ship must never reach the file, because on the next boot it would
   read back as a deliberate choice and the user would be pinned to a palette
   that does not exist. See palette::is_known. */
#[tauri::command]
fn get_palette() -> Option<String> {
    match cfg_value()["palette"].as_str() {
        Some(p) if palette::is_known(p) && p != palette::DEFAULT_PALETTE => Some(p.to_string()),
        // absent, hand-edited junk, or the default written by an older build:
        // all three mean "nothing to apply", and the frontend paints the default.
        _ => None,
    }
}

#[tauri::command]
fn set_palette(palette: String) {
    if !palette::is_known(&palette) {
        return; // refused: never written, so it can never read back as a choice
    }
    let mut v = cfg_value();
    if palette == palette::DEFAULT_PALETTE {
        // choosing the default is choosing the ABSENCE of a palette — the key is
        // removed, not set to "default", so the stored shape of "I picked the
        // default" and "I never picked" stay the same one state.
        if let Some(o) = v.as_object_mut() {
            o.remove("palette");
        }
    } else {
        v["palette"] = serde_json::json!(palette);
    }
    let _ = fs::write(cfg_path(), v.to_string());
}

/* ---- themefs R3 (snippets): thin commands over src-tauri/src/themefs.rs ----
   The vault-CSS axis lives in the VAULT's own .obsidian/appearance.json
   (stock's file, byte-wise round-trip — themefs.rs), never in ~/.rustidian.json:
   pointing rustidian at a vault must find the choice Obsidian already made.
   Every refusal string is user-visible (the frontend puts it on the notice
   banner) and names the file — R6: loud where stock is silent. */

/// what the frontend injects for one snippet or theme: the sanitized bytes
/// plus the R4X.4 strip message when mask declarations were cut (None = none).
/// `bridge` is the item-8 R4 alias sheet (themefs::bridge_css), GENERATED from
/// the theme's sanitized css — themes only (None for snippets: one bridge per
/// PAINTING theme, snippets compose on top and never re-alias the chrome).
#[derive(serde::Serialize)]
struct VaultCss {
    css: String,
    message: Option<String>,
    bridge: Option<String>,
}

#[tauri::command]
fn snippets_scan(v: State<Vault>, otel: Option<perf::Ctx>) -> Result<Vec<String>, String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    Ok(span_timed!(otel => "snippets_scan", themefs::list_snippets(&root)))
}

#[tauri::command]
fn snippets_enabled(v: State<Vault>) -> Result<Vec<String>, String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    Ok(themefs::enabled_snippets(&root))
}

#[tauri::command]
fn snippet_css(v: State<Vault>, label: String, otel: Option<perf::Ctx>) -> Result<VaultCss, String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    let (css, message) =
        span_timed!(otel => "snippet_css", themefs::load_snippet(&root, &label))?;
    Ok(VaultCss { css, message, bridge: None })
}

#[tauri::command]
fn set_snippet_enabled(v: State<Vault>, label: String, on: bool) -> Result<(), String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    themefs::set_snippet_enabled(&root, &label, on)
}

/* ---- themefs R5 (themes): the listing predicate is the oracle's, verbatim
   (probe-stock-vault.sh §3 — themefs::themes_scan). themes_scan/theme_css
   are INSTRUMENTED (a directory walk that grows with the user's installed
   themes; a user-sized CSS file through the R4X.4 sanitizer); get_css_theme/
   set_css_theme are the get_theme class homed in vault config — one scalar in
   the small appearance.json (byte-wise round-trip), OUT_OF_SCOPE_CMD with
   reasons in perf-coverage.sh. */

#[tauri::command]
fn themes_scan(v: State<Vault>, otel: Option<perf::Ctx>) -> Result<themefs::ThemesScan, String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    Ok(span_timed!(otel => "themes_scan", themefs::themes_scan(&root)))
}

#[tauri::command]
fn theme_css(v: State<Vault>, name: String, otel: Option<perf::Ctx>) -> Result<VaultCss, String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    let (css, message) =
        span_timed!(otel => "theme_css", themefs::load_theme(&root, &name))?;
    // item 8: the R4 alias bridge rides the same response — generated from
    // the SAME sanitized bytes the frontend is about to inject, so the bridge
    // can never describe a different file than the one painting
    let bridge = Some(themefs::bridge_css(&css));
    Ok(VaultCss { css, message, bridge })
}

#[tauri::command]
fn get_css_theme(v: State<Vault>) -> Result<String, String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    Ok(themefs::css_theme(&root))
}

#[tauri::command]
fn set_css_theme(v: State<Vault>, name: String) -> Result<(), String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    themefs::set_css_theme(&root, &name)
}

/* ---- themefs item 6: hot reload (DESIGN §8, criterion 4) ----------------
   The R11 vault watcher ticks at 1000 ms — it cannot meet stock's measured
   0.14–0.34 s repaint bar (T4) — so the APPLIED vault CSS gets its own
   poller: at most the painting theme.css + the enabled snippet files
   (themefs::watch_set), one (mtime, len) stat each per RELOAD_TICK_MS,
   ALIVE ONLY while that set is non-empty. On a fingerprint move it emits
   `vault-css-changed` {kind, name}; the frontend re-reads through the same
   sanitizing commands (theme_css / snippet_css) and re-injects that ONE
   element. Nothing on this path writes — criterion 4 asserts
   appearance.json's bytes across an edit — and NEW files are not live
   (stock's asymmetry): the set is re-derived only when vault_css_watch says
   the applied state moved, never by scanning on a tick.

   vault_css_watch is the frontend declaring what is APPLIED — it owns the
   listed/painting decision (an unlisted cssTheme paints Default and must
   not be watched), so the backend does not re-guess it. The command itself
   touches memory only (OUT_OF_SCOPE_CMD, reason in perf-coverage.sh); the
   derivation and the stats happen on the poller thread. */
struct CssReloadCfg {
    theme: String,
    snippets: Vec<String>,
    gen: u64,
    alive: bool,
}
static CSS_RELOAD: Mutex<CssReloadCfg> = Mutex::new(CssReloadCfg {
    theme: String::new(),
    snippets: Vec::new(),
    gen: 0,
    alive: false,
});

#[tauri::command]
fn vault_css_watch(
    app: tauri::AppHandle,
    v: State<Vault>,
    theme: String,
    snippets: Vec<String>,
) -> Result<(), String> {
    let have_vault = cur_vault(&v).is_some();
    let mut st = CSS_RELOAD.lock().unwrap();
    st.theme = theme;
    st.snippets = snippets;
    st.gen += 1;
    // spawn-on-demand, under the SAME lock the poller dies under: while
    // anything is applied a poller exists, and never two of them
    if have_vault && !(st.theme.is_empty() && st.snippets.is_empty()) && !st.alive {
        st.alive = true;
        spawn_css_reload(app);
    }
    Ok(())
}

fn spawn_css_reload(app: tauri::AppHandle) {
    use tauri::{Emitter, Manager};
    std::thread::spawn(move || {
        let mut gen = 0u64; // != any bumped gen, so the first tick derives
        let mut watched: Vec<(themefs::WatchedFile, themefs::ReloadFp)> = Vec::new();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(themefs::RELOAD_TICK_MS));
            let (g, theme, snippets) = {
                let st = CSS_RELOAD.lock().unwrap();
                (st.gen, st.theme.clone(), st.snippets.clone())
            };
            if g != gen {
                gen = g;
                // a config move is a USER action the frontend already
                // applied — reseed the fingerprints silently, emit nothing
                let root = cur_vault(&app.state::<Vault>());
                watched = match &root {
                    Some(r) => themefs::watch_set(r, &theme, &snippets)
                        .into_iter()
                        .map(|w| {
                            let fp = themefs::reload_fp(&w.path);
                            (w, fp)
                        })
                        .collect(),
                    None => Vec::new(),
                };
            }
            if watched.is_empty() {
                // nothing to watch: die — UNLESS the config moved again
                // while we looked (checked under the spawn lock above)
                let mut st = CSS_RELOAD.lock().unwrap();
                if st.gen == gen {
                    st.alive = false;
                    return;
                }
                continue;
            }
            for (w, fp) in watched.iter_mut() {
                let now = themefs::reload_fp(&w.path);
                if now != *fp {
                    *fp = now;
                    let _ = app.emit("vault-css-changed", &*w);
                }
            }
        }
    });
}

/* R14: custom hotkeys, persisted as "hotkeys" in ~/.rustidian.json in the stock
   Obsidian shape {"<cmd id>":[{"modifiers":["Mod","Shift"],"key":"G"}]}:
   [] = default removed, absent id = stock default. The frontend registry
   (ui/main.js CMDS) is the single source of truth for ids + defaults. */
#[tauri::command]
fn get_hotkeys() -> serde_json::Value {
    hotkeys_clean(cfg_value()["hotkeys"].clone())
}

#[tauri::command]
fn set_hotkeys(map: serde_json::Value) {
    let mut v = cfg_value();
    v["hotkeys"] = hotkeys_clean(map);
    let _ = fs::write(cfg_path(), v.to_string());
}

/// keep only well-formed entries (id -> [{modifiers:[str], key:str}]) so a
/// hand-edited config can never wedge the dispatcher; [] survives (= removed)
fn hotkeys_clean(map: serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    if let Some(o) = map.as_object() {
        for (id, chords) in o {
            let Some(arr) = chords.as_array() else { continue };
            let ok: Vec<serde_json::Value> = arr
                .iter()
                .filter(|c| {
                    c["key"].as_str().map_or(false, |k| !k.is_empty())
                        && c["modifiers"].as_array().map_or(false, |m| m.iter().all(|x| x.is_string()))
                })
                .cloned()
                .collect();
            if ok.len() == arr.len() {
                out.insert(id.clone(), serde_json::Value::Array(ok));
            }
        }
    }
    serde_json::Value::Object(out)
}

#[tauri::command]
fn graph(v: State<Vault>, otel: Option<perf::Ctx>) -> Graph {
    span_timed!(otel => "graph", graph_inner(&v), serde_json::json!({}))
}

fn graph_inner(v: &State<Vault>) -> Graph {
    // R19: served from the index graph cache (built once per edge change)
    v.index.lock().unwrap().graph().graph.clone()
}

/// R19 (feedback #5): local-graph neighbourhood cut in Rust from the cached
/// adjacency — the UI no longer pulls the whole vault graph per recentre.
#[tauri::command]
fn graph_local(v: State<Vault>, center: String, depth: usize, inc: bool, out: bool, otel: Option<perf::Ctx>) -> Graph {
    span_timed!(otel => "graph_fetch",
        v.index.lock().unwrap().graph().local(&center, depth.min(5), inc, out),
        serde_json::json!({ "center": center, "depth": depth }))
}

#[derive(serde::Serialize, Debug, PartialEq)]
struct SearchHit {
    note: String,
    line: u32, // 0-based source line; 0 with snippet==note means a NAME match
    snippet: String,
    /// R25.13m: a hit is an ABSOLUTE OFFSET into the file AS INDEXED, not a
    /// line number — measured against stock in docs/recon-srclick/README.md
    /// C12, where deleting lines above a match moved the jump by exactly the
    /// characters removed and never re-found the text. The frontend jumps by
    /// this, so it must be in the SAME unit a JS string is indexed in: UTF-16
    /// CODE UNITS, not bytes and not chars. `"é".length === 1` but 2 bytes;
    /// `"𝄞".length === 2` but 1 char — a byte offset would land mid-character
    /// and a char offset would drift one unit per astral character.
    offset: u32,
    /// match length in the same unit; 0 = nothing to highlight (a NAME match,
    /// or a tag-only line hit, which points at a line, not at a span).
    len: u32,
}

/// UTF-16 code units in `s` — the unit `offset`/`len` are counted in, so that
/// `editor.value.slice(offset, offset + len)` on the frontend is the match.
fn u16len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

/// R9.4: case-insensitive substring over note names + bodies. Hits ordered by
/// note (docs arrive sorted) then line; snippet = the matching line trimmed to
/// ~200 chars around the first hit; capped at 500 hits total.
/// query grammar: whitespace tokens; `tag:foo` / `tag:#foo` tokens restrict
/// hits to notes carrying that tag (nested match by prefix: tag:a hits a/b;
/// case-insensitive); the remaining tokens are the substring query. A
/// tag-only query yields the lines holding the #tag, else one name hit.
fn split_query(query: &str) -> (Vec<String>, String) {
    let (mut tags, mut text) = (Vec::new(), Vec::new());
    for tok in query.split_whitespace() {
        match tok.strip_prefix("tag:") {
            Some(t) => {
                let t = t.trim_start_matches('#').trim_end_matches('/').to_lowercase();
                if !t.is_empty() {
                    tags.push(t);
                }
            }
            None => text.push(tok),
        }
    }
    (tags, text.join(" "))
}

fn has_tag(tags: &[String], want: &str) -> bool {
    tags.iter().any(|t| {
        let t = t.to_lowercase();
        t == want || (t.starts_with(want) && t[want.len()..].starts_with('/'))
    })
}

/// `content.lines()` paired with each line's absolute UTF-16 offset in
/// `content`. Reproduces `lines()` EXACTLY — `split_inclusive('\n')` yields
/// the same pieces (no phantom trailing line when the file ends in \n, nothing
/// for an empty file) and the same text once the terminator is stripped, \r\n
/// included — while counting the terminators `lines()` throws away, because
/// R25.13m's offset is into the FILE, not into the line.
fn lines_with_offsets(content: &str) -> impl Iterator<Item = (usize, &str, u32)> {
    let mut base: u32 = 0;
    content.split_inclusive('\n').enumerate().map(move |(i, raw)| {
        let here = base;
        base += u16len(raw);
        let l = raw.strip_suffix('\n').unwrap_or(raw);
        (i, l.strip_suffix('\r').unwrap_or(l), here)
    })
}

fn search_docs<'a>(docs: impl IntoIterator<Item = (&'a str, &'a str, &'a [String])>, query: &str) -> Vec<SearchHit> {
    let (want, text) = split_query(query);
    let q = text.to_lowercase();
    let mut out = Vec::new();
    if q.is_empty() && want.is_empty() {
        return out;
    }
    'docs: for (name, content, tags) in docs {
        if !want.iter().all(|w| has_tag(tags, w)) {
            continue;
        }
        if q.is_empty() {
            // tag-only: show the lines carrying the tag inline (frontmatter-
            // only notes get a name hit so they still show up)
            let n0 = out.len();
            for (i, l, base) in lines_with_offsets(content) {
                let lower = l.to_lowercase();
                if tag_spans(&lower).iter().any(|&(a, b)| want.iter().any(|w| has_tag(&[lower[a + 1..b].to_string()], w))) {
                    // a tag-only hit points at a LINE, not at a span: offset =
                    // the line start, len = 0, so R25.13d paints nothing and
                    // the jump still lands on the line. Pointing at the matched
                    // #tag span itself is LATER — unmeasured (stock has no
                    // tag: grammar to measure against), never guessed.
                    out.push(SearchHit { note: name.to_string(), line: i as u32, snippet: l.trim().chars().take(200).collect(), offset: base, len: 0 });
                }
            }
            if out.len() == n0 {
                out.push(SearchHit { note: name.to_string(), line: 0, snippet: name.to_string(), offset: 0, len: 0 });
            }
            if out.len() >= 500 {
                break;
            }
            continue;
        }
        if name.to_lowercase().contains(&q) {
            // a NAME match has no span in the body: offset 0, len 0 — open the
            // note at the top, highlight nothing (R25.13a/d).
            out.push(SearchHit { note: name.to_string(), line: 0, snippet: name.to_string(), offset: 0, len: 0 });
        }
        for (i, l, base) in lines_with_offsets(content) {
            let lower = l.to_lowercase();
            let Some(bpos) = lower.find(&q) else { continue };
            let t = l.trim();
            // cpos: the match start as a CHAR index into the original line.
            // `bpos` is a byte index into the LOWERCASED line, so this inherits
            // the assumption the snippet window already makes — that
            // to_lowercase() preserves char COUNT. It does for every script
            // this app is shipped with; the pathological cases (İ -> i̇, one
            // char to two) would shift the highlight within the line, never
            // outside it, and never panic: the slicing below is by char index
            // into a Vec<char>, clamped to its own length.
            let chars: Vec<char> = l.chars().collect();
            let cpos = lower[..bpos].chars().count().min(chars.len());
            let qlen = q.chars().count().min(chars.len() - cpos);
            let snippet = if t.len() <= 200 {
                t.to_string()
            } else {
                // char-safe ~200-char window around the first hit
                let start = cpos.saturating_sub(80).min(chars.len());
                let end = (cpos + 120).min(chars.len());
                chars[start..end].iter().collect()
            };
            // R25.13m: absolute UTF-16 offset of the match in the FILE, and its
            // length, so the frontend jumps by the offset the backend found
            // instead of re-finding the match itself — two searches that can
            // disagree is exactly what the brief forbids.
            let in_line: String = chars[..cpos].iter().collect();
            let matched: String = chars[cpos..cpos + qlen].iter().collect();
            out.push(SearchHit {
                note: name.to_string(),
                line: i as u32,
                snippet,
                offset: base + u16len(&in_line),
                len: u16len(&matched),
            });
            if out.len() >= 500 {
                break 'docs;
            }
        }
        if out.len() >= 500 {
            break;
        }
    }
    out
}

#[tauri::command]
fn search(v: State<Vault>, query: String, otel: Option<perf::Ctx>) -> Vec<SearchHit> {
    span_timed!(otel => "search", search_inner(&v, &query))
}

fn search_inner(v: &State<Vault>, query: &str) -> Vec<SearchHit> {
    // perf-index: contents are resident; no per-call vault read
    search_docs(v.index.lock().unwrap().docs(), query)
}

/* R9.4 bookmarks live in vault/.obsidian/bookmarks.json — Obsidian's OWN
   file, read and written in stock 1.13.7's schema, because rustidian is a
   drop-in replacement (operator decision 2026-09-19, goal bmcompat). The
   shape is MEASURED, not remembered — docs/recon-bmcompat/README.md, every
   claim named after its capture:

     { "items": [ ... ] }                          top level: an OBJECT
     { "type": "file",  "ctime": N, "path": "Projects/Roadmap.md" }
     { "type": "file",  ..., "title": "Zet" }      title LAST, only when typed
     { "type": "group", "ctime": N, "items": [...], "title": "Work" }

   2-space indent, NO trailing newline (last byte `}`), key order per TYPE,
   nesting is `items` and nothing else. Stock's `path` is our R9.4 name +
   ".md" (recon §6): the name stays the model, the ".md" is spelled only at
   this file boundary. A file entry whose path is NOT *.md, and any entry of
   a type we do not model (stock's `search`, a future 1.14 type) rides
   through read -> edit -> write as an OPAQUE value, content-equal. UNKNOWN
   KEYS ROUND-TRIP (criterion 4): stock itself preserves per-entry keys it
   does not recognise and drops top-level ones (recon §5, 32-editdone); we
   preserve BOTH — never the app that lost someone's future 1.14 key.

   The old v0.12/v1 `.rustidian-bookmarks` is IGNORED: never read, never
   written, never deleted (criterion 5's one sentence, as behaviour —
   r4x_a_pre_existing_rustidian_bookmarks_file_is_ignored). */
const BM_FILE: &str = ".obsidian/bookmarks.json";
/// stock's default for a freshly created group, MEASURED, not remembered
/// (docs/recon-bmfolder/README.md, `03-newgroup.png`).
const BM_NEW_GROUP: &str = "Untitled group";

/// what a node carries that our model does not AUTHOR: stock's ctime
/// (preserved verbatim; minted only for nodes we create) and every key a
/// newer Obsidian wrote that we do not know — kept in FILE order
/// (serde_json's preserve_order) and re-emitted after the keys we do author,
/// which is where stock itself re-serialises them (32-editdone).
#[derive(Debug, Clone, PartialEq, Default)]
struct BmExtra {
    ctime: Option<serde_json::Number>,
    keys: Vec<(String, serde_json::Value)>,
}

impl BmExtra {
    /// a node we create gets its ctime exactly as stock mints one: unix
    /// millis (26-moved.bookmarks.json).
    fn now() -> Self {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        BmExtra { ctime: Some(serde_json::Number::from(ms)), keys: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum BmNode {
    /// a bookmarked note: `name` is the R9.4 vault-relative NAME; on disk it
    /// is stock's `path` = name + ".md". `title` is stock's optional display
    /// alias — we never author one, we never lose one.
    File { name: String, title: Option<String>, x: BmExtra },
    Group { title: String, items: Vec<BmNode>, x: BmExtra },
    /// an entry our model cannot express — stock's `search`, a non-.md path,
    /// a future type. Not painted, not addressable, written back verbatim.
    Opaque(serde_json::Value),
}

impl BmNode {
    fn file(name: impl Into<String>) -> Self {
        BmNode::File { name: name.into(), title: None, x: BmExtra::now() }
    }
    fn group(title: impl Into<String>, items: Vec<BmNode>) -> Self {
        BmNode::Group { title: title.into(), items, x: BmExtra::now() }
    }
}

/* one PAINTED ROW. The pane renders this vector top to bottom and indents by
   `depth`, so the UI never walks a tree and cannot invent an order the file
   does not have; `kind` is "f" or "g" and R4X.5 `[bmt:]` publishes exactly
   this pair per row. */
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct BmRow {
    kind: String,
    depth: usize,
    name: String,
    /// the TEXT the row paints — stock's measured rule (R4X.17, recon-bmcompat
    /// §5 shot 30-afterinject.png): a file row is labelled by its `title` when
    /// it has one, by the basename of the name when it has none; a group row
    /// by its title. `name` stays the click/open key; `label` is only paint.
    label: String,
}

/* the ONE parser, and it is TOLERANT by construction: a body that is not
   JSON, or whose `items` is not an array, reads as the EMPTY tree; an entry
   we cannot model reads as Opaque and is CARRIED, never dropped. There is no
   version branch — the only format this parser has ever read is stock's. */
fn parse_bm_tree(body: &str) -> Vec<BmNode> {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    match v.get("items").and_then(|i| i.as_array()) {
        Some(a) => a.iter().map(bm_node_of).collect(),
        None => Vec::new(),
    }
}

/// one JSON entry -> one node. Keys our model AUTHORS are lifted out; every
/// other key lands in `x.keys` unchanged, in file order. A `ctime`/`title`
/// of the wrong JSON type is NOT lifted — it stays an unknown key, because
/// re-typing someone's value is as much data loss as dropping it.
fn bm_node_of(v: &serde_json::Value) -> BmNode {
    let obj = match v.as_object() {
        Some(o) => o,
        None => return BmNode::Opaque(v.clone()),
    };
    let ctime = obj.get("ctime").and_then(|c| c.as_number()).cloned();
    match obj.get("type").and_then(|t| t.as_str()).unwrap_or("") {
        "file" => {
            // stock's `path` is our name + ".md" (recon §6); any other path
            // (canvas, pdf, bare) is not expressible as an R9.4 name -> opaque
            let name = match obj
                .get("path")
                .and_then(|p| p.as_str())
                .and_then(|p| p.strip_suffix(".md"))
                .filter(|n| !n.is_empty())
            {
                Some(n) => n.to_string(),
                None => return BmNode::Opaque(v.clone()),
            };
            let title = obj.get("title").and_then(|t| t.as_str()).map(str::to_string);
            let keys = obj
                .iter()
                .filter(|(k, _)| {
                    !(k.as_str() == "type"
                        || k.as_str() == "path"
                        || (k.as_str() == "ctime" && ctime.is_some())
                        || (k.as_str() == "title" && title.is_some()))
                })
                .map(|(k, val)| (k.clone(), val.clone()))
                .collect();
            BmNode::File { name, title, x: BmExtra { ctime, keys } }
        }
        "group" => {
            // a group whose title is not a string or whose items is not an
            // array is not ours to reshape: carry it whole
            let title = match obj.get("title").and_then(|t| t.as_str()) {
                Some(t) => t.to_string(),
                None => return BmNode::Opaque(v.clone()),
            };
            let items: Vec<BmNode> = match obj.get("items").and_then(|i| i.as_array()) {
                Some(a) => a.iter().map(bm_node_of).collect(),
                None => return BmNode::Opaque(v.clone()),
            };
            let keys = obj
                .iter()
                .filter(|(k, _)| {
                    !(k.as_str() == "type"
                        || k.as_str() == "items"
                        || k.as_str() == "title"
                        || (k.as_str() == "ctime" && ctime.is_some()))
                })
                .map(|(k, val)| (k.clone(), val.clone()))
                .collect();
            BmNode::Group { title, items, x: BmExtra { ctime, keys } }
        }
        _ => BmNode::Opaque(v.clone()),
    }
}

fn read_bm_tree(root: &Path) -> Vec<BmNode> {
    parse_bm_tree(&fs::read_to_string(root.join(BM_FILE)).unwrap_or_default())
}

/* the ONE serializer. bm_value_of spells the KEY ORDER stock writes — per
   type: file = type,ctime,path[,title]; group = type,ctime,items,title;
   unknown keys AFTER the authored ones, in file order, which is where stock
   itself re-serialises them (32-editdone.bookmarks.json) — and serde_json's
   pretty printer spells the layout stock uses: 2-space indent, ": "
   separator, NO trailing newline (32-editdone.bookmarks.json, last byte `}`
   — od -c verified at fixture-commit time).
   Nothing else may turn a tree into bytes. */
fn bm_value_of(n: &BmNode) -> serde_json::Value {
    use serde_json::{Map, Value};
    match n {
        BmNode::Opaque(v) => v.clone(),
        BmNode::File { name, title, x } => {
            let mut m = Map::new();
            m.insert("type".into(), Value::String("file".into()));
            if let Some(c) = &x.ctime {
                m.insert("ctime".into(), Value::Number(c.clone()));
            }
            m.insert("path".into(), Value::String(format!("{name}.md")));
            if let Some(t) = title {
                m.insert("title".into(), Value::String(t.clone()));
            }
            for (k, v) in &x.keys {
                m.insert(k.clone(), v.clone());
            }
            Value::Object(m)
        }
        BmNode::Group { title, items, x } => {
            let mut m = Map::new();
            m.insert("type".into(), Value::String("group".into()));
            if let Some(c) = &x.ctime {
                m.insert("ctime".into(), Value::Number(c.clone()));
            }
            m.insert("items".into(), Value::Array(items.iter().map(bm_value_of).collect()));
            m.insert("title".into(), Value::String(title.clone()));
            for (k, v) in &x.keys {
                m.insert(k.clone(), v.clone());
            }
            Value::Object(m)
        }
    }
}

fn bm_emit(tree: &[BmNode], top: &[(String, serde_json::Value)]) -> String {
    let mut root = serde_json::Map::new();
    root.insert("items".into(), serde_json::Value::Array(tree.iter().map(bm_value_of).collect()));
    for (k, v) in top {
        if k != "items" {
            root.insert(k.clone(), v.clone());
        }
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(root))
        .unwrap_or_else(|_| "{\n  \"items\": []\n}".to_string())
}

/// the TOP-LEVEL keys we do not author, read back off the current file so a
/// write preserves them. Stock DROPS these (recon §5); we keep them — the
/// strictly more conservative choice — re-emitted after `items`.
fn bm_top_extra(root: &Path) -> Vec<(String, serde_json::Value)> {
    let body = fs::read_to_string(root.join(BM_FILE)).unwrap_or_default();
    match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(serde_json::Value::Object(o)) => o.into_iter().filter(|(k, _)| k != "items").collect(),
        _ => Vec::new(),
    }
}

fn write_bm_tree(root: &Path, tree: &[BmNode]) -> Result<(), String> {
    let top = bm_top_extra(root); // preserved BEFORE the file is replaced
    let body = bm_emit(tree, &top);
    fs::create_dir_all(root.join(".obsidian")).map_err(|e| e.to_string())?;
    fs::write(root.join(BM_FILE), body).map_err(|e| e.to_string())
}

/* DERIVED VIEW: the `f` payloads in pre-order — exactly what list_bookmarks
   has always returned, and what every r9_8_* test asserts against unedited. */
fn bm_names(nodes: &[BmNode], out: &mut Vec<String>) {
    for n in nodes {
        match n {
            BmNode::File { name, .. } => out.push(name.clone()),
            BmNode::Group { items, .. } => bm_names(items, out),
            BmNode::Opaque(_) => {} // not a name — carried, not listed
        }
    }
}

fn read_bookmarks(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    bm_names(&read_bm_tree(root), &mut out);
    out
}

/* the FLAT convenience, for seeds and tests. A FLAT WRITE IS A
   STRUCTURE-DESTROYING WRITE: every group in the file is gone afterwards.
   Production paths that must preserve structure call write_bm_tree — which
   this delegates to, so there is still exactly ONE serializer. */
fn write_bookmarks(root: &Path, list: &[String]) -> Result<(), String> {
    let tree: Vec<BmNode> = list.iter().map(|n| BmNode::file(n.clone())).collect();
    write_bm_tree(root, &tree)
}

fn bm_rows_in(nodes: &[BmNode], depth: usize, out: &mut Vec<BmRow>) {
    for n in nodes {
        match n {
            BmNode::File { name, title, .. } => {
                // R4X.17 (recon-bmcompat §5, 30-afterinject.png): title when
                // typed, else basename minus ".md" — never the full path
                let label = title
                    .clone()
                    .unwrap_or_else(|| name.rsplit('/').next().unwrap_or(name).to_string());
                out.push(BmRow { kind: "f".into(), depth, name: name.clone(), label });
            }
            BmNode::Group { title, items, .. } => {
                out.push(BmRow { kind: "g".into(), depth, name: title.clone(), label: title.clone() });
                bm_rows_in(items, depth + 1, out);
            }
            BmNode::Opaque(_) => {} // preserved on disk, never painted
        }
    }
}

fn bm_rows_of(tree: &[BmNode]) -> Vec<BmRow> {
    let mut out = Vec::new();
    bm_rows_in(tree, 0, &mut out);
    out
}

/* Rows are addressed by INDEX into that pre-order vector, never by title: two
   sibling groups may carry the same title — measured on stock, `05-nest.png` —
   so a title is not a key. bm_path_of turns a row index into the chain of
   child indices that reaches it. */
fn bm_path_of(tree: &[BmNode], ix: usize) -> Option<Vec<usize>> {
    fn walk(nodes: &[BmNode], target: usize, seen: &mut usize, path: &mut Vec<usize>) -> bool {
        for (i, n) in nodes.iter().enumerate() {
            if matches!(n, BmNode::Opaque(_)) {
                continue; // never painted, so never numbered — but `i` still
                          // counts it: paths stay RAW child indices
            }
            path.push(i);
            if *seen == target {
                return true;
            }
            *seen += 1;
            if let BmNode::Group { items, .. } = n {
                if walk(items, target, seen, path) {
                    return true;
                }
            }
            path.pop();
        }
        false
    }
    let mut path = Vec::new();
    let mut seen = 0usize;
    walk(tree, ix, &mut seen, &mut path).then_some(path)
}

fn bm_at<'a>(tree: &'a [BmNode], path: &[usize]) -> Option<&'a BmNode> {
    let (last, rest) = path.split_last()?;
    let mut cur = tree;
    for &i in rest {
        cur = match cur.get(i)? {
            BmNode::Group { items, .. } => items,
            _ => return None,
        };
    }
    cur.get(*last)
}

fn bm_at_mut<'a>(tree: &'a mut Vec<BmNode>, path: &[usize]) -> Option<&'a mut BmNode> {
    let (last, rest) = path.split_last()?;
    let mut cur: &mut Vec<BmNode> = tree;
    for &i in rest {
        cur = match cur.get_mut(i)? {
            BmNode::Group { items, .. } => items,
            _ => return None,
        };
    }
    cur.get_mut(*last)
}

fn bm_take(tree: &mut Vec<BmNode>, path: &[usize]) -> Option<BmNode> {
    let (last, rest) = path.split_last()?;
    let mut cur: &mut Vec<BmNode> = tree;
    for &i in rest {
        cur = match cur.get_mut(i)? {
            BmNode::Group { items, .. } => items,
            _ => return None,
        };
    }
    (*last < cur.len()).then(|| cur.remove(*last))
}

/* ---- the four STRUCTURAL operations, pure so the tests drive them directly.
   NONE of them takes a vault path, calls the Index, fs::rename, create_note_in
   or delete_note_in: a group organises NAMES and never touches a .md on disk
   (R4X.2, criterion 6 — which the phase proves with a byte comparison). ---- */

fn bm_group_new_in(tree: &mut Vec<BmNode>, parent: Option<usize>) -> Result<(), String> {
    let node = BmNode::group(BM_NEW_GROUP, Vec::new());
    match parent {
        None => {
            tree.push(node);
            Ok(())
        }
        Some(ix) => {
            let p = bm_path_of(tree, ix).ok_or("no such row")?;
            match bm_at_mut(tree, &p) {
                Some(BmNode::Group { items, .. }) => {
                    items.push(node);
                    Ok(())
                }
                _ => Err("parent row is not a group".to_string()),
            }
        }
    }
}

fn bm_group_rename_in(tree: &mut Vec<BmNode>, ix: usize, title: &str) -> Result<(), String> {
    let p = bm_path_of(tree, ix).ok_or("no such row")?;
    match bm_at_mut(tree, &p) {
        Some(BmNode::Group { title: t, .. }) => {
            *t = title.to_string();
            Ok(())
        }
        _ => Err("that row is not a group".to_string()),
    }
}

/* R4X.3 — delete takes the SUBTREE and re-parents nothing, copied from stock
   deliberately (`23-del-menu.png` -> `24-deleted.png`), and it deletes NAMES:
   the notes behind them are still on disk afterwards. */
fn bm_group_delete_in(tree: &mut Vec<BmNode>, ix: usize) -> Result<(), String> {
    let p = bm_path_of(tree, ix).ok_or("no such row")?;
    match bm_at(tree, &p) {
        Some(BmNode::Group { .. }) => {}
        _ => return Err("that row is not a group".to_string()),
    }
    bm_take(tree, &p).map(|_| ()).ok_or_else(|| "no such row".to_string())
}

/* `into: None` is the SUPERSET over stock (doc §4): move back out, to the end
   of the top level. Both endpoints are resolved BEFORE the detach, because
   removing a row renumbers its siblings. */
fn bm_move_in_tree(tree: &mut Vec<BmNode>, ix: usize, into: Option<usize>) -> Result<(), String> {
    let src = bm_path_of(tree, ix).ok_or("no such row")?;
    let dst = match into {
        None => None,
        Some(d) => {
            let p = bm_path_of(tree, d).ok_or("no such group")?;
            match bm_at(tree, &p) {
                Some(BmNode::Group { .. }) => {}
                _ => return Err("target row is not a group".to_string()),
            }
            if p.len() >= src.len() && p[..src.len()] == src[..] {
                return Err("cannot move a row into itself".to_string());
            }
            Some(p)
        }
    };
    let node = bm_take(tree, &src).ok_or("no such row")?;
    match dst {
        None => tree.push(node),
        Some(mut p) => {
            let lvl = src.len() - 1; // the detach shifted src's later siblings
            if p.len() > lvl && p[..lvl] == src[..lvl] && p[lvl] > src[lvl] {
                p[lvl] -= 1;
            }
            match bm_at_mut(tree, &p) {
                Some(BmNode::Group { items, .. }) => items.push(node),
                _ => {
                    tree.push(node); // never lose the row we detached
                    return Err("target group vanished".to_string());
                }
            }
        }
    }
    Ok(())
}

/* bmdrag — the two painted<->raw maps the drag commit needs. Rows and slots in
   the PANE number only painted nodes, but bm_path_of's paths and the items
   vecs are RAW: an Opaque node (the stock fixture's search entry, anything the
   model cannot express) occupies a raw index the pane never shows. A drop
   names a PAINTED slot, so it must be translated before Vec::insert or a
   fixture carrying opaques lands the row one off. */
fn bm_painted_ix(list: &[BmNode], raw: usize) -> usize {
    list[..raw].iter().filter(|n| !matches!(n, BmNode::Opaque(_))).count()
}
fn bm_raw_slot(list: &[BmNode], pos: usize) -> usize {
    let mut painted = 0usize;
    for (i, n) in list.iter().enumerate() {
        if matches!(n, BmNode::Opaque(_)) {
            continue;
        }
        if painted == pos {
            return i; // insert BEFORE the painted node currently in slot pos
        }
        painted += 1;
    }
    list.len() // pos == painted len: append after everything, opaques included
}

/* bmdrag — the drag gesture's commit: a POSITIONAL insert. bm_move_in_tree can
   only PUSH (the Edit modal's semantics, doc §4); a drop names an exact slot,
   so this fn shares its helpers and its refusal predicate and inserts at
   `pos`, a PAINTED slot among `parent`'s children (parent None = top level).
   A refusal Err returns BEFORE bm_apply ever writes: the file is not touched,
   not even rewritten with identical bytes — recon-bmdrag case 6 measured stock
   bumping mtime on a refused drop, so the phase asserts BYTES and both pass.
   Both endpoints are resolved BEFORE the detach, then two fixups, because
   removing the source renumbers (a) a destination group sitting after it at
   the same level (raw path component) and (b) the slot itself on a
   same-parent move — fixup (b) is the one-line break negctl-bmdrag deletes. */
fn bm_drag_in_tree(tree: &mut Vec<BmNode>, ix: usize, parent: Option<usize>, mut pos: usize) -> Result<(), String> {
    let src = bm_path_of(tree, ix).ok_or("no such row")?;
    let dst = match parent {
        None => None,
        Some(g) => {
            let p = bm_path_of(tree, g).ok_or("no such group")?;
            match bm_at(tree, &p) {
                Some(BmNode::Group { .. }) => {}
                _ => return Err("target row is not a group".to_string()),
            }
            if p.len() >= src.len() && p[..src.len()] == src[..] {
                return Err("cannot move a row into itself".to_string());
            }
            Some(p)
        }
    };
    let lvl = src.len() - 1;
    // src's painted index among its own siblings, read BEFORE the detach
    let src_painted = {
        let sib: &[BmNode] = if lvl == 0 {
            tree
        } else {
            match bm_at(tree, &src[..lvl]) {
                Some(BmNode::Group { items, .. }) => items,
                _ => return Err("no such row".to_string()),
            }
        };
        bm_painted_ix(sib, src[lvl])
    };
    let node = bm_take(tree, &src).ok_or("no such row")?;
    let dstp = dst.map(|mut p| {
        if p.len() > lvl && p[..lvl] == src[..lvl] && p[lvl] > src[lvl] {
            p[lvl] -= 1; // fixup (a): the detach shifted src's later siblings
        }
        p
    });
    let same_list = match &dstp {
        None => lvl == 0,
        Some(p) => p.len() == lvl && p[..] == src[..lvl],
    };
    if same_list && src_painted < pos {
        pos -= 1; // fixup (b): the slot was numbered against the pre-detach list
    }
    let items: &mut Vec<BmNode> = match &dstp {
        None => tree,
        Some(p) => match bm_at_mut(tree, p) {
            Some(BmNode::Group { items, .. }) => items,
            _ => {
                tree.push(node); // never lose the row we detached
                return Err("target group vanished".to_string());
            }
        },
    };
    let raw = bm_raw_slot(items, pos); // never exceeds items.len() by construction
    items.insert(raw, node);
    Ok(())
}

/* R9.8 — a bookmark IS a vault-relative name, so the note that changes its
   name takes its bookmark with it. Called from move_note_in and nowhere else:
   that is the one function that knows both ends, and it is the function BOTH
   the rename path (rename_in) and the move path (the move_note command) go
   through, so a moved note keeps its bookmark by the same code that a renamed
   one does. Duplicating this into the two callers is how the two drift apart.

   Rules, each one a test in `mod tests`:
   - the entry keeps its INDEX. Order is insertion order and that is the order
     the pane paints, so a bookmark must not fall to the bottom because its
     note was renamed. Hence the rebuild-in-place rather than remove+push.
   - a note that was NOT bookmarked gains nothing, and the file is not even
     rewritten — nothing to follow means no write at all.
   - NO bookmarks file means no bookmarks: we do not materialise an empty one
     as a side effect of a rename.
   - renaming onto a name that is ALREADY bookmarked (possible: the target
     name may be a stale bookmark with no file, so the kernel does not refuse
     the rename) collapses to ONE entry, at the old entry's position.
   - best-effort by design: the note is already moved when we get here. A
     bookmarks file we cannot write is not a reason to report the rename as
     failed — that would be a lie about the note, which is the thing that
     matters. It is also why the call sits AFTER fs::rename: a rename refused
     by create_new(2) returns before this line, so the file is untouched. */
fn rename_bookmark_in(root: &Path, old: &str, new: &str) {
    if old == new {
        return;
    }
    let p = root.join(BM_FILE);
    if !p.is_file() {
        return; // no bookmarks file: nothing to follow, nothing to create
    }
    let mut tree = read_bm_tree(root);
    if !bm_rename_walk(&mut tree, old, new) {
        return; // this note was not bookmarked — do not touch the file
    }
    let _ = write_bm_tree(root, &tree);
}

/* R4X.4 (criterion 5) — IN PLACE, wherever the entry SITS: the payload of the
   matching `f` node is replaced and the node does not move, so a bookmark
   inside a group keeps its GROUP and its index within that group. Rebuilding a
   flat list here (what this function used to do) would silently flatten the
   whole tree on the next rename — exactly the bug the clause exists to forbid.
   Any OTHER entry equal to `old` or to `new`, at any depth, is dropped: that
   is the stale-bookmark collapse R9.8 already required. Returns whether the
   old name was found at all, because "not bookmarked" means NO write. */
fn bm_rename_walk(tree: &mut Vec<BmNode>, old: &str, new: &str) -> bool {
    fn walk(nodes: &mut Vec<BmNode>, old: &str, new: &str, done: &mut bool) {
        let mut i = 0;
        while i < nodes.len() {
            let mut drop = false;
            match &mut nodes[i] {
                BmNode::File { name: n, .. } => {
                    if !*done && n == old {
                        *n = new.to_string();
                        *done = true;
                    } else if n == old || n == new {
                        drop = true;
                    }
                }
                BmNode::Group { items, .. } => walk(items, old, new, done),
                BmNode::Opaque(_) => {} // not a name; never followed, never dropped
            }
            if drop {
                nodes.remove(i);
            } else {
                i += 1;
            }
        }
    }
    let mut done = false;
    walk(tree, old, new, &mut done);
    done
}

/* toggle_bookmark keeps its by-NAME signature (R9.4/R20.4): toggling ON
   appends at the TOP level, toggling OFF removes the first pre-order match AT
   WHATEVER DEPTH IT SITS — a bookmark inside a group is un-bookmarked from
   inside that group, and the group stays. */
fn bm_toggle_in(tree: &mut Vec<BmNode>, name: &str) {
    if !bm_drop_first_name(tree, name) {
        tree.push(BmNode::file(name));
    }
}

fn bm_drop_first_name(nodes: &mut Vec<BmNode>, name: &str) -> bool {
    for i in 0..nodes.len() {
        if matches!(&nodes[i], BmNode::File { name: n, .. } if n == name) {
            nodes.remove(i);
            return true;
        }
        if let BmNode::Group { items, .. } = &mut nodes[i] {
            if bm_drop_first_name(items, name) {
                return true;
            }
        }
    }
    false
}

#[tauri::command]
fn list_bookmarks(v: State<Vault>) -> Vec<String> {
    span_timed!("list_bookmarks", cur_vault(&v).map(|r| read_bookmarks(&r)).unwrap_or_default())
}

/// the pane's own source: one painted row per entry, in file order.
#[tauri::command]
fn bookmark_rows(v: State<Vault>) -> Vec<BmRow> {
    span_timed!("bookmark_rows", cur_vault(&v).map(|r| bm_rows_of(&read_bm_tree(&r))).unwrap_or_default())
}

#[tauri::command]
fn toggle_bookmark(v: State<Vault>, name: String) -> Result<Vec<String>, String> {
    span_timed!("toggle_bookmark", {
        let root = cur_vault(&v).ok_or("no vault open")?;
        let mut tree = read_bm_tree(&root);
        bm_toggle_in(&mut tree, &name);
        write_bm_tree(&root, &tree)?;
        let mut out = Vec::new();
        bm_names(&tree, &mut out);
        Ok(out)
    })
}

/* every structural command is the same three steps — read the tree, mutate it
   BY ROW INDEX, write it through the one serializer — and each returns the new
   painted vector, so the UI repaints what the FILE says instead of its own
   guess about what its click did. */
fn bm_apply(v: &State<Vault>, f: impl FnOnce(&mut Vec<BmNode>) -> Result<(), String>) -> Result<Vec<BmRow>, String> {
    let root = cur_vault(v).ok_or("no vault open")?;
    let mut tree = read_bm_tree(&root);
    f(&mut tree)?;
    write_bm_tree(&root, &tree)?;
    Ok(bm_rows_of(&tree))
}

#[tauri::command]
fn bm_group_new(v: State<Vault>, parent: Option<usize>) -> Result<Vec<BmRow>, String> {
    span_timed!("bm_group_new", bm_apply(&v, |t| bm_group_new_in(t, parent)))
}

#[tauri::command]
fn bm_group_rename(v: State<Vault>, ix: usize, title: String) -> Result<Vec<BmRow>, String> {
    span_timed!("bm_group_rename", bm_apply(&v, |t| bm_group_rename_in(t, ix, &title)))
}

#[tauri::command]
fn bm_group_delete(v: State<Vault>, ix: usize) -> Result<Vec<BmRow>, String> {
    span_timed!("bm_group_delete", bm_apply(&v, |t| bm_group_delete_in(t, ix)))
}

#[tauri::command]
fn bm_move(v: State<Vault>, ix: usize, into: Option<usize>) -> Result<Vec<BmRow>, String> {
    span_timed!("bm_move", bm_apply(&v, |t| bm_move_in_tree(t, ix, into)))
}

/* bmdrag — the drop's commit route: same bm_apply three-step as every other
   structural command, so the pane repaints what the FILE says. A refused drop
   (own descendant) is an Err out of f() and bm_apply never reaches
   write_bm_tree: the refusal is byte-level by construction. */
#[tauri::command]
fn bm_drag(v: State<Vault>, ix: usize, parent: Option<usize>, pos: usize) -> Result<Vec<BmRow>, String> {
    span_timed!("bm_drag", bm_apply(&v, |t| bm_drag_in_tree(t, ix, parent, pos)))
}

/* R11 watcher thread: every TICK_MS walk the vault (stat only), diff against
   the last snapshot, reconcile candidates with the Index under its lock
   (src/watcher.rs), emit `vault-changed` when anything external happened.
   A root switch (set_vault/create_vault) just reseeds the snapshot silently.
   Idle cost = one read_dir walk + one stat per note per tick, no reads. */
fn spawn_watcher(app: tauri::AppHandle) {
    use tauri::{Emitter, Manager};
    std::thread::spawn(move || {
        let mut prev: Option<(PathBuf, watcher::Snapshot)> = None;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(watcher::TICK_MS));
            let v = app.state::<Vault>();
            let Some(root) = cur_vault(&v) else { prev = None; continue };
            let mut cur = watcher::snapshot(&root);
            let change = match &prev {
                Some((r, s)) if *r == root => {
                    let mut d = watcher::diff(s, &cur);
                    /* THE S1 GUARD (goal/tabclose). A removal is the only
                       change class that DESTROYS state the user cannot get
                       back from the event: the UI closes those tabs (R11.4)
                       and whatever was inside the save debounce goes with
                       them. A walk that came back short produces exactly the
                       same Diff as a mass delete, so a claimed removal is
                       CONFIRMED before it is believed — a second walk, then an
                       lstat per name still claimed gone. Both cost nothing on
                       a tick that claims no removal, which is every idle
                       tick. */
                    if !d.removed.is_empty() {
                        let claimed = d.removed.len();
                        let mut c2 = watcher::snapshot(&root);
                        let mut d2 = watcher::diff(s, &c2);
                        let healed = watcher::heal_short_walk(&root, &mut c2, &mut d2);
                        if d2.removed.len() < claimed {
                            eprintln!(
                                "[tabclose] SHORT WALK REJECTED claimed={claimed} confirmed={} restat_healed={:?} kept={:?}",
                                d2.removed.len(),
                                healed,
                                d2.removed
                            );
                        }
                        cur = c2;
                        d = d2;
                    }
                    if d.added.is_empty() && d.removed.is_empty() && d.modified.is_empty() {
                        watcher::Change::default()
                    } else {
                        let mut ix = v.index.lock().unwrap();
                        span_timed!("watcher_reconcile", watcher::reconcile(&root, &d, &mut ix))
                    }
                }
                _ => watcher::Change::default(),
            };
            prev = Some((root, cur));
            if !change.is_empty() {
                let _ = app.emit("vault-changed", &change);
            }
        }
    });
}

/* ---------- R33 frameless window: the app draws its own frame ----------
   The window ships with `decorations: false` (tauri.conf.json), so there is no
   WM titlebar AND no WM resize border: every affordance the frame used to
   provide has to come from here, driven by #wframe / the resize handles in
   ui/. Stock Obsidian does the same (recon: its window reports
   _NET_FRAME_EXTENTS 0,0,0,0 under openbox while showing its own strip).

   WHY WE MOVE AND RESIZE THE WINDOW OURSELVES instead of handing the gesture
   to the WM (start_dragging / start_resize_dragging, i.e.
   _NET_WM_MOVERESIZE): the gesture must also work where there is NO window
   manager at all — our Xvfb rigs have none, and an undecorated window there
   has no other way to be moved or resized. So the UI sends the anchor rect
   plus the pointer delta and we apply the resulting outer geometry. Cost: the
   window follows the cursor one frame late. Deliberate divergence, recorded
   in docs/requirements.md R33. */
// The floor our own gesture enforces. It is R22's smallest supported window
// (the fuzz phase drives the app down to 320x240 and asserts the chrome still
// fits), NOT a tauri.conf minWidth: a config minimum would make GTK refuse the
// fuzz phase's own resizes and turn an R22 census into a false failure.
const WIN_MIN_W: f64 = 320.0;
const WIN_MIN_H: f64 = 240.0;

/// The outer rect a drag in direction `dir` produces, from the rect the
/// gesture started on (`r` = x, y, w, h) and the cursor delta since then.
/// `dir`: "move", or an edge/corner as n/s/e/w ("n", "se", ...). `None` = an
/// unknown direction: a typo must be an error, never a silent no-op.
/// The edge OPPOSITE the one being dragged never moves, including when the
/// clamp bites (drag the west edge right past the minimum and x stops, it
/// does not keep walking).
fn gesture_rect(dir: &str, r: (f64, f64, f64, f64), dx: f64, dy: f64) -> Option<(f64, f64, f64, f64)> {
    let (x, y, w, h) = r;
    if dir == "move" {
        return Some(((x + dx).round(), (y + dy).round(), w, h));
    }
    let (n, s, e, we) = match dir {
        "n" => (true, false, false, false),
        "s" => (false, true, false, false),
        "e" => (false, false, true, false),
        "w" => (false, false, false, true),
        "ne" => (true, false, true, false),
        "nw" => (true, false, false, true),
        "se" => (false, true, true, false),
        "sw" => (false, true, false, true),
        _ => return None,
    };
    let (mut nx, mut ny, mut nw, mut nh) = (x, y, w, h);
    if e {
        nw = w + dx;
    }
    if we {
        nw = w - dx;
        nx = x + dx;
    }
    if s {
        nh = h + dy;
    }
    if n {
        nh = h - dy;
        ny = y + dy;
    }
    if nw < WIN_MIN_W {
        nw = WIN_MIN_W;
        if we {
            nx = x + w - WIN_MIN_W;
        }
    }
    if nh < WIN_MIN_H {
        nh = WIN_MIN_H;
        if n {
            ny = y + h - WIN_MIN_H;
        }
    }
    Some((nx.round(), ny.round(), nw.round(), nh.round()))
}

#[derive(serde::Serialize)]
struct WinRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    max: bool,
    dec: bool,
}

/// The window's OUTER geometry in logical px, plus the two facts the frame UI
/// needs: maximised, and decorated (`dec: false` is the whole point of R33 —
/// it is also the config-level oracle for the WM-less displays, where no
/// screenshot can tell a decorated window from an undecorated one).
#[tauri::command]
fn win_rect(win: tauri::Window) -> Result<WinRect, String> {
    let sf = win.scale_factor().map_err(|e| e.to_string())?;
    let p = win.outer_position().map_err(|e| e.to_string())?.to_logical::<f64>(sf);
    let s = win.outer_size().map_err(|e| e.to_string())?.to_logical::<f64>(sf);
    Ok(WinRect {
        x: p.x,
        y: p.y,
        w: s.width,
        h: s.height,
        max: win.is_maximized().map_err(|e| e.to_string())?,
        dec: win.is_decorated().map_err(|e| e.to_string())?,
    })
}

/// Apply one step of a move/resize gesture. The UI sends the ANCHOR rect
/// (win_rect at pointerdown) and the cumulative delta, so the geometry is
/// absolute at every step and a dropped event cannot make the window drift.
/// TABCLOSE INSTRUMENTATION — the one place a tab removal cannot lie.
///
/// The operator reported "when I'm editing a note, it randomly closes". There
/// was no way to answer WHY from any artifact: the census is a title that only
/// carries the CURRENT state, and a tab that is gone leaves nothing behind. So
/// every path that removes a tab (ui/main.js dropTab / closeTab /
/// collapseGroup / enterVault) calls this, and the line lands in the app log
/// (`$OUT/app-*.log` under the smoke rig, stderr for a real user) where a
/// reviewer can grep it long after the window is closed.
///
/// `cause` is CLOSED, exactly five values — an unknown one is an error, never a
/// silent pass-through, because "some other path removed it" is precisely the
/// diagnosis that was missing:
///   user-close       the user asked: close glyph, ctrl+w, middle click, the
///                    delete dialog's own tidy-up
///   external-delete  the watcher saw the file vanish from the vault
///   external-rename  the watcher paired the vanished file with a new name
///   pane-collapse    the group went with its last tab (R6.5)
///   session-replace  the whole layout was thrown away (vault switch)
/// `dirty` and `flushed` are the F-class half: a removal with dirty=1
/// flushed=0 IS the data loss, stated in the log at the moment it happens.
#[tauri::command]
fn tab_removed(
    cause: String,
    note: String,
    dirty: bool,
    flushed: bool,
    preserved: bool,
    via: String,
    seq: u32,
    tabs_left: usize,
    groups: usize,
) -> Result<(), String> {
    if !TAB_REMOVAL_CAUSES.contains(&cause.as_str()) {
        // Loud, and still logged: a caller that invents a cause is a bug in the
        // instrumentation, and swallowing it would rebuild the blind spot.
        eprintln!("[tabgone] BAD-CAUSE cause={cause} note={note} seq={seq}");
        return Err(format!("unknown tab-removal cause: {cause}"));
    }
    eprintln!(
        "[tabgone] seq={seq} cause={cause} note={note} dirty={} flushed={} preserved={} via={via} tabs_left={tabs_left} groups={groups}",
        u8::from(dirty),
        u8::from(flushed),
        u8::from(preserved)
    );
    Ok(())
}

/// The closed set, shared by the command above and the UI (ui/main.js keeps the
/// same five strings in TAB_CAUSES; a test in this file pins them together so
/// the two lists cannot drift apart unnoticed).
const TAB_REMOVAL_CAUSES: [&str; 5] =
    ["user-close", "external-delete", "external-rename", "pane-collapse", "session-replace"];

#[tauri::command]
fn win_gesture(
    win: tauri::Window,
    proto: tauri::State<'_, DragProto>,
    dir: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    dx: f64,
    dy: f64,
) -> Result<(), String> {
    // THE ONE PLACE A GESTURE CANNOT LIE. The census ([wfg:]/[wfl:]) is published
    // through the window TITLE, and a title that stops updating looks exactly like a
    // gesture that never happened — an iteration was spent on that ambiguity, with
    // four resizes landing pixel-perfect while the census still showed the previous
    // one. This line is written by the process that actually moves the window.
    eprintln!("[win_gesture] dir={dir} anchor={x},{y},{w}x{h} d={dx},{dy}");
    let (nx, ny, nw, nh) =
        gesture_rect(&dir, (x, y, w, h), dx, dy).ok_or_else(|| format!("unknown gesture direction: {dir}"))?;
    if nw != w || nh != h {
        win.set_size(tauri::LogicalSize::new(nw, nh)).map_err(|e| e.to_string())?;
    }
    if nx != x || ny != y {
        win.set_position(tauri::LogicalPosition::new(nx, ny)).map_err(|e| e.to_string())?;
    }
    // ---- R33.6b THE MEASUREMENT THAT CHOOSES THE PATH (see drag_path) --------
    // Everything above is unchanged, on purpose: the geometry path runs FIRST and
    // in full, so a session that honours it behaves exactly as it does today and
    // phase panedrag passes for the reason it passes now. What follows only LOOKS.
    //
    // Why not at the first step: set_position on X11 is a request, and the reply
    // (ConfigureNotify) arrives later — reading back immediately would call a
    // healthy WM a liar and hand the press over on the one desktop where our
    // arithmetic is exact (openbox, E1). So the verdict waits for a gesture that
    // has been asking for a real displacement for a real amount of time, and asks
    // the only question that cannot be faked: did the window leave the anchor?
    if dir == "move" && win_gesture_should_probe(&proto) {
        let settle = std::time::Duration::from_millis(250);
        let mut st = proto.pos.lock().map_err(|e| e.to_string())?;
        match st.anchor {
            // a different anchor = a different gesture: restart the clock.
            Some(a) if a == (x, y) => {}
            _ => {
                st.anchor = Some((x, y));
                st.t0 = Some(std::time::Instant::now());
            }
        }
        let old_enough = st.t0.map(|t| t.elapsed() >= settle).unwrap_or(false);
        // 8 px: below that a WM's own snapping could legitimately eat the delta,
        // and a verdict off a 1 px request would be noise.
        if old_enough && dx.abs().max(dy.abs()) >= 8.0 {
            let sf = win.scale_factor().map_err(|e| e.to_string())?;
            let p = win.outer_position().map_err(|e| e.to_string())?.to_logical::<f64>(sf);
            let moved = (p.x - x).abs().max((p.y - y).abs()) >= 3.0;
            st.honoured = Some(moved);
            drop(st); // start_dragging re-enters nothing, but never hold a lock across it
            eprintln!(
                "[win_drag] position probe: anchor={x},{y} requested={nx},{ny} actual={},{} honoured={moved}",
                p.x, p.y
            );
            if !moved {
                // The press is STILL DOWN — this is the same gesture. Handing over
                // now rescues the very first drag of the session instead of making
                // the user drag twice; every later press takes the "wm"/"wayland"
                // branch in wfBegin without any of this running again.
                eprintln!("[win_drag] the session ignores client positioning — handing this press over mid-gesture");
                win.start_dragging().map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

/// Is the empirical probe worth running at all? Only where a verdict could
/// CHANGE the path: an X11 display, a WM that advertises the move protocol, and
/// no verdict yet. On the WM-less rigs this is false at boot and stays false, so
/// the gate's gesture path runs exactly the code it ran before R33.6b.
fn win_gesture_should_probe(proto: &tauri::State<'_, DragProto>) -> bool {
    proto.x11 && proto.moveresize && proto.pos.lock().map(|p| p.honoured.is_none()).unwrap_or(false)
}

/* ---------- R33.6b TWO-PATH DRAG: who is allowed to move this window? -------
   MEASURED, not assumed — docs/recon-hdrdrag/README.md carries the transcripts:
     E1  X11 + openbox 3.6.1 on Xvfb :118: our anchor+delta client-set geometry
         SURVIVES exactly (no snap, no frame offset, no late revert). R33.6's
         suspicion that a real WM "fights" us is FALSE there.
     E2  a Wayland session with the app as an XWAYLAND client (sway 1.9 /
         Xwayland 23.2.6): our whole path runs — win_gesture 0->2->4, four
         [win_gesture] lines in the app log — and the window moves 0 px. The
         control settles it: an external `xdotool windowmove` is ignored too,
         while the compositor's own `move position` works. The compositor owns
         the position, so NO X11-side position request is honoured, ever.
     E3  no WM at all (every Xvfb gate rig): our path works and is the ONLY
         mechanism that can — there is nobody to hand the press to.
   So the fix is not arithmetic, it is WHO MOVES THE WINDOW, chosen per session.

   THE TEST IS POSITIVE AND ABOUT THE LIVE SESSION, never a platform string:
   `gdk_x11_screen_supports_net_wm_hint(_NET_WM_MOVERESIZE)` asks the running
   root window's `_NET_SUPPORTED` (behind a live `_NET_SUPPORTING_WM_CHECK`),
   i.e. "is there a window manager here, right now, that implements the move
   protocol?" — openbox YES, sway's XWayland YES, the WM-less rig NO. A display
   that does not downcast to an X11 display is a native Wayland display, where a
   client cannot position itself at all and `start_dragging` (xdg_toplevel.move)
   is the only mechanism that exists.
   WHEN THE DETECTION IS WRONG, each direction: a false "wm" hands the press to
   a WM that ignores _NET_WM_MOVERESIZE -> the window does not move (no drift,
   no damage, and the [wfp:] census token says which path was taken); a false
   "none" falls back to today's geometry path, which is correct wherever the
   client may position itself and a 0 px no-op where it may not — exactly the
   bug being fixed, never worse than it. The startup half is probed ONCE on the
   GTK main thread: a WM started mid-session keeps the old answer until restart,
   which is stated here because it is real and is the reason [wfp:] is published.
   AND THE RULE ITSELF WAS MEASURED WRONG ONCE, WHICH IS WHY IT IS THIS SHAPE.
   The first cut handed the press over whenever a WM advertised the protocol.
   Under openbox that SHIPS A REGRESSION: docs/recon-hdrdrag/B-ALT.log drives
   five consecutive header drags and gets MOVED, DEAD, MOVED, DEAD, MOVED — the
   WM's move loop takes an X pointer grab, the webview never sees the release,
   and every other press is swallowed. E1 says our own geometry path is EXACT
   there. So "a move protocol exists" is NOT a reason to hand over; "this window
   cannot position itself" is. Those two are the same thing only on Wayland.
   THE DISCRIMINATOR IS THEREFORE EMPIRICAL, and it is the most positive test
   available: request a position, then look at where the window actually IS.
   A session that honours client positioning (X11, WM or not) keeps today's
   anchor+delta path byte-for-byte; a session that drops the request on the floor
   (XWayland under a compositor) is detected BY THAT and the press is handed over
   — mid-gesture the first time, at press time thereafter. */
fn drag_path(is_x11: bool, moveresize_advertised: bool, position_honoured: Option<bool>) -> &'static str {
    match (is_x11, moveresize_advertised, position_honoured) {
        // not an X11 display => native Wayland: a client cannot position itself at
        // all, so there is nothing to measure and no second path to choose from.
        (false, _, _) => "wayland",
        // an X server where no WM advertises the move protocol: the app's own
        // absolute geometry is the only thing that CAN move this window (E3, every
        // Xvfb gate rig). This arm is the belt on the braces — even a wrong
        // empirical verdict cannot take the gate off its measured path.
        (true, false, _) => "none",
        // MEASURED: a position request was issued and the window did not move.
        // Someone else owns the position; hand the press to them (E2).
        (true, true, Some(false)) => "wm",
        // honoured, or not yet measured: the path this repo has evidence for.
        (true, true, _) => "none",
    }
}

/// What this session lets us do, probed once at setup + refined by measurement.
struct DragProto {
    /// is the live GdkDisplay an X11 one (a real X server OR XWayland)?
    x11: bool,
    /// does a WM advertise `_NET_WM_MOVERESIZE` on the root window right now?
    moveresize: bool,
    /// the empirical half — `Some(false)` once a move request has been measured
    /// to do nothing. Written by win_gesture, read by win_move_proto.
    pos: Mutex<PosProbe>,
}

/// State for the one measurement that decides the path (see win_gesture).
#[derive(Default)]
struct PosProbe {
    /// the anchor of the gesture currently being watched — a new anchor is a new
    /// gesture, which is how the probe knows to restart its clock.
    anchor: Option<(f64, f64)>,
    /// when that gesture's first step was applied.
    t0: Option<std::time::Instant>,
    /// None = not measured yet; Some(true) = the window moved when asked.
    honoured: Option<bool>,
}

#[cfg(target_os = "linux")]
fn probe_drag_proto() -> DragProto {
    use gdk::prelude::*;
    let Some(display) = gdk::Display::default() else {
        // no GdkDisplay at all: nothing to hand a press to.
        return DragProto { x11: true, moveresize: false, pos: Mutex::new(PosProbe::default()) };
    };
    // The downcast IS the session test: GDK hands back a GdkX11Screen on a real
    // X server AND under XWayland, and a GdkWaylandScreen on a native Wayland
    // session — the live object the app is drawing on, not $XDG_SESSION_TYPE,
    // which is a login-time string a launcher can set to anything.
    match display.default_screen().downcast::<gdkx11::X11Screen>() {
        // gdk_x11_screen_supports_net_wm_hint re-reads `_NET_SUPPORTED` off the
        // root window behind a live `_NET_SUPPORTING_WM_CHECK`, so this is "is a
        // WM running here NOW and does it implement the move protocol", not "was
        // one running when someone built this".
        Ok(xs) => DragProto {
            x11: true,
            moveresize: xs.supports_net_wm_hint(&gdk::Atom::intern("_NET_WM_MOVERESIZE")),
            pos: Mutex::new(PosProbe::default()),
        },
        Err(_) => DragProto { x11: false, moveresize: false, pos: Mutex::new(PosProbe::default()) },
    }
}

#[cfg(not(target_os = "linux"))]
fn probe_drag_proto() -> DragProto {
    // Nothing is measured off this platform, so claim nothing: the geometry path
    // is the one this repo has evidence for.
    DragProto { x11: true, moveresize: false, pos: Mutex::new(PosProbe::default()) }
}

/// Which mechanism will move this window — "wm" | "wayland" | "none".
/// Read-only; the UI caches it and publishes it as the `[wfp:]` census token, so
/// a test never has to infer the path from whether the window moved. It is
/// re-read after each drag because the empirical half can flip it mid-session.
#[tauri::command]
fn win_move_proto(proto: tauri::State<'_, DragProto>) -> String {
    let honoured = proto.pos.lock().map(|p| p.honoured).unwrap_or(None);
    drag_path(proto.x11, proto.moveresize, honoured).to_string()
}

/// R33.6b: hand the press to the WM/compositor when this session will not let the
/// window position itself. Returns the path actually taken, so the UI can log it
/// and a smoke phase can assert on it; "none" means the caller must run today's
/// anchor+delta gesture.
///
/// TIMED, under its own name, and it is NOT the same case as win_minimize /
/// win_toggle_max / win_rect (all declared out of scope in
/// scripts/perf-coverage.sh because the WM owns the time and a screenshot phase
/// is the only honest clock for it). This command sits at the START of a user
/// gesture: the user has the button down and is already moving the mouse, so
/// every millisecond spent here is latency the user feels as a window that does
/// not follow the pointer yet. What the span measures is OURS, not the WM's —
/// take the PosProbe mutex (contended with win_gesture, which writes it from the
/// motion stream), decide the path, and dispatch. `start_dragging()` posts
/// _NET_WM_MOVERESIZE and returns; the WM's own move loop happens afterwards and
/// is not inside this region. If this ever crosses the ceiling the cause is a
/// lock we hold or a round trip we added, both of which are this app's to fix,
/// and an op that emits no span renders as fast BY BEING ABSENT.
#[tauri::command]
fn win_drag_start(win: tauri::Window, proto: tauri::State<'_, DragProto>) -> Result<String, String> {
    // the path the gesture actually took, lifted out of the timed region so the
    // span carries it: "wm" and "none" are different code paths with different
    // costs, and a span that cannot tell them apart is a number without a unit.
    let mut taken = "none";
    span_timed!(
        "win_drag_start",
        {
            let honoured = proto.pos.lock().map(|p| p.honoured).unwrap_or(None);
            let p = drag_path(proto.x11, proto.moveresize, honoured);
            taken = p;
            // Written by the process that actually asks, for the same reason
            // win_gesture logs: a title census that stops updating looks exactly
            // like a handover that never happened.
            eprintln!("[win_drag] proto={p} handover={}", p != "none");
            // no `?` inside the timed region on purpose: an early return would
            // jump over the measurement, so the FAILING handover — the slow one
            // worth seeing — would be the one case that emits no span.
            if p != "none" {
                win.start_dragging().map_err(|e| e.to_string()).map(|()| p.to_string())
            } else {
                Ok(p.to_string())
            }
        },
        serde_json::json!({"path": taken})
    )
}

#[tauri::command]
fn win_minimize(win: tauri::Window) -> Result<(), String> {
    win.minimize().map_err(|e| e.to_string())
}

/// maximise <-> restore; returns the state it left the window in
#[tauri::command]
fn win_toggle_max(win: tauri::Window) -> Result<bool, String> {
    let m = win.is_maximized().map_err(|e| e.to_string())?;
    if m {
        win.unmaximize().map_err(|e| e.to_string())?;
    } else {
        win.maximize().map_err(|e| e.to_string())?;
    }
    Ok(!m)
}

/// F1: closing the window is the app's own affordance, not the WM's — with
/// decorations off there is no frame button, and on a bare X server there is
/// no WM to ask. `close()` runs the normal close path (window events fire).
#[tauri::command]
fn win_close(win: tauri::Window) -> Result<(), String> {
    win.close().map_err(|e| e.to_string())
}

/* ---------- R36 INTERFACE ZOOM (operator, 2026-09-14: "interface wide zoom,
   not only text") ----------------------------------------------------------
   Ctrl+= / Ctrl+- / Ctrl+0, and the three palette commands behind them.
   The scale is applied by exactly ONE call — tauri `Webview::set_zoom` ->
   wry-0.55.1 `WebView::zoom` -> `webkit_web_view_set_zoom_level` — which
   scales the CSS PIXEL for the whole page. The window is frameless (R33), so
   everything the user can see is inside that webview: ribbon, sidebar, tab
   bar, icons and text move together, by construction, because they are all
   measured in the unit that changed.
   NOT a CSS font-size/rem scale. That moves the text and leaves every fixed-px
   chrome dimension (sidebar width, ribbon width, tab height) where it was —
   the text-only outcome the operator explicitly rejected. It is not merely
   discouraged here: negative control N1 in docs/negctl-zoom/ implements it,
   and `scripts/smoke.sh fast zoom` measures three NON-TEXT boundaries, so N1
   is RED.

   EVERY CONSTANT BELOW IS MEASURED OFF STOCK 1.13.7, BLACK-BOX, NOT CHOSEN
   (notes/recon-zoom.txt on :46; recon-zoom/drive-clamps.sh + clamps.log on :54;
   the readout is the sidebar/editor boundary x on the scanline y=400):
     STEP   one press = HALF an Electron zoom level, factor 1.2^level:
            348 -> 381 -> 417 going in, 318 going out.
            381/348 = 1.0948 vs 1.2^0.5  = 1.0954
            417/348 = 1.1983 vs 1.2^1.0  = 1.2
            318/348 = 0.9138 vs 1.2^-0.5 = 0.9129     (<= 0.1% apart)
            i.e. +9.54% / -8.71% per press. NOT 10%, NOT 20%.
     CEILING +6 presses = level +3.0 (1.728x): the sidebar edge stops at 598
            and presses 7..12 changed NOTHING AT ALL (pixel diff 0, six times).
     FLOOR  -5 presses = level -2.5 (0.6339x): edge stops at 222, presses
            6..10 pixel-diff 0. The range is ASYMMETRIC; that is what stock
            does, so it is what we do.
     RESET  Ctrl+0 after the ceiling returned the screen to a frame that is
            PIXEL-IDENTICAL to the baseline (diff 0), so reset means the
            original value, not "some neutral value" (negative control N3). */
/// Electron's zoom base — stock scales by this per whole level (recon-zoom).
const ZOOM_BASE: f64 = 1.2;
/// one keypress = half a level (measured 9.54% in / 8.71% out).
const ZOOM_STEP: f64 = 0.5;
/// measured ceiling: 6 presses in, then stock stops (recon-zoom/clamps.log).
const ZOOM_MAX: f64 = 3.0;
/// measured floor: 5 presses out, then stock stops (same log).
const ZOOM_MIN: f64 = -2.5;

/// zoom level -> the scale factor webkit is asked for.
fn zoom_factor(level: f64) -> f64 {
    ZOOM_BASE.powf(level)
}

/// `action` -> the level it lands on, clamped to the measured stock range.
/// Pure on purpose: the clamp is the part that can silently be wrong, and this
/// way it is unit-testable without a webview or a display.
fn zoom_next(level: f64, action: &str) -> Option<f64> {
    let l = match action {
        "in" => level + ZOOM_STEP,
        "out" => level - ZOOM_STEP,
        "reset" => 0.0,
        _ => return None,
    };
    Some(l.clamp(ZOOM_MIN, ZOOM_MAX))
}

#[derive(serde::Serialize)]
struct ZoomOut {
    level: f64,
    factor: f64,
    /// at a clamp — the UI can say so instead of the user pressing into silence
    clamped: bool,
}

/// the current zoom level of this app run (one webview; see R36.3 in docs)
struct ZoomLevel(Mutex<f64>);

/// R36.1 the ONE zoom entry point: "in" | "out" | "reset".
/// The step, the clamps and the level->factor math live in Rust (above), so
/// the palette command, the hotkey and any future settings row cannot drift
/// apart — they all land here.
#[tauri::command]
fn zoom(webview: tauri::Webview, z: State<ZoomLevel>, action: String) -> Result<ZoomOut, String> {
    let mut cur = z.0.lock().unwrap();
    let want = zoom_next(*cur, &action).ok_or_else(|| format!("unknown zoom action: {action}"))?;
    let f = zoom_factor(want);
    webview.set_zoom(f).map_err(|e| e.to_string())?;
    let clamped = want == *cur && action != "reset";
    *cur = want;
    set_zoom_cfg(want);
    Ok(ZoomOut { level: want, factor: f, clamped })
}

/// the level this app run STARTS at — persisted (R36.4), clamped on read so a
/// hand-edited config cannot park the UI outside the range the keys can leave.
/// Returns the SAME shape as `zoom` so the frontend never re-derives
/// level->factor: the base 1.2 and the 0.5 step exist in exactly one file.
#[tauri::command]
fn zoom_get() -> ZoomOut {
    let l = read_zoom_cfg();
    // `clamped` is false even when the restored level IS a clamp: nothing was
    // pressed into anything, and the census marker `!` means "that press did
    // nothing", which is a statement about a keypress, not about a value.
    ZoomOut { level: l, factor: zoom_factor(l), clamped: false }
}

fn read_zoom_cfg() -> f64 {
    cfg_value()["zoom"].as_f64().unwrap_or(0.0).clamp(ZOOM_MIN, ZOOM_MAX)
}

fn set_zoom_cfg(level: f64) {
    let mut v = cfg_value();
    v["zoom"] = serde_json::json!(level);
    let _ = fs::write(cfg_path(), v.to_string());
}

fn main() {
    // FIRST STATEMENT IN THE PROCESS, deliberately: the warm-up window (perf.rs,
    // WARMUP_MS) is measured from here, so anything that runs before this stamp
    // would be judged against a start time it predates.
    perf::mark_start();
    // perf-console: state the rule on the console BEFORE anything can breach it.
    // ONE line, printed unconditionally (breach or not), so "unusually long" is a
    // number you can read off the console instead of a promise in a comment. It is
    // also the whole quiet-case output of the feature: healthy run = this line, no
    // warnings. The ceiling itself is pinned in perf.rs; env can only tighten it.
    eprintln!("{}", perf::slow_banner());
    // VAULT_DIR (probes/tests) wins; else last persisted vault if still a dir (R1.6)
    let init = std::env::var("VAULT_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| read_cfg().0.map(PathBuf::from).filter(|p| p.is_dir()));
    // R18.1: open the telemetry sink BEFORE the sandbox closes. Landlock filters
    // path lookups, not open descriptors, and RUSTIDIAN_OTEL routinely names a
    // file outside the write roots (the gate's $OUT). Opening it after enforce()
    // is EACCES on every span, swallowed — see perf::SINK.
    perf::open_sink();
    // landlock: confine the whole process tree to the vault before webkit spawns
    // the off-switch is named ONCE, in sandbox.rs — not spelled again here
    if let (Some(p), true) = (&init, !sandbox::no_landlock_requested()) {
        match sandbox::enforce(p, &cfg_path()) {
            Ok(s) => eprintln!("landlock: {s:?}"),
            Err(e) => eprintln!("landlock: off ({e})"),
        }
    }
    // perf-index: one walk + read now, so the first note_open is already warm
    let index = init.as_deref().map(Index::build).unwrap_or_default();
    tauri::Builder::default()
        .manage(Vault { index: Mutex::new(index), root: Mutex::new(init) })
        .manage(ZoomLevel(Mutex::new(read_zoom_cfg())))
        .setup(|app| {
            spawn_watcher(app.handle().clone());
            // R33.6b: probe the LIVE session once, HERE — setup runs on the GTK
            // main thread with the display already open, and GDK may not be
            // touched from the command threads where win_drag_start runs.
            {
                use tauri::Manager;
                let proto = probe_drag_proto();
                eprintln!(
                    "[win_drag] session: x11={} _NET_WM_MOVERESIZE={} -> path {}",
                    proto.x11,
                    proto.moveresize,
                    drag_path(proto.x11, proto.moveresize, None)
                );
                app.manage(proto);
            }
            // R36.4 the persisted zoom is applied HERE, before the first paint the
            // user sees, and not from JS: a webview that boots at 100% and is
            // rescaled after the UI script runs shows one frame at the wrong size
            // on every start. Stock persists it too (recon-zoom/clamps.log:
            // ~/.config/obsidian/<vault-id>.json "zoom":1 after two presses in —
            // stock's own file stores the LEVEL, which is also the third
            // independent confirmation that one press is half a level).
            let lvl = read_zoom_cfg();
            if lvl != 0.0 {
                use tauri::Manager;
                if let Some(w) = app.webview_windows().values().next() {
                    let _ = w.set_zoom(zoom_factor(lvl));
                }
            }
            Ok(())
        })
        // R31.1 THE DROP HANDLER — 4 lines of body, no behaviour, no I/O, no policy.
        // wry delivers a real XDND drop to Rust and Tauri hands it to us as
        // WindowEvent::DragDrop (tauri-runtime-wry-2.11.4:4889: for a window's
        // own content webview the drop is a SYNTHESIZED WINDOW event, not a
        // webview one), so the DOM never sees a DataTransfer carrying files —
        // faking a DOM drop in JS would test a path production does not have.
        // Which note is open and where the caret sits is webview state, so this
        // re-emits the paths under ONE name the UI owns (`drop-files`) and
        // stops: every rule — source checks, name checks, the cap, containment,
        // no-overwrite — lives in `attach_drop`, reached through `attach_files`.
        // Caveat, stated because it is real: `emit` serializes to JSON, so a
        // source path whose bytes are not UTF-8 cannot be carried. It is then
        // dropped here and the user sees nothing — the same hole Tauri's own
        // `tauri://drag-drop` payload has. Such a file is never attached, never
        // half-attached; R31.10.
        .on_window_event(|w, e| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = e {
                use tauri::Emitter;
                let _ = w.emit(DROP_EVENT, paths);
            }
        })
        // S1: the window is the app, never a browser — every navigation off the
        // app origin (remote http(s), javascript:, file:, ...) is denied here.
        // Config-created windows have no builder hook; an inline plugin's
        // on_navigation applies to every webview of the app.
        .plugin(
            tauri::plugin::Builder::<tauri::Wry, ()>::new("navguard")
                .on_navigation(|_, u| u.scheme() == "tauri" || u.host_str() == Some("tauri.localhost"))
                .build(),
        )
        // R29: image bytes reach the webview on our own scheme, so the
        // containment rule is a function we can unit-test (img_path_in) rather
        // than a runtime scope config. Out-of-vault targets get a bodiless 404 —
        // the renderer paints R29.4's "could not be found" banner either way, so
        // an escape is indistinguishable from a typo (R29.5).
        .register_uri_scheme_protocol(IMG_SCHEME, |ctx, req| {
            use tauri::Manager;
            let root = ctx.app_handle().state::<Vault>().root.lock().unwrap().clone();
            let target = req.uri().path().trim_start_matches('/').to_string();
            match root.as_deref().and_then(|r| serve_image(r, &target)) {
                Some((mime, body)) => tauri::http::Response::builder()
                    .status(200)
                    .header("Content-Type", mime)
                    .header("X-Content-Type-Options", "nosniff")
                    .header("Cache-Control", "no-store")
                    .body(body),
                None => tauri::http::Response::builder().status(404).body(Vec::new()),
            }
            .expect("img response")
        })
        .invoke_handler(tauri::generate_handler![
            list_notes, list_images, read_note, write_note, create_note, render, render_blocks, block_lines, highlight_blocks, graph, graph_local, vault_get, set_vault,
            create_vault, home_dir, list_dirs, list_folders, create_dir, backlinks, search,
            list_bookmarks, toggle_bookmark, bookmark_rows, bm_group_new, bm_group_rename, bm_group_delete, bm_move, bm_drag, recent_vaults, rename_note, move_note, update_links, delete_note, link_consent, set_link_consent, tags, tag_counts,
            get_sidebar_w, set_sidebar_w, log_spans, graph_renderer_pref, type_probe, smoke_css,
            outline, outgoing, backlinks_ctx, unlinked_mentions, link_mention, get_rside_tab, set_rside_tab, get_theme, set_theme, get_palette, set_palette,
            snippets_scan, snippets_enabled, snippet_css, set_snippet_enabled,
            themes_scan, theme_css, get_css_theme, set_css_theme, vault_css_watch,
            get_hotkeys, set_hotkeys, open_external, save_debounce_ms, attach_files,
            win_rect, win_gesture, win_move_proto, win_drag_start, win_minimize, win_toggle_max, win_close,
            tab_removed,
            zoom, zoom_get,
            settings::settings_model
        ])
        .run(tauri::generate_context!())
        .expect("tauri run");
}

#[cfg(test)]
mod tests {
    use super::*;

    /* R36: the numbers the keys land on. These tests are not decoration — the
       step and the two clamps are the whole behavioural content of zoom, and
       all three are MEASURED values (recon-zoom/clamps.log) that a later
       "tidy-up" could round off without any phase noticing until a user hits
       the ceiling. */
    #[test]
    fn zoom_step_is_half_an_electron_level() {
        // one press in = +9.54%, one press out = -8.71% (measured off stock)
        assert!((zoom_factor(zoom_next(0.0, "in").unwrap()) - 1.0954).abs() < 0.0005);
        assert!((zoom_factor(zoom_next(0.0, "out").unwrap()) - 0.9129).abs() < 0.0005);
        // two presses in = exactly one Electron level = 1.2
        assert!((zoom_factor(zoom_next(zoom_next(0.0, "in").unwrap(), "in").unwrap()) - 1.2).abs() < 1e-9);
        assert_eq!(zoom_factor(0.0), 1.0);
    }

    #[test]
    fn zoom_clamps_match_the_measured_stock_range() {
        // ceiling: stock stopped after 6 presses in (level +3.0, 1.728x)
        let mut l = 0.0;
        for _ in 0..12 {
            l = zoom_next(l, "in").unwrap();
        }
        assert_eq!(l, 3.0);
        assert!((zoom_factor(l) - 1.728).abs() < 1e-9);
        // floor: stock stopped after 5 presses out (level -2.5, 0.6339x)
        let mut l = 0.0;
        for _ in 0..12 {
            l = zoom_next(l, "out").unwrap();
        }
        assert_eq!(l, -2.5);
        assert!((zoom_factor(l) - 0.63387).abs() < 0.0001);
    }

    #[test]
    fn zoom_reset_is_the_original_value_not_a_third_one() {
        // N3's shape: reset must land on 1.0 from either side, and from a clamp
        assert_eq!(zoom_next(3.0, "reset").unwrap(), 0.0);
        assert_eq!(zoom_next(-2.5, "reset").unwrap(), 0.0);
        assert_eq!(zoom_factor(zoom_next(1.5, "reset").unwrap()), 1.0);
    }

    #[test]
    fn zoom_rejects_an_unknown_action() {
        assert!(zoom_next(0.0, "bigger").is_none());
        assert!(zoom_next(0.0, "").is_none());
    }

    #[test]
    fn block_lines_maps_every_top_level_block_to_its_source_line() {
        // one of each shape the scroll fixture uses. The mapping is what the
        // reading view scrolls by, so it is asserted as EXACT line numbers,
        // and its LENGTH must equal the number of top-level elements the
        // renderer emits — one slot per #preview child, or the index mapping
        // in ui/main.js silently points at the wrong block.
        let md = "# H1\n\npara one\nstill one\n\n## H2\n\n- a\n- b\n\n> quote\n\n```\ncode\n```\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\ntail\n";
        assert_eq!(block_lines_of(md), vec![1, 3, 6, 8, 11, 13, 17, 21]);
        let html = render_md(md, &[]);
        let tops = ["<h1", "<p>", "<h2", "<ul>", "<blockquote>", "<pre>", "<table>"];
        for t in tops {
            assert!(html.contains(t), "renderer no longer emits {t}: {html}");
        }
        // frontmatter renders to nothing and must not consume a slot
        let fm = "---\ntitle: x\n---\n\npara\n";
        assert_eq!(block_lines_of(fm), vec![5]);
        // a line-1 start is line 1, not line 0 (1-based, like the fixture markers)
        assert_eq!(block_lines_of("only\n"), vec![1]);
        assert_eq!(block_lines_of(""), Vec::<u32>::new());
    }

    #[test]
    fn hotkeys_clean_keeps_shape_drops_garbage() {
        let v = serde_json::json!({
            "graph:open-local": [{"modifiers": ["Mod"], "key": "G"}],
            "editor:delete-paragraph": [],
            "bad-chord": [{"modifiers": "Mod", "key": "X"}],
            "no-key": [{"modifiers": []}],
            "not-array": "ctrl+x"
        });
        let c = hotkeys_clean(v);
        let o = c.as_object().unwrap();
        assert_eq!(o.len(), 2);
        assert_eq!(o["graph:open-local"][0]["key"], "G");
        assert_eq!(o["editor:delete-paragraph"].as_array().unwrap().len(), 0);
        assert_eq!(hotkeys_clean(serde_json::json!(null)), serde_json::json!({}));
    }

    /// (name, content) docs -> the (name, content, tags) stream search wants;
    /// tags empty here — tag queries are tested through a real Index
    fn docs_ref(d: &[(String, String)]) -> Vec<(&str, &str, &[String])> {
        d.iter().map(|(n, c)| (n.as_str(), c.as_str(), &[][..])).collect()
    }

    /// graph from (name, content) docs — what the index feeds build_graph
    fn graph_of(d: &[(String, String)]) -> Graph {
        let names: Vec<String> = d.iter().map(|(n, _)| n.clone()).collect();
        let links: Vec<Vec<String>> = d.iter().map(|(_, c)| links_in(c)).collect();
        let refs: Vec<&[String]> = links.iter().map(|l| l.as_slice()).collect();
        index::build_graph(&names, &refs)
    }

    fn tmp_vault(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rustidian-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sub")).unwrap();
        root
    }

    fn ix_graph(ix: &Index) -> Graph {
        index::build_graph(ix.names(), &ix.link_lists())
    }

    fn edge_names(g: &Graph) -> Vec<(String, String)> {
        let mut e: Vec<(String, String)> = g
            .edges
            .iter()
            .map(|&(a, b)| (g.nodes[a].name.clone(), g.nodes[b].name.clone()))
            .collect();
        e.sort();
        e
    }

    #[test]
    fn link_parts_split_note_anchor_alias() {
        use index::link_parts;
        assert_eq!(link_parts("Note"), ("Note", "", ""));
        assert_eq!(link_parts("Note#Heading"), ("Note", "#Heading", ""));
        assert_eq!(link_parts("Note#^blk1"), ("Note", "#^blk1", ""));
        assert_eq!(link_parts("Note|nick"), ("Note", "", "nick"));
        assert_eq!(link_parts("Note#H|nick"), ("Note", "#H", "nick"));
        assert_eq!(link_parts("#Local"), ("", "#Local", ""));
        assert_eq!(link_parts("sub/N#a|b|c"), ("sub/N", "#a", "b|c"));
        let notes = ["A".to_string(), "sub/B".to_string()];
        assert_eq!(resolve(&notes, "A#h"), Some(0));
        assert_eq!(resolve(&notes, "B#^id|alias"), Some(1));
        assert_eq!(resolve(&notes, "A|alias"), Some(0));
        assert_eq!(resolve(&notes, "#h"), None);
        assert_eq!(resolve(&notes, "Ghost#h"), None);
    }

    #[test]
    fn backlinks_and_graph_resolve_through_suffixes() {
        let root = tmp_vault("sfx");
        fs::write(root.join("T.md"), "# H\ntext ^blk").unwrap();
        fs::write(root.join("A.md"), "[[T#H]] [[T#^blk|nick]] [[#Own]]").unwrap();
        fs::write(root.join("sub/B.md"), "[[T|alias]] [[Ghost#H]] [[Ghost|g]]").unwrap();
        let ix = Index::build(&root);
        assert_eq!(ix.backlinks("T"), ["A", "sub/B"]);
        let g = ix_graph(&ix);
        // one ghost node "Ghost" (note part), no node for [[#Own]]
        assert_eq!(g.nodes.len(), 4);
        assert!(!g.nodes[3].resolved && g.nodes[3].name == "Ghost");
        let e = edge_names(&g);
        assert!(e.contains(&("A".to_string(), "T".to_string())));
        assert_eq!(e.iter().filter(|(s, _)| s == "A").count(), 1);
        assert_eq!(e.iter().filter(|(s, _)| s == "sub/B").count(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_rewrites_anchor_and_alias_links() {
        use index::rewrite_links;
        assert_eq!(
            rewrite_links("[[Old#h|alias]] [[Old#^b]] [[Old|a|b]] [[Older]]", "Old", "New", true),
            ("[[New#h|alias]] [[New#^b]] [[New|a|b]] [[Older]]".to_string(), true)
        );
        let root = tmp_vault("ra");
        fs::write(root.join("Old.md"), "# h").unwrap();
        fs::write(root.join("L.md"), "x [[Old#h|alias]] y").unwrap();
        let mut ix = Index::build(&root);
        rename_in(&root, &mut ix, "Old", "New").unwrap();
        assert_eq!(fs::read_to_string(root.join("L.md")).unwrap(), "x [[New#h|alias]] y");
        assert_eq!(ix.backlinks("New"), ["L"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn index_builds_from_temp_vault() {
        let root = tmp_vault("ix");
        fs::write(root.join("A.md"), "[[B]] and [[Nested]] and [[Ghost]]").unwrap();
        fs::write(root.join("B.md"), "back to [[A]] self [[B]]").unwrap();
        fs::write(root.join("sub/Nested.md"), "[[A]] twice [[A]]").unwrap();
        fs::write(root.join(".hidden.md"), "[[A]]").unwrap(); // dotfiles never indexed
        let ix = Index::build(&root);
        assert_eq!(ix.names(), ["A", "B", "sub/Nested"]);
        assert_eq!(ix.content("B"), Some("back to [[A]] self [[B]]"));
        assert_eq!(ix.links("A"), ["B", "Nested", "Ghost"]);
        // raw tokens are kept in links(); resolve() strips #anchor / |alias (R10)
        assert_eq!(ix.backlinks("A"), ["B", "sub/Nested"]);
        assert_eq!(ix.backlinks("B"), ["A"]); // B's self-link excluded
        assert_eq!(ix.backlinks("sub/Nested"), ["A"]); // basename resolve
        assert!(ix.backlinks("Ghost").is_empty());
        // graph: unresolved [[Ghost]] stays a ghost node
        let g = ix_graph(&ix);
        assert_eq!(g.nodes.len(), 4);
        assert!(!g.nodes[3].resolved && g.nodes[3].name == "Ghost");
        // search sees every body without touching disk: mutate disk behind it
        fs::write(root.join("B.md"), "changed on disk").unwrap();
        let hits = search_docs(ix.docs(), "back to");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].note, "B");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn graph_cache_invalidates_and_local_bfs() {  // R19
        let root = tmp_vault("gc");
        fs::write(root.join("A.md"), "[[B]] [[Ghost]]").unwrap();
        fs::write(root.join("B.md"), "[[C]]").unwrap();
        fs::write(root.join("C.md"), "[[A]]").unwrap();
        fs::write(root.join("D.md"), "[[C]]").unwrap();
        let mut ix = Index::build(&root);
        // cached graph == the from-scratch build
        let fresh = ix_graph(&ix);
        assert_eq!(ix.graph().graph, fresh);
        assert_eq!(ix.graph().out[0], [1, 4]); // A -> B, Ghost
        assert_eq!(ix.graph().inc[2], [1, 3]); // B, D -> C
        // local: depth 1 out+in around B = B, C, A; edges among them keep every direction
        let l = ix.graph().local("B", 1, true, true);
        assert_eq!(l.nodes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["B", "C", "A"]);
        assert_eq!(edge_names(&l).len(), 3); // A->B, B->C, C->A
        let l = ix.graph().local("B", 1, false, true); // outgoing only
        assert_eq!(l.nodes.len(), 2);
        let l = ix.graph().local("B", 2, true, true); // 2 hops reach D + Ghost
        assert_eq!(l.nodes.len(), 5);
        assert!(ix.graph().local("Nope", 1, true, true).nodes.is_empty());
        // plain save (links unchanged) keeps the cache; a link edit / new note / remove rebuilds it
        ix.upsert("D", "[[C]] more text");
        assert!(ix.graph.is_some());
        ix.upsert("D", "[[A]]");
        assert!(ix.graph.is_none());
        assert_eq!(ix.graph().inc[0], [2, 3]);
        ix.upsert("Ghost", "");
        assert!(ix.graph().graph.nodes.iter().all(|n| n.resolved));
        ix.remove("Ghost");
        assert!(!ix.graph().graph.nodes.iter().all(|n| n.resolved));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_updates_search_backlinks_graph() {
        let root = tmp_vault("ixw");
        fs::write(root.join("A.md"), "[[B]] [[Ghost]]").unwrap();
        fs::write(root.join("B.md"), "plain").unwrap();
        let mut ix = Index::build(&root);
        assert_eq!(ix.backlinks("B"), ["A"]);
        // edit A: drop the B link, add a C link (C absent -> unresolved)
        ix.upsert("A", "now [[C]] only, needle here");
        assert!(ix.backlinks("B").is_empty());
        assert_eq!(search_docs(ix.docs(), "needle").len(), 1);
        assert!(search_docs(ix.docs(), "ghost").is_empty());
        let g = ix_graph(&ix);
        assert_eq!(edge_names(&g), [("A".to_string(), "C".to_string())]);
        assert!(g.nodes.iter().any(|n| n.name == "C" && !n.resolved));
        // new note C appears: A's dangling link now resolves, ghost node gone
        ix.upsert("C", "[[A]]");
        assert_eq!(ix.names(), ["A", "B", "C"]);
        assert_eq!(ix.backlinks("C"), ["A"]);
        assert_eq!(ix.backlinks("A"), ["C"]);
        let g = ix_graph(&ix);
        assert!(g.nodes.iter().all(|n| n.resolved));
        assert_eq!(
            edge_names(&g),
            [("A".to_string(), "C".to_string()), ("C".to_string(), "A".to_string())]
        );
        // B links to C too: edge list stays sorted + deduped
        ix.upsert("B", "[[C]] [[C]]");
        assert_eq!(ix.backlinks("C"), ["A", "B"]);
        ix.upsert("B", "");
        assert_eq!(ix.backlinks("C"), ["A"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_moves_keys_edges_and_targets() {
        let root = tmp_vault("ixr");
        fs::write(root.join("Old.md"), "[[B]]").unwrap();
        fs::write(root.join("B.md"), "see [[Old]] and [[Old|nick]]").unwrap();
        fs::write(root.join("sub/C.md"), "[[Old]] [[Ghost]]").unwrap();
        let mut ix = Index::build(&root);
        assert_eq!(ix.backlinks("Old"), ["B", "sub/C"]);
        rename_in(&root, &mut ix, "Old", "sub/New").unwrap();
        // keys moved
        assert_eq!(ix.names(), ["B", "sub/C", "sub/New"]);
        assert!(ix.content("Old").is_none());
        assert_eq!(ix.content("sub/New"), Some("[[B]]"));
        // link targets rewritten in memory AND on disk (exact-path links get
        // the full new path — same rewrite_links rule as before)
        assert_eq!(ix.content("B"), Some("see [[sub/New]] and [[sub/New|nick]]"));
        assert_eq!(fs::read_to_string(root.join("B.md")).unwrap(), "see [[sub/New]] and [[sub/New|nick]]");
        assert_eq!(ix.content("sub/C"), Some("[[sub/New]] [[Ghost]]"));
        assert_eq!(ix.links("sub/C"), ["sub/New", "Ghost"]);
        // edges follow the new key
        assert!(ix.backlinks("Old").is_empty());
        assert_eq!(ix.backlinks("sub/New"), ["B", "sub/C"]);
        assert_eq!(ix.backlinks("B"), ["sub/New"]);
        // unresolved stays unresolved in the graph
        let g = ix_graph(&ix);
        assert!(g.nodes.iter().any(|n| n.name == "Ghost" && !n.resolved));
        assert!(!g.nodes.iter().any(|n| n.name == "Old"));
        // index == disk after the whole dance
        let fresh = Index::build(&root);
        assert_eq!(fresh.names(), ix.names());
        for n in ix.names() {
            assert_eq!(fresh.content(n), ix.content(n), "{n}");
            assert_eq!(fresh.backlinks(n), ix.backlinks(n), "{n}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_resyncs_externally_added_notes() {
        // smoke.sh fast ux seeds a linker note ON DISK after boot and expects
        // the rename to rewrite it: rename resyncs the index from disk first
        let root = tmp_vault("ixx");
        fs::write(root.join("Old.md"), "# Old").unwrap();
        let mut ix = Index::build(&root);
        assert_eq!(ix.names(), ["Old"]);
        fs::write(root.join("Linker.md"), "see [[Old]] and [[Old|nick]] here").unwrap();
        assert!(ix.content("Linker").is_none()); // invisible until the resync
        rename_in(&root, &mut ix, "Old", "New").unwrap();
        assert_eq!(ix.names(), ["Linker", "New"]);
        assert_eq!(
            fs::read_to_string(root.join("Linker.md")).unwrap(),
            "see [[New]] and [[New|nick]] here"
        );
        assert_eq!(ix.backlinks("New"), ["Linker"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn render_link_forms_and_block_ids() {
        let notes = ["T".to_string()];
        // LP: raw target keeps '#', alias-only, [[#H]] drops '#'; data-note = note part
        let h = render_md("[[T#Gamma]] [[T#^b1|nick]] [[#Local]] [[Ghost#H]]", &notes);
        assert!(h.contains(r#"class="wiki" data-note="T" data-anchor="Gamma">T#Gamma</a>"#), "{h}");
        assert!(h.contains(r#"class="wiki" data-note="T" data-anchor="^b1">nick</a>"#), "{h}");
        assert!(h.contains(r#"class="wiki" data-note="" data-anchor="Local">Local</a>"#), "{h}");
        assert!(h.contains(r#"class="wiki wiki-unresolved" data-note="Ghost" data-anchor="H">Ghost#H</a>"#), "{h}");
        // reading: " > " separator, alias unchanged
        let r = render_with("[[T#Gamma]] [[T#^b1|nick]] [[#Local]]", &notes, &[], true);
        assert!(r.contains(">T &gt; Gamma</a>"), "{r}");
        assert!(r.contains(">nick</a>") && r.contains(">Local</a>"), "{r}");
        // block ids: paragraph / list item / heading tails become span.blockid;
        // mid-line ^x and code stay literal
        let b = render_md("para text ^blk1\n\n- item ^i-2\n\n# Head ^h3\n\nnot ^mid here\n\n`code ^c`", &notes);
        assert!(b.contains(r#"para text<span class="blockid" data-bid="blk1">^blk1</span></p>"#), "{b}");
        assert!(b.contains(r#"item<span class="blockid" data-bid="i-2">^i-2</span></li>"#), "{b}");
        assert!(b.contains(r#"Head<span class="blockid" data-bid="h3">^h3</span></h1>"#), "{b}");
        assert!(b.contains("not ^mid here</p>") && b.contains("code ^c</code>"), "{b}");
        assert_eq!(split_block_id("x ^bad id"), None);
        assert_eq!(split_block_id("x ^ok-1  "), Some(("x", "ok-1")));
    }

    #[test]
    fn unlinked_mentions_detected_and_linked() {
        use index::{link_mention, mentions_in};
        let c = "Link Target once, link target twice\nalready [[Link Target]] here but Link Target too\n```\nLink Target in code\n```\nno hit\n";
        assert_eq!(
            mentions_in(c, "Link Target"),
            [(0, 0, 11), (0, 18, 11), (1, 33, 11)]
        );
        assert!(mentions_in(c, "").is_empty());
        // Link button: wrap exactly the matched text, case preserved
        let nc = link_mention(c, "Link Target", 0, 18, 11).unwrap();
        assert!(nc.starts_with("Link Target once, [[link target]] twice\n"), "{nc}");
        assert_eq!(mentions_in(&nc, "Link Target").len(), 2);
        assert!(link_mention(c, "Link Target", 0, 5, 11).is_none()); // stale offsets
        // through the index (basename of a nested note)
        let root = tmp_vault("um");
        fs::write(root.join("sub/Deep Note.md"), "# t").unwrap();
        fs::write(root.join("A.md"), "see deep note and [[Deep Note]]").unwrap();
        let ix = Index::build(&root);
        let hits: Vec<_> = ix.docs().flat_map(|(n, c, _)| mentions_in(c, "Deep Note").into_iter().map(move |h| (n.to_string(), h))).collect();
        assert_eq!(hits, [("A".to_string(), (0, 4, 9))]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn render_marks_unresolved() {
        let notes = vec!["Ideas".to_string(), "sub/Nested".to_string()];
        let h = render_md("[[Ideas]] [[Nested]] [[Nope]]", &notes);
        assert!(h.contains(r#"class="wiki" data-note="Ideas""#));
        assert!(h.contains(r#"class="wiki" data-note="Nested""#)); // basename resolve
        assert!(h.contains(r#"class="wiki wiki-unresolved" data-note="Nope""#));
    }

    #[test]
    fn render_neutralizes_raw_html() {
        // H1 (docs/security-review.md): raw/inline HTML must render inert
        let h = render_md("hi <img src=x onerror=alert(1)> there", &[]);
        assert!(!h.contains("<img"), "raw inline html leaked: {h}");
        assert!(h.contains("&lt;img"));
        let h = render_md("<script>alert(1)</script>", &[]);
        assert!(!h.contains("<script"), "html block leaked: {h}");
        // ...but code blocks still render their (escaped) content normally
        let h = render_md("```\n<b>code</b>\n```", &[]);
        assert!(h.contains("<pre><code>") && h.contains("&lt;b&gt;"));
    }

    #[test]
    fn render_keeps_http_mailto_anchors() {
        // S1: allowed schemes -> our own anchor: class ext, rel noopener
        let h = render_md("[ext](https://example.com/a?b=1&c=2) [m](mailto:a@b.c) [h](HTTP://x.y)", &[]);
        assert!(h.contains(r#"<a href="https://example.com/a?b=1&amp;c=2" class="ext" rel="noopener">ext</a>"#), "{h}");
        assert!(h.contains(r#"<a href="mailto:a@b.c" class="ext" rel="noopener">m</a>"#), "{h}");
        assert!(h.contains(r#"<a href="HTTP://x.y" class="ext" rel="noopener">h</a>"#), "{h}");
        // autolink + title survive too
        let h = render_md(r#"<https://a.b> [t](https://c.d "Ti")"#, &[]);
        assert!(h.contains(r#"class="ext" rel="noopener">https://a.b</a>"#), "{h}");
        assert!(h.contains(r#"rel="noopener" title="Ti">t</a>"#), "{h}");
        // reading mode identical
        assert!(render_with("[x](https://q)", &[], &[], true).contains(r#"class="ext""#));
        // R29.10 (operator decision, 2026-09-12): a REMOTE image mints NOTHING.
        // The same URL as a LINK still works — that is the point of scoping the
        // refusal to images, and extlink asserts it end to end. The <img> that
        // used to be asserted here is now the mutation of
        // docs/negctl-remote-img/; the rule lives in
        // remote_images_never_mint_an_img_element.
        let h = render_md("![pic](https://img.x/a.png)", &[]);
        assert!(!h.contains("<img"), "a remote image minted an element: {h}");
        assert!(!h.contains("img.x"), "a remote URL reached the DOM: {h}");
        assert!(!h.contains(IMG_SCHEME), "remote image must not ride our scheme: {h}");
    }

    /// R29.6 — what the index calls a vault image. This list is what BOTH
    /// renderers resolve against, so anything wrongly in it is a URL the
    /// webview will ask for: hidden dirs, symlinks and non-image extensions
    /// must stay out, and the extension set must match the byte server's.
    #[test]
    fn index_lists_vault_images_only() {
        use std::os::unix::fs::symlink;
        // the renderer's allowlist and the byte server's table cannot drift:
        // a name the index resolves but serve_image refuses is a broken image
        let served: Vec<&str> = IMG_TYPES.iter().map(|(e, _)| *e).collect();
        assert_eq!(served, index::IMG_EXTS.to_vec(), "IMG_TYPES vs index::IMG_EXTS");
        let root = tmp_vault("imgidx");
        let outside = std::env::temp_dir().join(format!("rustidian-imgout-{}", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.png"), b"out").unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        for (p, b) in [
            ("A.md", "note"),
            ("pic.png", "x"),
            ("UP.JPG", "x"),
            ("sub/nested.webp", "x"),
            (".hidden/h.png", "x"),
            ("doc.svg", "x"),
            ("notes.txt", "x"),
        ] {
            fs::write(root.join(p), b).unwrap();
        }
        symlink(outside.join("secret.png"), root.join("link.png")).unwrap();
        let ix = Index::build(&root);
        assert_eq!(ix.images(), ["UP.JPG", "pic.png", "sub/nested.webp"], "{:?}", ix.images());
        assert_eq!(ix.names(), ["A"], "images must never become notes");
        let _ = fs::remove_dir_all(&outside);
    }

    /// the two halves must MEET: whatever url the renderer emits, the byte
    /// server must serve — and only that. A pct_encode/pct_decode mismatch
    /// would show up here as a broken image in the app and nowhere else.
    #[test]
    fn rendered_img_url_round_trips_through_the_byte_server() {
        let root = tmp_vault("imgtrip");
        fs::create_dir_all(root.join("sub")).unwrap();
        let png = b"\x89PNG\r\n\x1a\n-solid";
        for p in ["pic.png", "my pic.png", "sub/nested.webp"] {
            fs::write(root.join(p), png).unwrap();
        }
        let ix = Index::build(&root);
        for md in ["![](pic.png)", "![[pic.png]]", "![](my%20pic.png)", "![[my pic.png]]", "![[nested.webp]]"] {
            let h = render_with(md, &[], ix.images(), true);
            let i = h.find("src=\"").unwrap_or_else(|| panic!("no img for {md}: {h}")) + 5;
            let url = h[i..].split('"').next().unwrap();
            let target = url.strip_prefix(&format!("{IMG_SCHEME}://localhost/")).expect(url);
            // exactly the path the protocol handler passes to serve_image
            let (mime, bytes) = serve_image(&root, target).unwrap_or_else(|| panic!("{md} -> {url} served nothing"));
            assert_eq!(bytes, png, "{md}");
            assert!(mime.starts_with("image/"), "{md}: {mime}");
        }
    }

    /// R29.1/R29.2/R29.3/R29.4/R29.5 — the READING-MODE renderer. Both syntaxes
    /// go through ONE resolver (index::resolve over the index's image list), so
    /// what they cannot resolve they cannot emit a URL for.
    #[test]
    fn render_images_resolve_through_the_index() {
        let imgs: Vec<String> = ["pic.png", "sub/nested.png", "my pic.png", "q\"t.png", "up.JPG"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let src = |h: &str| -> String {
            let i = h.find("src=\"").expect("no src") + 5;
            h[i..].split('"').next().unwrap().to_string()
        };
        // images resolve against the INDEX IMAGE LIST, not the note list
        let rmd = |c: &str| render_with(c, &[], &imgs, false);
        // R29.2 relative path, alt preserved and escaped
        let h = rmd("![a pic](pic.png)");
        assert_eq!(src(&h), format!("{IMG_SCHEME}://localhost/pic.png"), "{h}");
        assert!(h.contains(r#"alt="a pic""#), "{h}");
        // subfolder: full path AND bare basename resolve to the same file
        let full = rmd("![](sub/nested.png)");
        let bare = rmd("![](nested.png)");
        assert_eq!(src(&full), format!("{IMG_SCHEME}://localhost/sub/nested.png"), "{full}");
        assert_eq!(src(&bare), src(&full), "basename must resolve like the path");
        // R29.1: the wikilink embed form agrees with the markdown form, exactly
        assert_eq!(src(&rmd("![[pic.png]]")), src(&rmd("![](pic.png)")));
        assert_eq!(src(&rmd("![[nested.png]]")), src(&full));
        assert!(!rmd("![[pic.png]]").contains('!'), "bang leaked as text");
        // R29.3: %20 resolves and comes back percent-ENCODED in the url
        let h = rmd("![](my%20pic.png)");
        assert_eq!(src(&h), format!("{IMG_SCHEME}://localhost/my%20pic.png"), "{h}");
        // R29.3 second half: a RAW space is not an image at all (pulldown), text
        let h = rmd("![](my pic.png)");
        assert!(!h.contains("<img") && h.contains("![](my pic.png)"), "{h}");
        // case-insensitive extension is still an embed target
        assert!(rmd("![[up.JPG]]").contains("<img"), "ext case");
        // a quote in a vault filename cannot break out of the src attribute
        assert!(src(&rmd("![[q\"t.png]]")).ends_with("q%22t.png"));
        // R29.5: an ESCAPE is indistinguishable from a typo, and serves no url
        for t in [
            "../../../etc/passwd",
            "../outside.png",
            "/etc/passwd",
            "..%2f..%2fpic.png",
            "%2e%2e%2fpic.png",
            ".hidden/pic.png",
            "sub/../../pic.png",
        ] {
            for h in [rmd(&format!("![]({t})")), rmd(&format!("![[{t}]]"))] {
                assert!(!h.contains("<img"), "{t} produced an image: {h}");
                assert!(!h.contains(IMG_SCHEME), "{t} produced a url: {h}");
            }
        }
        // R29.4: missing -> stock's banner, verbatim, and NO element to load
        for m in ["![](nope.png)", "![[nope.png]]"] {
            let h = rmd(m);
            assert!(h.contains("\u{201c}nope.png\u{201d} could not be found."), "{m}: {h}");
            assert!(!h.contains("<img") && !h.contains(IMG_SCHEME), "{m}: {h}");
        }
        // the banner escapes the target it echoes back
        let h = rmd(r#"![[x" onerror="alert(1).png]]"#);
        assert!(!h.contains("onerror=\"alert") && h.contains("&quot;"), "{h}");
        // note embeds are NOT image embeds (requirements.md:410): still a link
        let h = render_md("![[Second Note]]", &["Second Note".to_string()]);
        assert!(h.contains(r#"class="wiki""#) && !h.contains("<img"), "{h}");
        // both Rust modes (reading view, live-preview block batch) agree
        let note = "![[pic.png]] ![](sub/nested.png) ![](nope.png)";
        assert_eq!(
            render_with(note, &[], &imgs, true),
            render_with(note, &[], &imgs, false),
            "reading view and block render disagree"
        );
    }

    #[test]
    fn render_drops_bad_schemes_to_text() {
        // S1: javascript:/data:/file:/vbscript:/relative -> literal text, no anchor at all
        for src in [
            "[js](javascript:alert(1))",
            "[d](data:text/html,<b>x</b>)",
            "[f](file:///etc/passwd)",
            "[v](vbscript:msgbox)",
            "[r](../../etc/passwd)",
            "[frag](#h)",
            "[sp]( javascript:alert(1))",
            "[tab](java\tscript:alert(1))",
            "<javascript:alert(1)>",
        ] {
            let h = render_md(src, &[]);
            assert!(!h.contains("<a "), "anchor leaked for {src}: {h}");
            assert!(!h.contains("href"), "href leaked for {src}: {h}");
        }
        let h = render_md("[js](javascript:alert(1))", &[]);
        assert!(h.contains("[js](javascript:alert(1))"), "literal expected: {h}");
        let h = render_md("![x](file:///etc/hostname) ![y](javascript:alert(1)) ![z](data:image/png;base64,AAAA)", &[]);
        assert!(!h.contains("<img"), "img leaked: {h}");
        assert!(h.contains("![x](file:///etc/hostname)"), "{h}");
        // label text still escaped (push_html Text), url text too
        let h = render_md("[<b>](javascript:'<s>')", &[]);
        assert!(!h.contains("<b>") && !h.contains("<s>"), "{h}");
        // wikilinks / tags unaffected
        let h = render_md("[[Ideas]] #tag [x](javascript:1)", &["Ideas".into()]);
        assert!(h.contains(r#"class="wiki""#) && h.contains(r#"class="tag""#), "{h}");
    }

    #[test]
    fn open_external_rejects_non_http() {
        for u in ["javascript:alert(1)", "file:///etc/passwd", "data:text/html,x", "vbscript:x", "../x", "#h", "", "ftp://a.b"] {
            assert!(check_external(u).is_err(), "accepted {u}");
            assert!(open_external(u.to_string()).is_err(), "command accepted {u}");
        }
        for u in ["https://example.com", "http://a.b/c", "mailto:x@y.z", "HTTPS://A.B"] {
            assert!(check_external(u).is_ok(), "rejected {u}");
        }
        assert_eq!(url_scheme("javascript:x").as_deref(), Some("javascript"));
        assert_eq!(url_scheme("no-scheme/path"), None);
        assert_eq!(url_scheme("1http://x"), None);
        assert!(!ext_ok("mailto:a@b", true)); // images: no scheme at all (R29.10)
        for u in ["https://example.com/x.png", "http://example.com/y.png", "HTTPS://A.B/z.png"] {
            assert!(!ext_ok(u, true), "an image was allowed a remote src: {u}");
            assert!(ext_ok(u, false), "the same url must still open as a LINK: {u}");
        }
    }

    /// R29.10 layer 1 (the renderer). The CSP is layer 2 and is pinned by
    /// csp_is_exactly_this_string_and_no_remote_image_origin — neither test
    /// knows about the other, which is the whole point of two layers.
    ///
    /// MUTATE TO CHECK: restore the remote branch in `ext_ok`
    /// (`matches!(url_scheme(url).as_deref(), Some("http" | "https")) || …`)
    /// and this goes red on `<img src="https://example.com/x.png">`.
    #[test]
    fn remote_images_never_mint_an_img_element() {
        let md = "![a](https://example.com/x.png)\n\n![b](http://example.com/y.png)\n\n\
                  ![c](HTTPS://EXAMPLE.COM/z.png)\n";
        for reading in [true, false] {
            // the vault DOES hold an x.png: a remote target must not be able to
            // borrow a local file's bytes either (resolve is never reached).
            let h = render_with(md, &[], &["x.png".to_string()], reading);
            let lo = h.to_ascii_lowercase(); // the third target is UPPERCASE on purpose
            assert!(!h.contains("<img"), "reading={reading}: a remote target minted an <img>: {h}");
            assert!(!lo.contains("http"), "reading={reading}: a remote URL reached the DOM: {h}");
            assert!(!lo.contains("example.com"), "reading={reading}: the host leaked: {h}");
            assert_eq!(
                h.matches("class=\"imgmiss\"").count(),
                3,
                "reading={reading}: want R29.4's banner for each remote image: {h}"
            );
            assert!(
                h.contains("\u{201c}remote image\u{201d} could not be found."),
                "reading={reading}: the banner is not R29.4's wording: {h}"
            );
            // S4/extlink is NOT affected: the same url is still a working link.
            let l = render_with("[text](https://example.com/x.png)", &[], &[], reading);
            assert!(
                l.contains("<a href=\"https://example.com/x.png\" class=\"ext\""),
                "reading={reading}: a non-image external link stopped working: {l}"
            );
        }
        // S1 unchanged: any other scheme is still literal text, not a banner
        // (smoke `imgsec`/ZJ-File asserts the census publishes no [xi:] token).
        let f = render_with("![d](file:///etc/passwd)", &[], &[], true);
        assert!(!f.contains("imgmiss") && !f.contains("<img"), "file:// image changed shape: {f}");
        assert!(f.contains("![d](file:///etc/passwd)"), "file:// image is no longer literal text: {f}");
    }

    #[test]
    fn render_escapes_wikilink_attr() {
        // attribute breakout: [[x" onmouseover=...]] must stay inside data-note
        let h = render_md(r#"[[x" onmouseover="alert(1)]]"#, &[]);
        assert!(!h.contains(r#"" onmouseover="#), "attr breakout: {h}");
        assert!(h.contains("data-note=\"x&quot; onmouseover=&quot;alert(1)\""));
        // wikilinks inside code blocks are NOT linkified
        let h = render_md("```\n[[Ideas]]\n```", &["Ideas".to_string()]);
        assert!(!h.contains("class=\"wiki\""));
    }

    #[test]
    fn render_tags_become_pills() {
        let h = render_md("see #alpha and #beta/gamma here", &[]);
        assert!(h.contains(r##"<a href="#" class="tag" data-tag="alpha">#alpha</a>"##), "{h}");
        assert!(h.contains(r##"<a href="#" class="tag" data-tag="beta/gamma">#beta/gamma</a>"##), "{h}");
        // tag next to a wikilink: both anchors, text between preserved
        let h = render_md("[[Ideas]] #x", &["Ideas".to_string()]);
        assert!(h.contains("class=\"wiki\"") && h.contains("data-tag=\"x\""), "{h}");
        // never inside fenced blocks, code spans or frontmatter
        let h = render_md("```\n#code\n```\n`#span`\n\n---\ntags: [fm]\n---\n", &[]);
        assert!(!h.contains("class=\"tag\""), "{h}");
        let h = render_md("---\ntags: #fm\n---\nbody", &[]);
        assert!(!h.contains("class=\"tag\""), "{h}");
        // URL fragments and headings are not tags
        let h = render_md("https://x/y#frag and <https://x/y#frag>\n\n# Heading", &[]);
        assert!(!h.contains("class=\"tag\""), "{h}");
        // no raw html leak via tags: '#' followed by '<' is not a tag, and
        // the '<img>' is escaped by push_html like any text
        let h = render_md("#<img src=x onerror=alert(1)> #a<img>", &[]);
        assert!(!h.contains("<img"), "raw html leaked: {h}");
        assert!(h.contains("data-tag=\"a\">#a</a>&lt;img&gt;"), "{h}");
    }

    #[test]
    fn tags_extraction_edge_cases() {
        use index::tags_in;
        assert_eq!(tags_in("plain #alpha text"), ["alpha"]);
        assert_eq!(tags_in("#nested/tag and #nested/tag again"), ["nested/tag"]);
        assert_eq!(tags_in("#a #b\n#a"), ["a", "b"]);
        assert_eq!(tags_in("#_under #dash-ed #digits2 #ünï"), ["_under", "dash-ed", "digits2", "ünï"]);
        // must start with a letter or '_'; punctuation/headings/bare '#'
        assert!(tags_in("#1 #123 # Heading\n# H1\n## H2 #").is_empty());
        // trailing '/' trimmed, trailing punctuation not part of the tag
        assert_eq!(tags_in("#a/ (#b), #c."), ["a", "c"]);
        // fenced code (both fences) and inline code spans
        assert_eq!(tags_in("```\n#no\n```\n#yes\n~~~\n#no2\n~~~\n  ```rust\n  #no3\n  ```\n"), ["yes"]);
        assert_eq!(tags_in("`#no` #yes `x #no2 y`"), ["yes"]);
        // URL fragments / mid-word '#' / wikilink anchors
        assert!(tags_in("https://x/y#frag foo#bar [[Note#head]] [[#head]]").is_empty());
        // frontmatter: YAML list, comma string, inline list, block list, tag: key
        assert_eq!(tags_in("---\ntags: [one, \"two\", '#three']\n---\n"), ["one", "three", "two"]);
        assert_eq!(tags_in("---\ntags: one, two/sub\n---\nbody"), ["one", "two/sub"]);
        assert_eq!(tags_in("---\ntitle: x\ntags:\n  - one\n  - two\nother: y\n---\n#body"), ["body", "one", "two"]);
        assert_eq!(tags_in("---\ntag: solo\n---\n"), ["solo"]);
        // frontmatter must open on line 1 and close; unclosed = plain body
        assert!(tags_in("text\n---\ntags: [x]\n---\n").is_empty());
        assert!(tags_in("---\ntags: [x]\n").is_empty());
    }

    #[test]
    fn index_tags_counts_search_and_sync() {
        let root = tmp_vault("ixt");
        fs::write(root.join("A.md"), "---\ntags: [fm, shared]\n---\n#alpha #beta/gamma").unwrap();
        fs::write(root.join("B.md"), "#shared and #beta/delta").unwrap();
        fs::write(root.join("sub/C.md"), "no tags").unwrap();
        let mut ix = Index::build(&root);
        assert_eq!(ix.tags("A"), ["alpha", "beta/gamma", "fm", "shared"]);
        assert_eq!(ix.tags("B"), ["beta/delta", "shared"]);
        assert!(ix.tags("sub/C").is_empty() && ix.tags("Nope").is_empty());
        let tc = ix.tag_counts();
        let got: Vec<(&str, usize)> = tc.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        assert_eq!(got, [("alpha", 1), ("beta/delta", 1), ("beta/gamma", 1), ("fm", 1), ("shared", 2)]);
        // search: tag: filter, with/without '#', case-insensitive, nested prefix
        let notes = |ix: &Index, q: &str| {
            let mut v: Vec<String> = search_docs(ix.docs(), q).into_iter().map(|h| h.note).collect();
            v.dedup();
            v
        };
        assert_eq!(notes(&ix, "tag:shared"), ["A", "B"]);
        assert_eq!(notes(&ix, "tag:#alpha"), ["A"]);
        assert_eq!(notes(&ix, "tag:ALPHA"), ["A"]);
        assert_eq!(notes(&ix, "tag:beta"), ["A", "B"]);          // prefix hits both nested tags
        assert_eq!(notes(&ix, "tag:beta/gamma"), ["A"]);
        assert!(notes(&ix, "tag:bet").is_empty());               // prefix is per segment
        assert_eq!(notes(&ix, "tag:fm"), ["A"]);                 // frontmatter-only -> name hit
        assert_eq!(notes(&ix, "tag:shared delta"), ["B"]);       // tag filter + text
        let h = search_docs(ix.docs(), "tag:alpha");
        assert_eq!((h[0].line, h[0].snippet.as_str()), (3, "#alpha #beta/gamma"));
        // upsert keeps tags + counts in sync (existing key and new key)
        ix.upsert("B", "#shared only");
        assert_eq!(ix.tags("B"), ["shared"]);
        assert!(!ix.tag_counts().contains_key("beta/delta"));
        ix.upsert("D", "#fresh");
        assert_eq!(ix.tag_counts()["fresh"], 1);
        assert_eq!(notes(&ix, "tag:fresh"), ["D"]);
        // rename moves the tags with the key
        ix.rename("A", "sub/Z", None);
        assert!(ix.tags("A").is_empty());
        assert_eq!(ix.tags("sub/Z"), ["alpha", "beta/gamma", "fm", "shared"]);
        assert_eq!(ix.tag_counts()["shared"], 2);
        assert_eq!(notes(&ix, "tag:alpha"), ["sub/Z"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn graph_emits_unresolved_nodes() {
        let docs = vec![
            ("A".to_string(), "[[B]] [[Ghost]] [[Ghost]]".to_string()),
            ("B".to_string(), "[[Ghost]] [[sub/C]]".to_string()),
            ("sub/C".to_string(), String::new()),
        ];
        let g = graph_of(&docs);
        // 3 real notes + 1 deduped unresolved
        assert_eq!(g.nodes.len(), 4);
        assert!(g.nodes[..3].iter().all(|n| n.resolved));
        assert_eq!(g.nodes[3].name, "Ghost");
        assert!(!g.nodes[3].resolved);
        // A->B, A->Ghost (deduped), B->Ghost, B->sub/C
        assert_eq!(g.edges, vec![(0, 1), (0, 3), (1, 3), (1, 2)]);
    }

    #[test]
    fn search_finds_case_insensitive_hits() {
        let docs = vec![
            ("Alpha".to_string(), "first LINE here\nsecond alpha line".to_string()),
            ("Beta".to_string(), "nothing\nAlPhA again".to_string()),
        ];
        let hits = search_docs(docs_ref(&docs), "alpha");
        // name hit (Alpha@0) + body hit in Alpha line 1 + body hit in Beta line 1
        assert_eq!(hits.len(), 3);
        assert_eq!((hits[0].note.as_str(), hits[0].line, hits[0].snippet.as_str()),
                   ("Alpha", 0, "Alpha"));
        assert_eq!((hits[1].note.as_str(), hits[1].line, hits[1].snippet.as_str()),
                   ("Alpha", 1, "second alpha line"));
        assert_eq!((hits[2].note.as_str(), hits[2].line, hits[2].snippet.as_str()),
                   ("Beta", 1, "AlPhA again"));
        assert!(search_docs(docs_ref(&docs), "").is_empty());
        assert!(search_docs(docs_ref(&docs), "zzz").is_empty());
        // long line trims to a window around the hit
        let long = ("L".to_string(), format!("{}needle{}", "x".repeat(300), "y".repeat(300)));
        let h = search_docs(docs_ref(&[long]), "needle");
        assert_eq!(h.len(), 1);
        assert!(h[0].snippet.contains("needle") && h[0].snippet.len() <= 210);
    }

    /// R25.13m: every hit carries the ABSOLUTE UTF-16 offset of the match in
    /// the file as indexed, plus its length, and `content[offset..offset+len]`
    /// IS the match. Measured against stock in docs/recon-srclick C12, where
    /// the jump moved by exactly the characters deleted above it and never
    /// re-found the text — so the frontend must jump by this number and never
    /// search again. The unit is UTF-16 code units because that is how the
    /// frontend indexes the buffer; this test is what makes the difference
    /// between "chars" and "code units" a failure rather than a drift.
    #[test]
    fn search_hits_carry_absolute_utf16_offsets() {
        // the slice a JS `String.prototype.slice(offset, offset+len)` would
        // take, computed in Rust over the same UTF-16 units.
        fn u16slice(s: &str, off: u32, len: u32) -> String {
            let u: Vec<u16> = s.encode_utf16().collect();
            String::from_utf16_lossy(&u[off as usize..(off + len) as usize])
        }

        // ASCII, three lines: the offsets are the byte offsets here, which is
        // exactly why the astral case below is the one that matters.
        let body = "zero\nneedle here\ntail needle\n".to_string();
        let docs = vec![("N".to_string(), body.clone())];
        let h = search_docs(docs_ref(&docs), "needle");
        assert_eq!(h.len(), 2);
        assert_eq!((h[0].line, h[0].offset, h[0].len), (1, 5, 6));
        assert_eq!((h[1].line, h[1].offset, h[1].len), (2, 22, 6));
        for x in &h {
            assert_eq!(u16slice(&body, x.offset, x.len), "needle", "offset must land ON the match");
        }

        // CRLF: lines() drops \r\n, the offset must still count both units.
        let crlf = "zero\r\nneedle\r\n".to_string();
        let h = search_docs(docs_ref(&[("C".to_string(), crlf.clone())]), "needle");
        assert_eq!((h[0].line, h[0].offset, h[0].len), (1, 6, 6));
        assert_eq!(u16slice(&crlf, h[0].offset, h[0].len), "needle");

        // non-ASCII: "é" is 2 BYTES and 1 unit, "𝄞" is 4 bytes, 1 char and
        // 2 UNITS. A byte offset lands mid-character; a char offset drifts by
        // one per astral char. Only the UTF-16 count round-trips.
        let uni = "é𝄞x\nplain é𝄞 needle\n".to_string();
        let h = search_docs(docs_ref(&[("U".to_string(), uni.clone())]), "needle");
        assert_eq!(h.len(), 1);
        assert_eq!((h[0].line, h[0].len), (1, 6));
        // line 0 = é(1) + 𝄞(2) + x(1) + \n(1) = 5; then the line-1 prefix
        // "plain é𝄞 " = p,l,a,i,n,SPACE(6) + é(1) + 𝄞(2) + SPACE(1) = 10 units.
        // 5 + 10 = 15. The literal was 14 until 2026-09-19: the count dropped
        // the space between 𝄞 and the match, and the ONLY reason it was ever
        // believable is that it is a hand-count — which is why the u16slice
        // round-trip below is the assertion that actually decides the rule.
        // It disagreed with the literal (it reads " needl" at 14), and the
        // gate's cargo test is where that disagreement surfaced.
        assert_eq!(h[0].offset, 15);
        assert_eq!(u16slice(&uni, h[0].offset, h[0].len), "needle");

        // case-insensitive match: the offset points at the ORIGINAL text, and
        // len is the matched text's length, not the query's.
        let mixed = "say NeEdLe now\n".to_string();
        let h = search_docs(docs_ref(&[("M".to_string(), mixed.clone())]), "needle");
        assert_eq!((h[0].offset, h[0].len), (4, 6));
        assert_eq!(u16slice(&mixed, h[0].offset, h[0].len), "NeEdLe");

        // a NAME hit has no span in the body: offset 0, len 0 -> open, do not
        // highlight (R25.13a/d). "needle" is in the name AND the body here.
        let named = "needle body\n".to_string();
        let h = search_docs(docs_ref(&[("needle".to_string(), named.clone())]), "needle");
        assert_eq!((h[0].line, h[0].offset, h[0].len), (0, 0, 0), "name hit highlights nothing");
        assert_eq!((h[1].line, h[1].offset, h[1].len), (0, 0, 6), "the body hit still carries its span");

        // a tag-only hit points at the LINE start with len 0 (see search_docs).
        let tagged = "intro\nline with #alpha on it\n".to_string();
        let h = search_docs(
            vec![("T", tagged.as_str(), &["alpha".to_string()][..])],
            "tag:alpha",
        );
        assert_eq!((h[0].line, h[0].offset, h[0].len), (1, 6, 0));

        // the last line without a terminator, and an empty file, do not panic
        // and do not invent a line.
        let tailless = "a\nneedle".to_string();
        let h = search_docs(docs_ref(&[("E".to_string(), tailless.clone())]), "needle");
        assert_eq!((h[0].line, h[0].offset, h[0].len), (1, 2, 6));
        assert_eq!(u16slice(&tailless, h[0].offset, h[0].len), "needle");
        assert!(search_docs(docs_ref(&[("Z".to_string(), String::new())]), "needle").is_empty());
    }

    /// names of the `f` nodes, pre-order — the flat view every older
    /// assertion in this file is written against.
    fn bm_flat(tree: &[BmNode]) -> Vec<String> {
        let mut out = Vec::new();
        bm_names(tree, &mut out);
        out
    }

    #[test]
    fn bookmark_toggle_adds_then_removes() {
        let mut t: Vec<BmNode> = vec![];
        bm_toggle_in(&mut t, "A");
        assert_eq!(bm_flat(&t), vec!["A"]);
        bm_toggle_in(&mut t, "sub/B");           // append keeps insertion order
        assert_eq!(bm_flat(&t), vec!["A", "sub/B"]);
        bm_toggle_in(&mut t, "A");               // second toggle removes
        assert_eq!(bm_flat(&t), vec!["sub/B"]);
        bm_toggle_in(&mut t, "sub/B");
        assert!(bm_flat(&t).is_empty());
        // ...and toggling OFF reaches INSIDE a group, leaving the group there
        let mut g = vec![BmNode::group("Work", vec![BmNode::file("Ideas")])];
        bm_toggle_in(&mut g, "Ideas");
        assert!(bm_flat(&g).is_empty(), "toggled OFF inside the group");
        assert_eq!(bm_rows_of(&g).iter().map(|r| (r.kind.as_str(), r.depth)).collect::<Vec<_>>(),
                   vec![("g", 0)], "...and the group stays");
        bm_toggle_in(&mut g, "Ideas");           // ...and back ON at the TOP level
        assert_eq!(bm_rows_of(&g).iter().map(|r| (r.kind.as_str(), r.depth)).collect::<Vec<_>>(),
                   vec![("g", 0), ("f", 0)]);
    }

    /* ---- R9.8: a renamed or MOVED note takes its bookmark with it ----
       Every test below drives the real functions against a real vault on
       disk, because the thing under test is a FILE (.obsidian/bookmarks.json)
       and half the cases are about bytes that must NOT change. */

    /// seed a vault with a bookmarks file, in toggle_bookmark's exact format
    fn bm_seed(root: &Path, names: &[&str]) {
        let list: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        write_bookmarks(root, &list).unwrap();
    }

    fn bm_bytes(root: &Path) -> Vec<u8> {
        fs::read(root.join(BM_FILE)).unwrap()
    }

    /// R9.8 criterion 1: the new name is there, the old one is gone, and the
    /// entry holds the SAME INDEX — the pane renders insertion order, so a
    /// bookmark that fell to the bottom would be a visible regression.
    #[test]
    fn r9_8_rename_keeps_the_bookmark_at_its_index() {
        let root = tmp_vault("bm-idx");
        fs::write(root.join("A.md"), "a").unwrap();
        fs::write(root.join("Old.md"), "self [[Old]]").unwrap();
        fs::write(root.join("Z.md"), "z").unwrap();
        bm_seed(&root, &["A", "Old", "Z"]);
        assert_eq!(read_bookmarks(&root).iter().position(|b| b == "Old"), Some(1));
        let mut ix = Index::build(&root);
        rename_in(&root, &mut ix, "Old", "New").unwrap();
        let after = read_bookmarks(&root);
        assert!(after.contains(&"New".to_string()), "new name bookmarked: {after:?}");
        assert!(!after.contains(&"Old".to_string()), "old name gone: {after:?}");
        assert_eq!(after.iter().position(|b| b == "New"), Some(1), "same index: {after:?}");
        assert_eq!(after, vec!["A", "New", "Z"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// R9.8 criterion 2: a MOVE into another folder changes the
    /// vault-relative name too, and it goes through move_note_in — the
    /// command's own path, asserted here rather than assumed from the source.
    #[test]
    fn r9_8_move_to_another_folder_keeps_the_bookmark() {
        let root = tmp_vault("bm-move");
        fs::write(root.join("Note.md"), "n").unwrap();
        fs::write(root.join("Keep.md"), "k").unwrap();
        bm_seed(&root, &["Note", "Keep"]);
        let mut ix = Index::build(&root);
        // move_note_in DIRECTLY: this is what the move_note command calls,
        // without rename_in's link rewrite
        move_note_in(&root, &mut ix, "Note", "sub/Note").unwrap();
        assert!(root.join("sub/Note.md").is_file());
        assert_eq!(read_bookmarks(&root), vec!["sub/Note", "Keep"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// R9.8: a note that was NOT bookmarked gains no bookmark.
    #[test]
    fn r9_8_unbookmarked_note_gains_no_bookmark() {
        let root = tmp_vault("bm-none");
        fs::write(root.join("Plain.md"), "p").unwrap();
        fs::write(root.join("Fav.md"), "f").unwrap();
        bm_seed(&root, &["Fav"]);
        let before = bm_bytes(&root);
        let mut ix = Index::build(&root);
        rename_in(&root, &mut ix, "Plain", "Plain2").unwrap();
        assert_eq!(read_bookmarks(&root), vec!["Fav"]);
        assert_eq!(bm_bytes(&root), before, "file not even rewritten");
        let _ = fs::remove_dir_all(&root);
    }

    /// R9.8: renaming onto a name that is ALREADY bookmarked leaves ONE
    /// entry, not two. Reachable because a bookmark can be stale (its note
    /// deleted), so the kernel does not refuse the rename.
    #[test]
    fn r9_8_rename_onto_a_bookmarked_name_leaves_no_duplicate() {
        let root = tmp_vault("bm-dup");
        fs::write(root.join("Old.md"), "o").unwrap();
        // "New" is bookmarked but has NO file — a stale bookmark
        bm_seed(&root, &["Old", "New", "Tail"]);
        let mut ix = Index::build(&root);
        rename_in(&root, &mut ix, "Old", "New").unwrap();
        let after = read_bookmarks(&root);
        assert_eq!(after.iter().filter(|b| *b == "New").count(), 1, "one entry: {after:?}");
        assert_eq!(after, vec!["New", "Tail"], "kept the old entry's position");
        let _ = fs::remove_dir_all(&root);
    }

    /// R9.8: a case-only rename is a rename like any other.
    #[test]
    fn r9_8_case_only_rename_is_followed() {
        let root = tmp_vault("bm-case");
        fs::write(root.join("Note.md"), "n").unwrap();
        bm_seed(&root, &["Note"]);
        let mut ix = Index::build(&root);
        rename_in(&root, &mut ix, "Note", "note").unwrap();
        assert_eq!(read_bookmarks(&root), vec!["note"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// R9.8: every OTHER note's bookmark survives byte for byte.
    #[test]
    fn r9_8_other_bookmarks_are_byte_for_byte_unchanged() {
        let root = tmp_vault("bm-others");
        fs::write(root.join("A.md"), "a").unwrap();
        fs::write(root.join("Old.md"), "o").unwrap();
        fs::write(root.join("sub/Z.md"), "z").unwrap();
        bm_seed(&root, &["A", "Old", "sub/Z"]);
        let before = bm_bytes(&root);
        let mut ix = Index::build(&root);
        rename_in(&root, &mut ix, "Old", "New").unwrap();
        let after = bm_bytes(&root);
        // the ONLY difference is the renamed entry's path value (stock schema:
        // the entry is a JSON object now, not a bare line)
        assert_eq!(
            String::from_utf8(before).unwrap().replace("\"path\": \"Old.md\"", "\"path\": \"New.md\""),
            String::from_utf8(after).unwrap()
        );
        assert_eq!(read_bookmarks(&root), vec!["A", "New", "sub/Z"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// R9.8: a vault with NO bookmarks file does not grow one as a side
    /// effect of a rename. An empty file is a different state from no file —
    /// it is what the pane would read, and it is litter in someone's vault.
    #[test]
    fn r9_8_vault_without_bookmarks_file_gains_none() {
        let root = tmp_vault("bm-absent");
        fs::write(root.join("Old.md"), "o").unwrap();
        assert!(!root.join(BM_FILE).exists());
        let mut ix = Index::build(&root);
        rename_in(&root, &mut ix, "Old", "New").unwrap();
        assert!(!root.join(BM_FILE).exists(), "no bookmarks file materialised");
        let _ = fs::remove_dir_all(&root);
    }

    /// R9.8 criterion 4: a rename REFUSED by the kernel (create_new(2) on a
    /// target that exists) leaves the bookmarks file byte for byte unchanged.
    /// Asserted with a genuinely failed rename, not by reading the source.
    #[test]
    fn r9_8_refused_rename_leaves_bookmarks_unchanged() {
        let root = tmp_vault("bm-refused");
        fs::write(root.join("Old.md"), "o").unwrap();
        fs::write(root.join("Taken.md"), "t").unwrap(); // the collision
        bm_seed(&root, &["Old", "Taken"]);
        let before = bm_bytes(&root);
        let mut ix = Index::build(&root);
        let e = rename_in(&root, &mut ix, "Old", "Taken").unwrap_err();
        assert_eq!(e, "target exists", "the rename really was refused");
        assert!(root.join("Old.md").is_file(), "the note did not move");
        assert_eq!(bm_bytes(&root), before, "bookmarks untouched after a failed rename");
        let _ = fs::remove_dir_all(&root);
    }

    /* ---- R4X: bookmark GROUPS (goal bmfolder, docs/bookmark-groups.md),
       stored in stock's schema since goal bmcompat. The file is the contract:
       the tests below assert on the BYTES or on the painted row vector,
       because criterion 3's smoke phase asserts on the same file from
       shell. */

    /// seed a TREE and read the file straight back: stock 1.13.7's schema
    /// round-trips through the one serializer — key order per type, 2-space
    /// indent, no trailing newline — byte-deterministic under fixed ctimes.
    #[test]
    fn r4x_tree_round_trips_through_the_one_serializer() {
        let root = tmp_vault("bm-tree");
        let x = |ms: u64| BmExtra { ctime: Some(serde_json::Number::from(ms)), keys: Vec::new() };
        let tree = vec![
            BmNode::Group {
                title: "Work".into(),
                items: vec![
                    BmNode::File { name: "Ideas".into(), title: None, x: x(2) },
                    BmNode::Group {
                        title: "Inner".into(),
                        items: vec![BmNode::File { name: "Deep".into(), title: None, x: x(3) }],
                        x: x(4),
                    },
                ],
                x: x(1),
            },
            BmNode::File { name: "Second Note".into(), title: None, x: x(5) },
        ];
        write_bm_tree(&root, &tree).unwrap();
        assert_eq!(
            String::from_utf8(bm_bytes(&root)).unwrap(),
            concat!(
                "{\n",
                "  \"items\": [\n",
                "    {\n",
                "      \"type\": \"group\",\n",
                "      \"ctime\": 1,\n",
                "      \"items\": [\n",
                "        {\n",
                "          \"type\": \"file\",\n",
                "          \"ctime\": 2,\n",
                "          \"path\": \"Ideas.md\"\n",
                "        },\n",
                "        {\n",
                "          \"type\": \"group\",\n",
                "          \"ctime\": 4,\n",
                "          \"items\": [\n",
                "            {\n",
                "              \"type\": \"file\",\n",
                "              \"ctime\": 3,\n",
                "              \"path\": \"Deep.md\"\n",
                "            }\n",
                "          ],\n",
                "          \"title\": \"Inner\"\n",
                "        }\n",
                "      ],\n",
                "      \"title\": \"Work\"\n",
                "    },\n",
                "    {\n",
                "      \"type\": \"file\",\n",
                "      \"ctime\": 5,\n",
                "      \"path\": \"Second Note.md\"\n",
                "    }\n",
                "  ]\n",
                "}"
            ),
            "stock's exact layout: docs/recon-bmcompat captures 26-moved/32-editdone"
        );
        assert_eq!(read_bm_tree(&root), tree, "read(write(t)) == t, ctime included");
        // the derived flat view is the `f` payloads in pre-order, and that is
        // what list_bookmarks returns
        assert_eq!(read_bookmarks(&root), vec!["Ideas", "Deep", "Second Note"]);
        // ...and the painted vector is the same sequence, with depths
        let rows: Vec<(String, usize, String)> =
            bm_rows_of(&tree).into_iter().map(|r| (r.kind, r.depth, r.name)).collect();
        assert_eq!(
            rows,
            vec![
                ("g".to_string(), 0, "Work".to_string()),
                ("f".to_string(), 1, "Ideas".to_string()),
                ("g".to_string(), 1, "Inner".to_string()),
                ("f".to_string(), 2, "Deep".to_string()),
                ("f".to_string(), 0, "Second Note".to_string()),
            ]
        );
        // names the old LINE format had to escape or flatten round-trip
        // VERBATIM now: JSON escaping is not lossy.
        let odd = vec![BmNode::file(":g:not a group"), BmNode::file("tab\there"), BmNode::file("nl\nthere")];
        write_bm_tree(&root, &odd).unwrap();
        assert_eq!(read_bookmarks(&root), vec![":g:not a group", "tab\there", "nl\nthere"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// criterion 5's one sentence, as behaviour: a pre-existing
    /// .rustidian-bookmarks is IGNORED — never read, never written, never
    /// deleted. Bookmarks come from .obsidian/bookmarks.json alone.
    #[test]
    fn r4x_a_pre_existing_rustidian_bookmarks_file_is_ignored() {
        let root = tmp_vault("bm-old-dotfile");
        let v012 = b"Ideas\n:g:Work\n\tA-LP\n";
        fs::write(root.join(".rustidian-bookmarks"), v012).unwrap();
        assert!(read_bookmarks(&root).is_empty(), "the old dotfile is not read");
        let mut t = read_bm_tree(&root);
        bm_toggle_in(&mut t, "Ideas");
        write_bm_tree(&root, &t).unwrap();
        assert_eq!(read_bookmarks(&root), vec!["Ideas"], "bookmarks live in the stock file");
        assert!(root.join(".obsidian/bookmarks.json").is_file());
        assert_eq!(
            fs::read(root.join(".rustidian-bookmarks")).unwrap(),
            v012.to_vec(),
            "...and the old dotfile is byte-for-byte untouched"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// entries our model cannot express — stock's `search`, a non-.md path —
    /// ride through read -> edit -> write VERBATIM, invisible to the pane
    /// (recon §5: dropping someone else's entry is data loss in their app).
    #[test]
    fn r4x_unmodelled_entries_ride_through_a_write_verbatim() {
        let root = tmp_vault("bm-opaque");
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        fs::write(
            root.join(BM_FILE),
            r#"{"items": [
                {"type": "search", "ctime": 1789851600000, "query": "tag:#recon"},
                {"type": "file", "ctime": 7, "path": "Diagram.canvas"},
                {"type": "file", "ctime": 9, "path": "Plain.md"}
            ]}"#,
        )
        .unwrap();
        let t = read_bm_tree(&root);
        // the pane paints ONLY what we model...
        assert_eq!(bm_rows_of(&t).iter().map(|r| r.name.clone()).collect::<Vec<_>>(), vec!["Plain"]);
        // ...row 0 is "Plain" even though two opaque entries precede it on
        // disk: rows never number an opaque, paths stay raw child indices
        assert_eq!(bm_path_of(&t, 0), Some(vec![2]));
        // ...and an edit does not cost the vault the other two entries
        let mut t = t;
        bm_toggle_in(&mut t, "Plain"); // remove the only modelled entry
        write_bm_tree(&root, &t).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bm_bytes(&root)).unwrap();
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), 2, "search + canvas survived: {items:?}");
        assert_eq!(items[0]["query"], "tag:#recon");
        assert_eq!(items[0]["ctime"], serde_json::json!(1789851600000u64), "ctime included");
        assert_eq!(items[1]["path"], "Diagram.canvas");
        let _ = fs::remove_dir_all(&root);
    }

    /// R4X.17 — the painted LABEL follows stock's measured rule
    /// (docs/recon-bmcompat/README.md §5, shots/30-afterinject.png): a file
    /// row is labelled by its `title` when it has one (`ZettelAlpha`, not
    /// `Atomic Notes`), by the basename of the name when it has none
    /// (`Roadmap`, not `Projects/Roadmap`); a group by its title. The input
    /// is the committed pure fixture stock itself wrote, and `name` — the
    /// key a click opens by — keeps the full extensionless path throughout.
    #[test]
    fn r4x_row_labels_follow_stocks_measured_rule() {
        let orig: &str =
            include_str!("../../docs/fixtures/bmcompat/stock-1.13.7-pure.bookmarks.json");
        let t = parse_bm_tree(orig);
        let rows = bm_rows_of(&t);
        assert_eq!(
            rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(),
            vec!["Roadmap", "Work", "ZettelAlpha", "Untitled group"],
            "labels must be what stock paints (30-afterinject.png)"
        );
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["Projects/Roadmap", "Work", "Zettel/Atomic Notes", "Untitled group"],
            "names keep the full extensionless path — the label is only paint"
        );
    }

    /// criterion 4, and it is BYTE-WISE: the input is the committed fixture
    /// stock 1.13.7 itself wrote while CARRYING keys it does not recognise
    /// (docs/fixtures/bmcompat/README.md — capture 32-editdone, sha ea9f2f6e…).
    /// Read it, apply ONE edit through the model, write it back: the file on
    /// disk is the fixture with exactly ONE line changed — the edited title —
    /// so every key the fixture had that our model does not author
    /// (`zzUnknownFile`, `zzUnknownGroup`, the whole `search` entry, and every
    /// `ctime`) is proved present and unchanged by byte equality, not by a
    /// checklist of the keys we DO author.
    #[test]
    fn r4x_criterion4_stock_fixture_unknown_keys_round_trip_byte_wise() {
        let orig: &str =
            include_str!("../../docs/fixtures/bmcompat/stock-1.13.7-unknown-keys.bookmarks.json");
        let root = tmp_vault("bm-c4-fixture");
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        fs::write(root.join(BM_FILE), orig).unwrap();

        // read -> write with NO edit first: the output is byte-identical to
        // what stock wrote, layout included (2-space indent, no trailing
        // newline, key order per type, unknowns after authored keys)
        let t = read_bm_tree(&root);
        write_bm_tree(&root, &t).unwrap();
        let unedited = String::from_utf8(bm_bytes(&root)).unwrap();
        assert_eq!(unedited, orig, "an edit-free round trip must not change one byte");

        // the ONE edit: rename the group `Work` (row 1: rows are Roadmap=0,
        // Work=1 — the `search` entry paints no row)
        let mut t = read_bm_tree(&root);
        assert_eq!(bm_rows_of(&t)[1].name, "Work", "the edit target is the measured row");
        bm_group_rename_in(&mut t, 1, "Work Renamed").unwrap();
        write_bm_tree(&root, &t).unwrap();
        let edited = String::from_utf8(bm_bytes(&root)).unwrap();

        // byte-wise: the result IS the fixture with that one line swapped —
        // `"title": "Work",` appears exactly once in the fixture
        assert_eq!(orig.matches("\"title\": \"Work\",").count(), 1);
        let want = orig.replacen("\"title\": \"Work\",", "\"title\": \"Work Renamed\",", 1);
        assert_eq!(edited, want, "one edit changes one line and nothing else");

        // the same facts spelled key by key, so a failure names the loss:
        let o: serde_json::Value = serde_json::from_str(orig).unwrap();
        let e: serde_json::Value = serde_json::from_str(&edited).unwrap();
        assert_eq!(e["items"][0]["zzUnknownFile"], serde_json::json!(42));
        assert_eq!(e["items"][1]["zzUnknownGroup"], o["items"][1]["zzUnknownGroup"]);
        assert_eq!(e["items"][2], o["items"][2], "the whole search entry, verbatim");
        for i in 0..4 {
            assert_eq!(e["items"][i]["ctime"], o["items"][i]["ctime"], "ctime included (item {i})");
        }
        assert_eq!(
            e["items"][1]["items"][0]["ctime"], o["items"][1]["items"][0]["ctime"],
            "nested ctime too"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// R4X.n (bmdrag, criterion 4) — the r4x_criterion4 pattern applied to a
    /// MOVE: the input is the same fixture stock 1.13.7 wrote while carrying
    /// keys it does not recognise, and the edit is `bm_drag_in_tree` — the
    /// drop path — not a field edit. Three drags, each proved BYTE-WISE by
    /// string surgery on the fixture itself (relocate the entry's text block,
    /// touch nothing else), so `zzUnknownFile` rides with its entry,
    /// `zzUnknownGroup`, the whole `search` entry and every `ctime` are
    /// proved unchanged by byte equality, not by a checklist:
    ///   1. row 0 -> end of top level (parent=None): the block lands AFTER
    ///      the search entry the pane never numbers — the painted-slot ->
    ///      raw-index mapping on the write path, on disk;
    ///   2. inverse drag: the file is byte-identical to the fixture again —
    ///      a move away and back destroys nothing;
    ///   3. row 0 -> first child of the group (recon case 2's prepend): the
    ///      same block re-indented into `Work`'s items, everything else
    ///      byte-for-byte the fixture.
    #[test]
    fn r4x_criterion4_move_through_the_drop_path_round_trips_byte_wise() {
        let orig: &str =
            include_str!("../../docs/fixtures/bmcompat/stock-1.13.7-unknown-keys.bookmarks.json");
        let root = tmp_vault("bm-c4-move");
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        fs::write(root.join(BM_FILE), orig).unwrap();

        // the entry we move, exactly as the fixture spells it (top level,
        // 4-space indent) — present exactly once
        let block = "    {\n      \"type\": \"file\",\n      \"ctime\": 1789851379753,\n      \"path\": \"Projects/Roadmap.md\",\n      \"zzUnknownFile\": 42\n    },\n";
        assert_eq!(orig.matches(block).count(), 1, "surgery anchor: the moved block");

        // drag 1: row 0 (Roadmap) to the end of the top level. Painted
        // top-level rows pre-detach: Roadmap=0, Work=1, Untitled group=2 —
        // pos 3 is the below-everything slot recon case 3 measured (append
        // to END of top level).
        let mut t = read_bm_tree(&root);
        assert_eq!(bm_rows_of(&t)[0].name, "Projects/Roadmap", "the drag source is the measured row");
        bm_drag_in_tree(&mut t, 0, None, 3).unwrap();
        write_bm_tree(&root, &t).unwrap();
        let moved = String::from_utf8(bm_bytes(&root)).unwrap();
        // expected bytes: the fixture minus the block, with the block (comma
        // dropped) re-attached after the LAST entry — i.e. after the search
        // entry, which paints no row: the raw slot is proved on disk
        let tail_old = "      \"title\": \"Untitled group\"\n    }\n  ]\n}";
        let tail_new = "      \"title\": \"Untitled group\"\n    },\n    {\n      \"type\": \"file\",\n      \"ctime\": 1789851379753,\n      \"path\": \"Projects/Roadmap.md\",\n      \"zzUnknownFile\": 42\n    }\n  ]\n}";
        assert!(orig.ends_with(tail_old), "surgery anchor: the fixture's tail");
        let want = orig.replacen(block, "", 1).replacen(tail_old, tail_new, 1);
        assert_eq!(moved, want, "the move relocates one block and changes nothing else");

        // drag 2: the inverse — back to painted slot 0 at the top level.
        // Painted rows now: Work=0, Untitled group=1, Roadmap=2.
        let mut t = read_bm_tree(&root);
        assert_eq!(bm_rows_of(&t)[4].name, "Projects/Roadmap");
        bm_drag_in_tree(&mut t, 4, None, 0).unwrap();
        write_bm_tree(&root, &t).unwrap();
        let restored = String::from_utf8(bm_bytes(&root)).unwrap();
        assert_eq!(restored, orig, "a move away and back is byte-identity");

        // drag 3: row 0 INTO the group `Work` as its first child (recon case
        // 2: a drop onto a group PREPENDS). The block re-indents to 8 spaces;
        // everything else is the fixture, byte for byte.
        let mut t = read_bm_tree(&root);
        bm_drag_in_tree(&mut t, 0, Some(1), 0).unwrap();
        write_bm_tree(&root, &t).unwrap();
        let nested = String::from_utf8(bm_bytes(&root)).unwrap();
        let open_old = "      \"items\": [\n        {\n          \"type\": \"file\",\n          \"ctime\": 1789851462235,";
        let open_new = "      \"items\": [\n        {\n          \"type\": \"file\",\n          \"ctime\": 1789851379753,\n          \"path\": \"Projects/Roadmap.md\",\n          \"zzUnknownFile\": 42\n        },\n        {\n          \"type\": \"file\",\n          \"ctime\": 1789851462235,";
        assert_eq!(orig.matches(open_old).count(), 1, "surgery anchor: Work's items open");
        let want = orig.replacen(block, "", 1).replacen(open_old, open_new, 1);
        assert_eq!(nested, want, "into-group prepends the same bytes, re-indented");
        let _ = fs::remove_dir_all(&root);
    }

    /// the refusal is byte-level BY CONSTRUCTION (recon case 6): a group
    /// dragged into itself or its own descendant returns Err BEFORE the tree
    /// is touched, so bm_apply never reaches the write and the file keeps its
    /// bytes — asserted here on the tree, on disk by phase_bmdrag.
    #[test]
    fn r4x_drag_refuses_self_and_descendant_without_touching_the_tree() {
        // rows: A=0(f), B=1(f), Work=2(g), inner=3(g, Work's child)
        let mut t: Vec<BmNode> = vec![BmNode::file("A"), BmNode::file("B")];
        bm_group_new_in(&mut t, None).unwrap();
        bm_group_rename_in(&mut t, 2, "Work").unwrap();
        bm_group_new_in(&mut t, Some(2)).unwrap();
        let snap = |t: &Vec<BmNode>| {
            bm_rows_of(t).iter().map(|r| format!("{}{}:{}", r.kind, r.depth, r.name)).collect::<Vec<_>>()
        };
        let before = snap(&t);
        let wk = before.iter().position(|s| s.ends_with(":Work")).unwrap();
        let inner = wk + 1; // its child paints directly under it
        assert!(bm_drag_in_tree(&mut t, wk, Some(wk), 0).is_err(), "into itself: refused");
        assert_eq!(snap(&t), before, "…and the tree is untouched");
        assert!(bm_drag_in_tree(&mut t, wk, Some(inner), 0).is_err(), "into own descendant: refused");
        assert_eq!(snap(&t), before, "…and the tree is untouched");
        // a file row is not a drop target
        assert!(bm_drag_in_tree(&mut t, wk, Some(0), 0).is_err(), "a file row is not a group");
        assert_eq!(snap(&t), before);
    }

    /// criterion 4's decision beyond stock: a TOP-LEVEL key we do not author
    /// survives our write even though stock itself would drop it (recon §5 —
    /// we are strictly more conservative than the app we replace).
    #[test]
    fn r4x_top_level_unknown_keys_survive_a_write() {
        let root = tmp_vault("bm-c4-toplevel");
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        fs::write(
            root.join(BM_FILE),
            r#"{"items": [{"type": "file", "ctime": 5, "path": "A.md"}], "zzTop": {"v": 1}}"#,
        )
        .unwrap();
        let mut t = read_bm_tree(&root);
        bm_toggle_in(&mut t, "B"); // one edit: add a bookmark
        write_bm_tree(&root, &t).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bm_bytes(&root)).unwrap();
        assert_eq!(v["zzTop"], serde_json::json!({"v": 1}), "top-level unknown kept");
        assert_eq!(v["items"][0]["ctime"], serde_json::json!(5));
        assert_eq!(v["items"][1]["path"], "B.md");
        let _ = fs::remove_dir_all(&root);
    }

    /// the parser is TOLERANT: not-JSON reads as the empty tree, an entry it
    /// cannot model reads as OPAQUE, and nothing panics.
    #[test]
    fn r4x_the_parser_tolerates_what_it_cannot_model() {
        assert!(parse_bm_tree("").is_empty());
        assert!(parse_bm_tree("\n\n").is_empty());
        assert!(parse_bm_tree("A\nsub/B\nC\n").is_empty(), "a v0.12 body is not JSON: ignored");
        assert!(parse_bm_tree("[]").is_empty(), "top level must be an object");
        assert!(parse_bm_tree(r#"{"items": 3}"#).is_empty(), "items must be an array");
        let t = parse_bm_tree(
            r#"{"items": [7, {"type": "zz"}, {"type": "group", "title": 5, "items": []},
                          {"type": "file", "ctime": 1, "path": "A.md"}]}"#,
        );
        assert_eq!(t.len(), 4, "nothing dropped");
        assert!(matches!(&t[0], BmNode::Opaque(_)), "a non-object entry is opaque");
        assert!(matches!(&t[1], BmNode::Opaque(_)), "an unknown type is opaque");
        assert!(matches!(&t[2], BmNode::Opaque(_)), "a non-string title is not ours to reshape");
        assert_eq!(
            bm_rows_of(&t).into_iter().map(|r| (r.kind, r.depth, r.name)).collect::<Vec<_>>(),
            vec![("f".to_string(), 0, "A".to_string())]
        );
    }

    /// the four structural operations, addressed by ROW INDEX (titles are not
    /// keys: stock allows two sibling groups with the same title).
    #[test]
    fn r4x_group_new_rename_delete_and_move_by_row_index() {
        let mut t: Vec<BmNode> = vec![BmNode::file("A"), BmNode::file("B")];
        // new group at the top level, with stock's default name
        bm_group_new_in(&mut t, None).unwrap();
        assert_eq!(bm_rows_of(&t)[2].name, "Untitled group");
        // ...and a second one, nested in the first — two groups, same title
        bm_group_new_in(&mut t, Some(2)).unwrap();
        assert_eq!(
            bm_rows_of(&t).iter().map(|r| format!("{}{}", r.kind, r.depth)).collect::<Vec<_>>(),
            vec!["f0", "f0", "g0", "g1"]
        );
        bm_group_rename_in(&mut t, 2, "Work").unwrap();
        assert_eq!(bm_rows_of(&t)[2].name, "Work");
        assert!(bm_group_rename_in(&mut t, 0, "nope").is_err(), "a file row is not a group");
        // move a file IN, then back OUT to the top level (the superset route)
        bm_move_in_tree(&mut t, 0, Some(2)).unwrap();
        assert_eq!(
            bm_rows_of(&t).iter().map(|r| format!("{}{}:{}", r.kind, r.depth, r.name)).collect::<Vec<_>>(),
            vec!["f0:B", "g0:Work", "g1:Untitled group", "f1:A"]
        );
        bm_move_in_tree(&mut t, 3, None).unwrap();
        assert_eq!(
            bm_rows_of(&t).iter().map(|r| format!("{}{}:{}", r.kind, r.depth, r.name)).collect::<Vec<_>>(),
            vec!["f0:B", "g0:Work", "g1:Untitled group", "f0:A"]
        );
        assert!(bm_move_in_tree(&mut t, 1, Some(2)).is_err(), "a group cannot move into its own child");
        // R4X.3: delete takes the SUBTREE, children are not re-parented
        bm_move_in_tree(&mut t, 0, Some(2)).unwrap(); // B into the nested group
        bm_group_delete_in(&mut t, 0).unwrap();       // delete Work, holding it
        assert!(bm_rows_of(&t).iter().map(|r| r.name.clone()).collect::<Vec<_>>() == vec!["A"],
                "the whole subtree went, nothing re-parented: {:?}", bm_rows_of(&t));
    }

    /// criterion 5, half 1 — a bookmarked note RENAMED while it lives inside a
    /// group keeps that group. The assertion NAMES the group and its line,
    /// rather than counting survivors: a flat rebuild would leave the same
    /// number of bookmarks and lose every group.
    #[test]
    fn r4x_grouped_bookmark_keeps_group_on_rename() {
        let root = tmp_vault("bm-grp-ren");
        fs::write(root.join("Ideas.md"), "i").unwrap();
        fs::write(root.join("Loose.md"), "l").unwrap();
        write_bm_tree(
            &root,
            &[
                BmNode::group("Work", vec![BmNode::file("Ideas")]),
                BmNode::file("Loose"),
            ],
        )
        .unwrap();
        let rows = |root: &Path| -> Vec<String> {
            bm_rows_of(&read_bm_tree(root))
                .into_iter()
                .map(|r| format!("{}{}:{}", r.kind, r.depth, r.name))
                .collect()
        };
        let mut ix = Index::build(&root);
        rename_in(&root, &mut ix, "Ideas", "Plans").unwrap();
        assert_eq!(
            rows(&root),
            vec!["g0:Work", "f1:Plans", "f0:Loose"],
            "the renamed bookmark is still INSIDE Work, at its index"
        );
        // and a MOVE to another folder goes through the same code (R9.8)
        move_note_in(&root, &mut ix, "Plans", "sub/Plans").unwrap();
        assert_eq!(
            rows(&root),
            vec!["g0:Work", "f1:sub/Plans", "f0:Loose"],
            "a moved note keeps its group too"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// criterion 5, half 2 — a bookmarked note DELETED while it lives inside a
    /// group keeps that group: delete_note_in does not touch the bookmarks
    /// file at all, so the entry stays where it sits and goes stale, which is
    /// a state R9.8 already treats as reachable.
    #[test]
    fn r4x_grouped_bookmark_keeps_group_on_delete() {
        let root = tmp_vault("bm-grp-del");
        fs::write(root.join("Ideas.md"), "i").unwrap();
        write_bm_tree(&root, &[BmNode::group("Work", vec![BmNode::file("Ideas")])]).unwrap();
        let before = bm_bytes(&root);
        let mut ix = Index::build(&root);
        delete_note_in(&root, &mut ix, "Ideas").unwrap();
        assert!(!root.join("Ideas.md").exists(), "the note really was deleted");
        assert_eq!(bm_bytes(&root), before, "a delete rewrote .obsidian/bookmarks.json");
        let v: serde_json::Value = serde_json::from_slice(&bm_bytes(&root)).unwrap();
        assert_eq!(v["items"][0]["title"], "Work");
        assert_eq!(
            v["items"][0]["items"][0]["path"], "Ideas.md",
            "the stale bookmark is still inside Work"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// R4X.2 / criterion 6 at the unit level — no structural operation may
    /// touch a .md. The phase proves it again against the real app; this
    /// proves the MODEL cannot even express it: the vault's bytes and mtimes
    /// are identical across a create/rename/move/delete sequence.
    #[test]
    fn r4x_no_group_operation_touches_a_note_on_disk() {
        let root = tmp_vault("bm-nomd");
        fs::write(root.join("Ideas.md"), "i").unwrap();
        fs::write(root.join("Other.md"), "o").unwrap();
        bm_seed(&root, &["Ideas", "Other"]);
        let snap = |r: &Path| -> Vec<(String, Vec<u8>, std::time::SystemTime)> {
            let mut v: Vec<_> = fs::read_dir(r)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
                .map(|e| {
                    (
                        e.file_name().to_string_lossy().to_string(),
                        fs::read(e.path()).unwrap(),
                        e.metadata().unwrap().modified().unwrap(),
                    )
                })
                .collect();
            v.sort();
            v
        };
        let before = snap(&root);
        let mut t = read_bm_tree(&root);
        bm_group_new_in(&mut t, None).unwrap();
        bm_group_rename_in(&mut t, 2, "Work").unwrap();
        bm_move_in_tree(&mut t, 0, Some(2)).unwrap();
        bm_group_delete_in(&mut t, 1).unwrap();
        write_bm_tree(&root, &t).unwrap();
        assert_eq!(bm_flat(&t), vec!["Other"], "the NAME went with the group");
        assert!(root.join("Ideas.md").is_file(), "the NOTE did not");
        assert_eq!(snap(&root), before, "a group operation rewrote a .md");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_updates_links_vault_wide() {
        let root = std::env::temp_dir().join(format!("rustidian-rl-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("Old.md"), "self [[Old]]").unwrap();
        fs::write(
            root.join("B.md"),
            "see [[Old]] and [[Old|nick]] plus [[Old#h2]] and [[Other]]",
        )
        .unwrap();
        fs::write(root.join("sub/C.md"), "[[Old|x]] deep").unwrap();
        let mut ix = Index::build(&root);
        rename_in(&root, &mut ix, "Old", "New").unwrap();
        assert_eq!(
            fs::read_to_string(root.join("B.md")).unwrap(),
            "see [[New]] and [[New|nick]] plus [[New#h2]] and [[Other]]"
        );
        assert_eq!(fs::read_to_string(root.join("sub/C.md")).unwrap(), "[[New|x]] deep");
        assert_eq!(fs::read_to_string(root.join("New.md")).unwrap(), "self [[New]]");
        // full-path links track a move into a folder; basename links keep basename
        fs::write(root.join("D.md"), "[[New]] and [[sub/C]]").unwrap();
        ix.upsert("D", "[[New]] and [[sub/C]]");
        rename_in(&root, &mut ix, "sub/C", "sub/C2").unwrap();
        assert_eq!(
            fs::read_to_string(root.join("D.md")).unwrap(),
            "[[New]] and [[sub/C2]]"
        );
        // ambiguity guard: renaming sub/C2 -> sub/C while a top-level C.md
        // exists — basename rewrite would make [[C2]] capture the decoy, so
        // only the full-path link updates
        fs::write(root.join("C.md"), "decoy").unwrap();
        fs::write(root.join("E.md"), "[[C2]] and [[sub/C2]]").unwrap();
        ix.upsert("C", "decoy");
        ix.upsert("E", "[[C2]] and [[sub/C2]]");
        rename_in(&root, &mut ix, "sub/C2", "sub/C").unwrap();
        assert_eq!(
            fs::read_to_string(root.join("E.md")).unwrap(),
            "[[C2]] and [[sub/C]]"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /* ---- R34: rename-from-title splits the move from the link rewrite ---- */

    /// scratch vault under a per-test name (tests share the process)
    fn r34_vault(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rustidian-r34-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    // ---- R24.7/R24.8 DELETE ------------------------------------------------

    /// R24.8, asserted BY BYTES, which is the only way it can be asserted: every
    /// note that links to the target is hashed before the delete and compared
    /// after it. "We did not intend to write them" is not evidence; a byte
    /// comparison over all six link forms is.
    #[test]
    fn delete_leaves_every_linking_note_byte_identical() {
        let root = r34_vault("del-bytes");
        fs::write(root.join("Target.md"), "# Target\nbody\n").unwrap();
        fs::write(root.join("B.md"), "[[Target]] and [[Target|alias]] and [[Target#anchor]]\n").unwrap();
        fs::write(root.join("C.md"), "embed ![[Target]] and block [[Target#^id]]\n").unwrap();
        fs::write(root.join("D.md"), "markdown [t](Target.md)\n").unwrap();
        fs::write(root.join("E.md"), "links nowhere near it\n").unwrap();
        let mut ix = Index::build(&root);
        let before: Vec<Vec<u8>> = ["B", "C", "D", "E"]
            .iter()
            .map(|n| fs::read(root.join(format!("{n}.md"))).unwrap())
            .collect();

        let dest = delete_note_in(&root, &mut ix, "Target").unwrap();

        // the file is GONE from the vault, and recoverable from the trash with
        // its own bytes intact
        assert!(!root.join("Target.md").exists(), "the note is still where it was");
        assert_eq!(dest, ".trash/Target.md");
        assert_eq!(fs::read_to_string(root.join(&dest)).unwrap(), "# Target\nbody\n");
        // R24.8: not one linking note changed by a single byte
        for (n, b0) in ["B", "C", "D", "E"].iter().zip(before) {
            assert_eq!(fs::read(root.join(format!("{n}.md"))).unwrap(), b0, "{n} was rewritten by a DELETE");
        }
        // R24.9's precondition: the key left the index, so those links no longer
        // resolve — which is what makes them render faded
        assert_eq!(ix.names(), ["B", "C", "D", "E"]);
        assert!(ix.content("Target").is_none());
        assert_eq!(resolve(ix.names(), "Target"), None, "a deleted note must not resolve");
    }

    /// The trashed copy must be gone from the VAULT as the app walks it —
    /// otherwise "deleted" means "moved to a folder the explorer still lists".
    #[test]
    fn a_deleted_note_never_walks_back_into_the_index() {
        let root = r34_vault("del-walk");
        fs::write(root.join("Gone.md"), "x\n").unwrap();
        fs::write(root.join("Keep.md"), "[[Gone]]\n").unwrap();
        let mut ix = Index::build(&root);
        delete_note_in(&root, &mut ix, "Gone").unwrap();
        assert!(root.join(".trash/Gone.md").is_file(), "nothing was preserved");
        // a FRESH walk of the vault, i.e. what the next boot / watcher tick sees
        assert_eq!(Index::build(&root).names(), ["Keep"]);
        assert!(!index::notes_of(&root).iter().any(|n| n.contains("trash")));
    }

    /// Two notes with the same basename in different folders: the second delete
    /// must not overwrite the first one's only remaining copy.
    #[test]
    fn deleting_two_notes_that_share_a_basename_keeps_both_copies() {
        let root = r34_vault("del-collide");
        fs::create_dir_all(root.join("a")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        fs::write(root.join("a/Note.md"), "first\n").unwrap();
        fs::write(root.join("b/Note.md"), "second\n").unwrap();
        let mut ix = Index::build(&root);
        assert_eq!(delete_note_in(&root, &mut ix, "a/Note").unwrap(), ".trash/Note.md");
        assert_eq!(delete_note_in(&root, &mut ix, "b/Note").unwrap(), ".trash/Note.1.md");
        assert_eq!(fs::read_to_string(root.join(".trash/Note.md")).unwrap(), "first\n");
        assert_eq!(fs::read_to_string(root.join(".trash/Note.1.md")).unwrap(), "second\n");
    }

    /// A refused delete changes NOTHING: no trash directory materialises, no
    /// note moves, and the index still holds every key.
    #[test]
    fn a_refused_delete_touches_nothing() {
        let root = r34_vault("del-refuse");
        fs::write(root.join("A.md"), "a\n").unwrap();
        let mut ix = Index::build(&root);
        for bad in ["../escape", "Missing", ".obsidian/app", ""] {
            assert!(delete_note_in(&root, &mut ix, bad).is_err(), "delete accepted {bad:?}");
        }
        assert!(!root.join(".trash").exists(), "a refused delete created the trash dir");
        assert_eq!(fs::read_to_string(root.join("A.md")).unwrap(), "a\n");
        assert_eq!(ix.names(), ["A"]);
    }

    /// R24.8 taken literally: a delete writes the deleted note's path and no
    /// other file — including the user's bookmarks file, which this codebase
    /// already tolerates holding an entry with no file behind it.
    #[test]
    fn delete_does_not_touch_the_bookmarks_file() {
        let root = r34_vault("del-bm");
        fs::write(root.join("A.md"), "a\n").unwrap();
        fs::write(root.join("B.md"), "[[A]]\n").unwrap();
        write_bookmarks(&root, &["A".to_string(), "B".to_string()]).unwrap();
        let bm0 = fs::read(root.join(BM_FILE)).unwrap();
        let mut ix = Index::build(&root);
        delete_note_in(&root, &mut ix, "A").unwrap();
        assert_eq!(fs::read(root.join(BM_FILE)).unwrap(), bm0, "a delete rewrote .obsidian/bookmarks.json");
    }

    /// R34.1 + R34.2: the move renames the FILE and NOT ONE inbound link.
    /// This is the backend shape of "never rewrite links without the prompt":
    /// after move_note_in the vault is in stock's answer-was-no state.
    #[test]
    fn move_note_renames_the_file_and_rewrites_no_link() {
        let root = r34_vault("move");
        fs::write(root.join("Old.md"), "# Heading\nbody [[Other]]\n").unwrap();
        fs::write(root.join("B.md"), "[[Old]] and [[Old|alias]] and [[Old#anchor]]").unwrap();
        fs::write(root.join("C.md"), "embed ![[Old]]").unwrap();
        fs::write(root.join("D.md"), "unrelated").unwrap();
        let mut ix = Index::build(&root);
        let (b0, c0, d0) = (
            fs::read(root.join("B.md")).unwrap(),
            fs::read(root.join("C.md")).unwrap(),
            fs::read(root.join("D.md")).unwrap(),
        );

        let (links, files) = move_note_in(&root, &mut ix, "Old", "New").unwrap();
        // 3 links in B + 1 embed in C, in 2 files — links are counted per
        // OCCURRENCE, so this can never be read off a backlink list (which
        // would say "2")
        assert_eq!((links, files), (4, 2), "blast radius the modal states");

        // the file moved, byte-for-byte, body untouched (R34.1)
        assert!(!root.join("Old.md").exists());
        assert_eq!(
            fs::read_to_string(root.join("New.md")).unwrap(),
            "# Heading\nbody [[Other]]\n",
            "the move must not touch the note's own text"
        );
        // and NOT ONE inbound link moved with it
        assert_eq!(fs::read(root.join("B.md")).unwrap(), b0, "B was rewritten without consent");
        assert_eq!(fs::read(root.join("C.md")).unwrap(), c0, "C was rewritten without consent");
        assert_eq!(fs::read(root.join("D.md")).unwrap(), d0);
        assert_eq!(ix.names(), ["B", "C", "D", "New"]);

        // ...and the consented half preserves alias, #anchor and embed form
        let wrote = update_links_in(&root, &mut ix, "Old", "New");
        assert_eq!(wrote, 2);
        assert_eq!(
            fs::read_to_string(root.join("B.md")).unwrap(),
            "[[New]] and [[New|alias]] and [[New#anchor]]"
        );
        assert_eq!(fs::read_to_string(root.join("C.md")).unwrap(), "embed ![[New]]");
        assert_eq!(fs::read(root.join("D.md")).unwrap(), d0, "an unlinked note is never rewritten");
        assert_eq!(ix.backlinks("New"), ["B", "C"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// R34.11: collision NEVER overwrites. create_new(2) is the kernel's
    /// atomic exists-check — the refusal does not depend on a stat that a
    /// racing writer could invalidate, and the victim's bytes are the proof.
    #[test]
    fn move_note_never_overwrites_on_collision() {
        let root = r34_vault("collide");
        fs::write(root.join("A.md"), "the note being renamed").unwrap();
        fs::write(root.join("X.md"), "PRECIOUS — must survive").unwrap();
        fs::create_dir_all(root.join("Dir.md")).unwrap(); // a DIRECTORY in the way
        let mut ix = Index::build(&root);

        assert_eq!(move_note_in(&root, &mut ix, "A", "X").unwrap_err(), "target exists");
        assert_eq!(
            fs::read_to_string(root.join("X.md")).unwrap(),
            "PRECIOUS — must survive",
            "the target was clobbered"
        );
        assert_eq!(fs::read_to_string(root.join("A.md")).unwrap(), "the note being renamed");
        assert!(move_note_in(&root, &mut ix, "A", "Dir").is_err());
        assert!(root.join("Dir.md").is_dir());
        // a refused move leaves NO zero-byte claim behind anywhere
        let mut names: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["A.md", "Dir.md", "X.md"]);
        // rename onto ITSELF is a collision too, and must not truncate
        assert!(move_note_in(&root, &mut ix, "A", "A").is_err());
        assert_eq!(fs::read_to_string(root.join("A.md")).unwrap(), "the note being renamed");
        let _ = fs::remove_dir_all(&root);
    }

    /// R34.3: zero inbound links => (0, 0) => stock shows NO modal. The
    /// count is what suppresses the prompt, so it gets its own assertion.
    #[test]
    fn move_note_blast_radius_is_zero_when_nothing_links_in() {
        let root = r34_vault("zero");
        fs::write(root.join("Lonely.md"), "nobody links here").unwrap();
        fs::write(root.join("Other.md"), "[[Somewhere]] else").unwrap();
        let mut ix = Index::build(&root);
        assert_eq!(ix.rename_blast("Lonely", "Renamed"), (0, 0));
        assert_eq!(move_note_in(&root, &mut ix, "Lonely", "Renamed").unwrap(), (0, 0));
        let _ = fs::remove_dir_all(&root);
    }

    /// the count is a PROMISE: whatever rename_blast says before the move,
    /// update_links delivers exactly that many rewritten links after it.
    #[test]
    fn blast_radius_equals_what_the_rewrite_actually_changes() {
        let root = r34_vault("promise");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/Target.md"), "x").unwrap();
        fs::write(root.join("P.md"), "[[Target]] [[sub/Target]] [[Target|a]]").unwrap();
        fs::write(root.join("Q.md"), "![[sub/Target#h]]").unwrap();
        let mut ix = Index::build(&root);
        let before = ix.rename_blast("sub/Target", "sub/Moved");
        assert_eq!(before, (4, 2));
        let after = move_note_in(&root, &mut ix, "sub/Target", "sub/Moved").unwrap();
        assert_eq!(after, before);
        update_links_in(&root, &mut ix, "sub/Target", "sub/Moved");
        let (p, q) = (
            fs::read_to_string(root.join("P.md")).unwrap(),
            fs::read_to_string(root.join("Q.md")).unwrap(),
        );
        assert_eq!(p, "[[Moved]] [[sub/Moved]] [[Moved|a]]");
        assert_eq!(q, "![[sub/Moved#h]]");
        // exactly `links` occurrences changed, not one more
        assert_eq!(p.matches("Moved").count() + q.matches("Moved").count(), before.0);
        let _ = fs::remove_dir_all(&root);
    }

    /// R34.8: the consent flag round-trips through `.obsidian/app.json`, and
    /// the MERGE is the point — a vault that already configures
    /// attachmentFolderPath (R31.3) must still configure it afterwards. A
    /// remembered checkbox that eats the rest of someone's config is a worse
    /// bug than the modal it suppresses.
    #[test]
    fn link_consent_round_trips_and_preserves_other_config() {
        let root = r34_vault("consent");
        assert!(!link_consent_in(&root), "a fresh vault has no consent on record");
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        fs::write(root.join(".obsidian/app.json"), r#"{"attachmentFolderPath":"files"}"#).unwrap();
        assert!(!link_consent_in(&root), "an unrelated config is not consent");
        set_link_consent_in(&root, true).unwrap();
        assert!(link_consent_in(&root));
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(root.join(".obsidian/app.json")).unwrap()).unwrap();
        assert_eq!(v["attachmentFolderPath"], "files", "the merge kept the key we do not own");
        assert_eq!(v["alwaysUpdateLinks"], true);
        // and it can be revoked without dropping the neighbour key either
        set_link_consent_in(&root, false).unwrap();
        assert!(!link_consent_in(&root));
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(root.join(".obsidian/app.json")).unwrap()).unwrap();
        assert_eq!(v["attachmentFolderPath"], "files");
        assert!(!root.join(".obsidian/app.json.tmp").exists(), "no stray temp left behind");
        let _ = fs::remove_dir_all(&root);
    }

    /// ...and an app.json we cannot parse is REFUSED, byte-for-byte intact.
    /// Same rule as the collision: data loss outranks the feature.
    #[test]
    fn set_link_consent_never_overwrites_an_unparseable_config() {
        let root = r34_vault("badcfg");
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        let junk = "{ this is not json, but it IS someone's config\n";
        fs::write(root.join(".obsidian/app.json"), junk).unwrap();
        assert!(set_link_consent_in(&root, true).is_err());
        assert_eq!(fs::read_to_string(root.join(".obsidian/app.json")).unwrap(), junk);
        assert!(!link_consent_in(&root), "unparseable is never read as consent");
        // a JSON document that is not an object is refused too, not wrapped
        fs::write(root.join(".obsidian/app.json"), "[1,2,3]").unwrap();
        assert!(set_link_consent_in(&root, true).is_err());
        assert_eq!(fs::read_to_string(root.join(".obsidian/app.json")).unwrap(), "[1,2,3]");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_moves_refuses_overwrite() {
        let root = std::env::temp_dir().join(format!("rustidian-rn-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("A.md"), "body").unwrap();
        fs::write(root.join("B.md"), "other").unwrap();
        let mut ix = Index::build(&root);
        rename_in(&root, &mut ix, "A", "sub/A2").unwrap(); // parents created
        assert!(!root.join("A.md").exists());
        assert_eq!(fs::read_to_string(root.join("sub/A2.md")).unwrap(), "body");
        assert_eq!(ix.names(), ["B", "sub/A2"]); // index key moved with the file
        assert!(rename_in(&root, &mut ix, "sub/A2", "B").is_err()); // refuse overwrite
        assert!(rename_in(&root, &mut ix, "Ghost", "X").is_err()); // missing source
        assert!(rename_in(&root, &mut ix, "B", "../esc").is_err()); // traversal blocked
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn render_blocks_matches_per_block_render() {
        let notes = vec!["Ideas".to_string(), "sub/Nested".to_string()];
        let blocks: Vec<String> = [
            "# Title [[Ideas]]",
            "plain para with [[Nested]] and [[Nope]]",
            "- a\n- b [[Ideas|alias]]",
            "```\n[[Ideas]]\n```",
            "",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let batch = render_blocks_with(&blocks, &notes, &[]);
        assert_eq!(batch.len(), blocks.len());
        for (b, out) in blocks.iter().zip(&batch) {
            assert_eq!(*out, render_md(b, &notes), "block {b:?} diverged");
        }
        assert!(render_blocks_with(&[], &notes, &[]).is_empty());
    }

    #[test]
    fn recent_list_dedups_newest_first_capped() {
        let l = push_recent(vec![], "/a");
        let l = push_recent(l, "/b");
        assert_eq!(l, vec!["/b", "/a"]); // newest first
        let l = push_recent(l, "/a");    // re-open dedups + promotes
        assert_eq!(l, vec!["/a", "/b"]);
        let mut l = l;
        for i in 0..10 { l = push_recent(l, &format!("/v{i}")); }
        assert_eq!(l.len(), 8);          // capped
        assert_eq!(l[0], "/v9");
    }

    /// S2: symlinked dir + file inside the vault are invisible: not indexed,
    /// not snapshotted, not readable, not writable, not creatable-through
    #[test]
    fn symlinks_are_not_part_of_the_vault() {
        use std::os::unix::fs::symlink;
        let root = tmp_vault("sym");
        let outside = std::env::temp_dir().join(format!("rustidian-sym-out-{}", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("Secret.md"), "secret").unwrap();
        fs::write(root.join("A.md"), "[[L]] [[link/Secret]]").unwrap();
        symlink(&outside, root.join("link")).unwrap(); // dir link
        symlink(outside.join("Secret.md"), root.join("L.md")).unwrap(); // file link
        // not listed (Index::build + watcher::snapshot share the walk)
        let ix = Index::build(&root);
        assert_eq!(ix.names(), ["A"]);
        assert_eq!(watcher::snapshot(&root).keys().cloned().collect::<Vec<_>>(), ["A"]);
        // not readable
        assert_eq!(note_path_in(&root, "L", false), None);
        assert_eq!(note_path_in(&root, "link/Secret", false), None);
        assert_eq!(note_path_in(&root, "A", false), Some(root.canonicalize().unwrap().join("A.md")));
        // not writable: create=true must not mkdir through the link either
        assert_eq!(note_path_in(&root, "L", true), None);
        assert_eq!(note_path_in(&root, "link/New", true), None);
        assert_eq!(note_path_in(&root, "link/deep/er/New", true), None);
        assert!(!outside.join("New.md").exists());
        assert!(!outside.join("deep").exists());
        assert_eq!(fs::read_to_string(outside.join("Secret.md")).unwrap(), "secret");
        // rename refuses both directions
        let mut ix = Index::build(&root);
        assert!(rename_in(&root, &mut ix, "A", "link/A").is_err());
        assert!(rename_in(&root, &mut ix, "L", "X").is_err());
        assert!(root.join("A.md").is_file());
        assert!(!outside.join("A.md").exists());
        // a plain note still round-trips through create=true (parents made)
        let p = note_path_in(&root, "sub/deep/N", true).unwrap();
        assert!(p.parent().unwrap().is_dir());
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    /// R29 images, POSITIVE half: the shapes that MUST serve bytes. Kept next
    /// to the traversal test because a containment check that refuses
    /// everything also passes the negative one.
    #[test]
    fn img_serves_contained_vault_images_only() {
        let root = tmp_vault("img-ok");
        let px = b"\x89PNG\r\n\x1a\nfake-but-bytes";
        for f in ["pic.png", "my pic.png", "UPPER.PNG", "sub/nested.jpeg", "doc.md", "vec.svg"] {
            fs::write(root.join(f), px).unwrap();
        }
        let croot = root.canonicalize().unwrap();
        // R29.2 relative + subfolder; R29.3 percent-decoded space
        assert_eq!(img_path_in(&root, "pic.png"), Some(croot.join("pic.png")));
        assert_eq!(img_path_in(&root, "sub/nested.jpeg"), Some(croot.join("sub/nested.jpeg")));
        assert_eq!(img_path_in(&root, "my%20pic.png"), Some(croot.join("my pic.png")));
        assert_eq!(img_path_in(&root, "my pic.png"), Some(croot.join("my pic.png")));
        // extension match is case-insensitive, the Content-Type comes from OUR table
        assert_eq!(serve_image(&root, "UPPER.PNG").unwrap().0, "image/png");
        assert_eq!(serve_image(&root, "sub/nested.jpeg").unwrap().0, "image/jpeg");
        assert_eq!(serve_image(&root, "pic.png").unwrap().1, px.to_vec());
        // R29.8: svg is NOT in v1 (scriptable, no sanitizer here); non-images never
        assert_eq!(img_path_in(&root, "vec.svg"), None);
        assert_eq!(img_path_in(&root, "doc.md"), None);
        assert_eq!(img_path_in(&root, "pic"), None);
        // R29.4: a missing file serves nothing — same None as an escape, no leak
        assert_eq!(img_path_in(&root, "nope.png"), None);
        // S5-style cap: an oversized image is not part of the vault either
        let big = root.join("big.png");
        fs::write(&big, px).unwrap();
        assert!(img_path_in(&root, "big.png").is_some());
        let f = fs::OpenOptions::new().write(true).open(&big).unwrap();
        f.set_len(MAX_IMG_BYTES + 1).unwrap();
        assert_eq!(img_path_in(&root, "big.png"), None);
        let _ = fs::remove_dir_all(&root);
    }

    /// R29.5 + S2: every escape serves NO bytes, in both directions —
    /// a target that walks out, and a symlink inside the vault that points out.
    /// The positive control (a genuinely SYMLINKED VAULT still serves) is in
    /// the same test on purpose: canonicalizing only one side passes one half
    /// and fails the other, which is the likely failure mode.
    #[test]
    fn img_traversal_and_symlink_escapes_serve_no_bytes() {
        use std::os::unix::fs::symlink;
        let root = tmp_vault("img-esc");
        let outside = std::env::temp_dir().join(format!("rustidian-img-out-{}", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        let px = b"\x89PNG\r\n\x1a\nsecret-bytes";
        fs::write(outside.join("outside.png"), px).unwrap();
        fs::write(root.join("pic.png"), b"\x89PNG\r\n\x1a\nin-vault").unwrap();
        // a HIDDEN component is not a vault member (same rule as notes, safe_rel).
        // The file is created on purpose: without it the `.hidden/pic.png` case
        // below would be refused by a failed canonicalize instead of by the
        // check, and deleting safe_rel would not turn this test red.
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::write(root.join(".hidden/pic.png"), px).unwrap();
        // the out-of-vault file EXISTS — refusing it must not depend on a stat miss
        assert!(outside.join("outside.png").is_file());
        for t in [
            "../../../etc/passwd",
            "../outside.png",
            "sub/../../outside.png",
            "..%2foutside.png",
            "%2e%2e%2foutside.png",
            "%2e%2e/outside.png",
            "/etc/passwd",
            "/tmp/rustidian-img-out.png",
            "file:///etc/passwd",
            "pic.png\0.txt",
            "pic.png%00.txt",
            ".hidden/pic.png",
            "%zz.png",
            "%2",
        ] {
            assert_eq!(img_path_in(&root, t), None, "escaped the vault: {t:?}");
            assert!(serve_image(&root, t).is_none(), "served bytes for {t:?}");
        }
        // a symlink INSIDE the vault pointing OUT of it: leaf link and dir link
        symlink(outside.join("outside.png"), root.join("link.png")).unwrap();
        symlink(&outside, root.join("out")).unwrap();
        assert_eq!(img_path_in(&root, "link.png"), None);
        assert_eq!(img_path_in(&root, "out/outside.png"), None);
        assert!(serve_image(&root, "link.png").is_none());
        assert!(serve_image(&root, "out/outside.png").is_none());
        // POSITIVE CONTROL (req S2): a vault reached THROUGH a symlink still
        // serves its own images — this is what canonicalizing the root buys.
        let vlink = std::env::temp_dir().join(format!("rustidian-img-vlink-{}", std::process::id()));
        let _ = fs::remove_file(&vlink);
        symlink(&root, &vlink).unwrap();
        assert_eq!(serve_image(&vlink, "pic.png").unwrap().1, b"\x89PNG\r\n\x1a\nin-vault".to_vec());
        assert_eq!(img_path_in(&vlink, "link.png"), None); // still refused through the link
        let _ = fs::remove_file(&vlink);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    /// R29 + S1: the CSP is a security boundary, so it is asserted as an EXACT
    /// string. A future widening (`img-src *`, an http origin, a stray
    /// 'unsafe-inline' in script-src) then fails a test instead of shipping
    /// silently. tauri.conf.json IS a cargo input, so editing it rebuilds this.
    #[test]
    fn csp_is_exactly_this_string_and_no_remote_image_origin() {
        const CONF: &str = include_str!("../tauri.conf.json");
        let csp = serde_json::from_str::<serde_json::Value>(CONF).unwrap()["app"]["security"]["csp"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            csp,
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' data: rustidian-img:; font-src 'self'; \
             connect-src ipc: http://ipc.localhost",
            "the CSP changed — if that was deliberate, say why in the commit message"
        );
        // R29: the image widening is ONE scheme token, and it is ours
        let img = csp.split("; ").find(|d| d.starts_with("img-src ")).unwrap();
        assert_eq!(img, format!("img-src 'self' data: {IMG_SCHEME}:"));
        // R29.10: remote images stay closed pending an operator decision —
        // no wildcard, no http(s) origin anywhere in the image directive
        assert!(!img.contains('*'), "img-src wildcard re-enables remote beacons: {img}");
        assert!(!img.contains("http"), "img-src names a remote origin: {img}");
    }

    /// S4: picker commands refuse system trees and dot-components, keep the
    /// roots the folder picker offers
    #[test]
    fn picker_denies_system_paths_allows_user_roots() {
        for p in ["/etc", "/proc", "/etc/ssh", "/proc/self", "/sys", "/dev", "/usr/bin", "/boot", "/root", "/var/log",
                  "/home/u/.ssh", "/tmp/../etc", "/tmp/.hidden", "tmp", ""] {
            assert!(!picker_allows(Path::new(p)), "{p} must be denied");
        }
        for p in ["/", "/workspace", "/workspace/vault", "/tmp", "/tmp/rustidian-smoke97/vault", "/mnt", "/media", "/run/media", "/home", "/home/u/notes"] {
            assert!(picker_allows(Path::new(p)), "{p} must be allowed");
        }
        assert_eq!(list_dirs("/etc".into()), Vec::<String>::new());
        assert_eq!(list_dirs("/proc".into()), Vec::<String>::new());
        assert!(list_dirs("/".into()).iter().any(|d| d == "tmp"));
    }

    /// S5: a 33 MiB sparse note is skipped by the walk and unreadable
    #[test]
    fn oversized_note_is_skipped() {
        let root = tmp_vault("big");
        fs::write(root.join("A.md"), "small").unwrap();
        let big = root.join("Big.md");
        fs::File::create(&big).unwrap().set_len(33 * 1024 * 1024).unwrap();
        assert_eq!(Index::build(&root).names(), ["A"]);
        assert_eq!(watcher::snapshot(&root).keys().cloned().collect::<Vec<_>>(), ["A"]);
        assert_eq!(read_capped(&big), None); // read_note -> ""
        assert_eq!(read_capped(&root.join("A.md")).as_deref(), Some("small"));
        let _ = fs::remove_dir_all(&root);
    }

    /* ---------- dataloss-audit F1/F3/F4 regression tests ----------
       These bugs are invisible in normal operation: the product reports
       success while the bytes are gone. Each test fails without its fix. */

    /// F1 [audit src-tauri/src/main.rs:162]: a write that cannot land must
    /// RETURN the error and must NOT upsert the index — the old code dropped
    /// fs::write's Result inside an `if ... .is_ok()`, so the UI marked the
    /// buffer clean and every edit of the session died in RAM.
    #[test]
    fn f1_failed_write_returns_err_and_leaves_the_index_untouched() {
        let root = tmp_vault("f1");
        fs::write(root.join("Note.md"), "old body").unwrap();
        let mut ix = Index::build(&root);
        // EISDIR as a portable stand-in for the audit's ENOSPC/EROFS/EDQUOT:
        // a directory sitting exactly where the note file must be created.
        fs::create_dir(root.join("Blocked.md")).unwrap();
        let e = write_note_in(&root, &mut ix, "Blocked", "new body").unwrap_err();
        assert!(!e.is_empty(), "a failed save must carry a message to the UI");
        assert!(ix.content("Blocked").is_none(), "index upserted for bytes that never landed");
        // the happy path still succeeds, lands, and indexes
        assert_eq!(write_note_in(&root, &mut ix, "Note", "new body"), Ok(()));
        assert_eq!(fs::read_to_string(root.join("Note.md")).unwrap(), "new body");
        assert_eq!(ix.content("Note"), Some("new body"));
        let _ = fs::remove_dir_all(&root);
    }

    /// LW (lost-write): `link_mention` used to call write_note_inner and
    /// THROW THE RESULT AWAY, returning Ok(()) — the user clicked "link
    /// mention", the write was refused (ENOSPC/EROFS/sandbox), the UI said it
    /// worked and the edit was gone. A refused write must reach the caller.
    /// Read-only vault dir = a real refusal (write_atomic must create a
    /// sibling temp in it), not a deleted file, which is a different path.
    #[test]
    fn lw_link_mention_surfaces_a_refused_write() {
        use std::os::unix::fs::PermissionsExt;
        /// restores the mode even if an assert below panics — a test that
        /// dies with the fixture at 0555 poisons every later run
        struct RestoreMode(PathBuf, u32);
        impl Drop for RestoreMode {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(self.1));
            }
        }
        const BODY: &str = "see Link Target here\n";
        let root = tmp_vault("lw");
        fs::write(root.join("Link Target.md"), "target\n").unwrap();
        fs::write(root.join("Note.md"), BODY).unwrap();
        let mut ix = Index::build(&root);
        // control: while the vault is writable the same call lands
        link_mention_in(&root, &mut ix, "Note", "Link Target", 0, 4, 11).unwrap();
        assert_eq!(fs::read_to_string(root.join("Note.md")).unwrap(), "see [[Link Target]] here\n");

        fs::write(root.join("Note.md"), BODY).unwrap();
        let mut ix = Index::build(&root);
        let mode0 = fs::metadata(&root).unwrap().permissions().mode();
        let guard = RestoreMode(root.clone(), mode0);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();
        // the setup must ACTUALLY refuse the write — root ignores 0555, and a
        // test whose failure injection silently did nothing proves nothing
        assert!(
            fs::File::create(root.join("probe.tmp")).is_err(),
            "vault still writable at 0555 (running as root?) — no refusal was injected"
        );
        let e = link_mention_in(&root, &mut ix, "Note", "Link Target", 0, 4, 11)
            .expect_err("a write that was REFUSED was reported to the user as a saved one");
        assert!(!e.is_empty(), "a failed save must carry a message to the UI");
        // nothing landed, and the index still == disk
        assert_eq!(fs::read_to_string(root.join("Note.md")).unwrap(), BODY);
        assert_eq!(ix.content("Note"), Some(BODY));
        drop(guard);
        let _ = fs::remove_dir_all(&root);
    }

    /// F3 [audit src-tauri/src/main.rs:162]: the note write must be
    /// sibling-temp + fsync + rename, not a truncating open. rename(2)
    /// installs a NEW inode over the name; a truncating write keeps the old
    /// one — that identity IS the window where a crash leaves a short file.
    #[test]
    fn f3_note_write_is_sibling_temp_rename_not_a_truncating_open() {
        use std::os::unix::fs::MetadataExt;
        let root = tmp_vault("f3");
        let p = root.join("Note.md");
        fs::write(&p, "OLD").unwrap();
        let ino0 = fs::metadata(&p).unwrap().ino();
        let mut ix = Index::build(&root);
        write_note_in(&root, &mut ix, "Note", "NEW").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "NEW");
        assert_ne!(
            fs::metadata(&p).unwrap().ino(),
            ino0,
            "note was written in place (O_TRUNC) — no rename barrier"
        );
        // no stray temp survives a successful save
        let stray: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(stray.is_empty(), "temp left behind: {stray:?}");
        // the temp is a SIBLING (same dir => same filesystem => atomic rename):
        // block that exact path and the save fails with the OLD note intact,
        // where a truncating write would already have destroyed it.
        fs::write(root.join("Keep.md"), "KEEP").unwrap();
        fs::create_dir(root.join("Keep.md.tmp")).unwrap();
        let mut ix2 = Index::build(&root);
        assert!(write_note_in(&root, &mut ix2, "Keep", "LOST").is_err());
        assert_eq!(fs::read_to_string(root.join("Keep.md")).unwrap(), "KEEP");
        assert_eq!(ix2.content("Keep"), Some("KEEP"));
        let _ = fs::remove_dir_all(&root);
    }

    /// F4 [audit ui/main.js:606]: creation must refuse to clobber. create_new
    /// is the kernel's atomic exists-check, so the stale in-RAM index cannot
    /// turn a real note into a two-line stub between the check and the write.
    #[test]
    fn f4_create_note_refuses_to_clobber_an_existing_note() {
        let root = tmp_vault("f4");
        fs::write(root.join("Ghost.md"), "valuable\n").unwrap();
        let mut ix = Index::build(&root);
        let e = create_note_in(&root, &mut ix, "Ghost", "# Ghost\n\n").unwrap_err();
        assert_eq!(e, EXISTS, "the UI opens the existing note on this exact error");
        assert_eq!(fs::read_to_string(root.join("Ghost.md")).unwrap(), "valuable\n");
        assert_eq!(ix.content("Ghost"), Some("valuable\n"), "even the RAM copy must survive");
        // a genuinely new note is still created, on disk and in the index
        create_note_in(&root, &mut ix, "sub/New", "# New\n\n").unwrap();
        assert_eq!(fs::read_to_string(root.join("sub/New.md")).unwrap(), "# New\n\n");
        assert_eq!(ix.content("sub/New"), Some("# New\n\n"));
        let _ = fs::remove_dir_all(&root);
    }

    /* feedback #20 [acceptance 1]: a brand-new note materializes NOTHING.
       Recon Q1 against stock 1.13.7: Ctrl+N produces `Untitled.md` whose
       `wc -c` is 0, and a second one `Untitled 1.md`, also 0. The big title
       the user sees is the INLINE TITLE — the FILENAME, rendered — so the
       bytes on disk are empty. Asserted on disk (metadata len) AND in the
       in-RAM index, which the UI reads back for search. */
    #[test]
    fn f20_a_new_note_is_zero_bytes_on_disk() {
        let root = tmp_vault("f20zero");
        let mut ix = Index::build(&root);
        create_note_in(&root, &mut ix, "Untitled", "").unwrap();
        let p = root.join("Untitled.md");
        assert_eq!(fs::metadata(&p).unwrap().len(), 0, "stock's new note is ZERO bytes");
        assert_eq!(fs::read_to_string(&p).unwrap(), "");
        assert_eq!(ix.content("Untitled"), Some(""), "the index copy is empty too");
        // the path-qualified path is where the old code seeded the BASENAME
        create_note_in(&root, &mut ix, "sub/Deep Note", "").unwrap();
        assert_eq!(fs::metadata(root.join("sub/Deep Note.md")).unwrap().len(), 0);
        // an explicit body is still honoured — this is a DEFAULT, not a filter
        create_note_in(&root, &mut ix, "Seeded", "given\n").unwrap();
        assert_eq!(fs::read_to_string(root.join("Seeded.md")).unwrap(), "given\n");
        let _ = fs::remove_dir_all(&root);
    }

    /* feedback #20 [R32.6]: the NEW-VAULT seed is starter CONTENT, not note
       creation — it stays (R1.2) — but it must not re-mint the title as bytes.
       Recon, stock 1.13.7, its own create-vault flow (shots5/E9): one file,
       `Welcome.md`, 203 bytes, first line `This is your new *vault*.` — no
       heading anywhere in it; the big "Welcome" is the inline title drawn from
       the filename. This test pins our seed to that SHAPE: a non-empty starter
       note whose bytes contain no ATX heading at all, and in particular not
       the vault-name heading we used to write. */
    #[test]
    fn f20_new_vault_seed_carries_prose_not_a_heading() {
        let root = tmp_vault("f20seed");
        seed_new_vault(&root).unwrap();
        let p = root.join(NEW_VAULT_SEED_NAME);
        assert_eq!(p.file_name().unwrap(), "Welcome.md", "stock seeds Welcome.md");
        let got = fs::read_to_string(&p).unwrap();
        assert_eq!(got, NEW_VAULT_SEED, "the seed on disk is the constant, byte for byte");
        // starter content is deliberate — this is NOT the zero-byte new-note path
        assert!(!got.is_empty(), "R1.2: a new vault still gets one starter note");
        // ...but nothing in it is a heading, and nothing repeats the filename
        for (i, line) in got.lines().enumerate() {
            assert!(
                !line.trim_start().starts_with('#'),
                "seed line {i} is a heading, the inline title already draws the name: {line:?}"
            );
        }
        assert!(!got.contains("# Welcome"), "the redundant `# Welcome` is gone");
        assert!(got.ends_with('\n'), "known delta from stock: our seed ends with a newline");
        let _ = fs::remove_dir_all(&root);
    }

    /* feedback #20 [acceptance 1]: ...and NO code path may re-mint a heading.
       The fix is at the two seams every creation path funnels through —
       ui/main.js `createNote` (the shared helper for all four JS paths) and
       the rust `create_note` command's default — so this test reads the JS
       source and pins BOTH: the helper's default body is the empty string,
       and the `"# " + name` construction exists nowhere in main.js. A string
       strip in one caller would not satisfy either half. */
    #[test]
    fn f20_no_code_path_materializes_a_heading_at_creation() {
        const UI: &str = include_str!("../../ui/main.js");
        assert!(
            UI.contains(r#"  const body = content != null ? content : "";"#),
            "createNote's default body must be EMPTY — that is the shared seam"
        );
        assert!(
            !UI.contains(r##""# " + name"##),
            "no creation path may mint a heading from the note name"
        );
        // exactly one call site invokes the backend command, and it passes the
        // helper's body — so the default above is the only default there is.
        assert_eq!(
            UI.matches(r#"inv("create_note""#).count(),
            1,
            "creation must funnel through the single createNote helper"
        );
        assert!(UI.contains(r#"inv("create_note", { name, content: body })"#));
        // and the backend must not synthesise one from a name either. The
        // needle is ASSEMBLED AT RUNTIME on purpose: include_str!("main.rs")
        // contains THIS test, so a literal would match itself and the
        // assertion would fail no matter what the production code does (it
        // did, on the first run — the failure was the test, not the fix).
        const RS: &str = include_str!("main.rs");
        let minted = ["format!(\"#", " {}"].concat();
        assert!(
            !RS.contains(&minted),
            "the backend must not synthesise a heading from a name either"
        );
        // the rust command's own default is empty, mirroring the JS seam.
        assert!(RS.contains("let content = content.unwrap_or_default();"));
    }

    /* feedback #20 [acceptance 2]: an EXISTING note is NOT ours to tidy.
       Now that the big title is the filename, a note whose body still starts
       with `# Foo` shows the title twice — exactly what stock does (recon Q6),
       and exactly what we must leave alone. The rule of order puts data loss
       above every fidelity argument: a "migration" that strips a redundant
       heading rewrites bytes the user typed, so none may exist.

       The cycle below is open -> render -> close at the PURE CORES the Tauri
       commands delegate to (read_capped for read_note, render_with for
       render, render_blocks_with for live preview, srcmode::highlight_block
       for source mode), plus Index::build for opening the vault itself — the
       only places that could plausibly rewrite a file. The oracle is the
       whole vault's bytes AND mtimes, so a rewrite that happened to produce
       identical content elsewhere would still be caught. */
    #[test]
    fn f20_an_existing_body_heading_survives_open_render_close_byte_for_byte() {
        let root = tmp_vault("f20keep");
        // written BEHIND the app's back: these are pre-existing user files
        let foo = "# Foo\n\nfirst paragraph\n\n# Foo\ntwo headings, both stay\n";
        let same = "# Same Name\n\nbody\n"; // Q6: the heading EQUALS the filename
        let odd = "# Keep\r\n\r\nCRLF, no trailing newline, trailing spaces   ";
        fs::write(root.join("Foo.md"), foo).unwrap();
        fs::write(root.join("Same Name.md"), same).unwrap();
        fs::write(root.join("Keep.md"), odd).unwrap();
        let files = ["Foo", "Same Name", "Keep"];
        let snap = |root: &Path| -> Vec<(Vec<u8>, std::time::SystemTime)> {
            files
                .iter()
                .map(|n| {
                    let p = root.join(format!("{n}.md"));
                    let m = fs::metadata(&p).unwrap();
                    (fs::read(&p).unwrap(), m.modified().unwrap())
                })
                .collect()
        };
        let before = snap(&root);

        // OPEN the vault, then per note: read, render (reading + LP + source)
        let ix = Index::build(&root);
        for n in files {
            let p = root.join(format!("{n}.md"));
            let text = read_capped(&p).expect("read_note's core must read it");
            let html = render_with(&text, ix.names(), ix.images(), true);
            let blocks: Vec<String> = text.split("\n\n").map(str::to_string).collect();
            let _ = render_blocks_with(&blocks, ix.names(), ix.images());
            let _: Vec<String> = blocks.iter().map(|b| srcmode::highlight_block(b)).collect();
            // the index must hold the file's bytes, not a cleaned-up copy
            assert_eq!(ix.content(n), Some(text.as_str()), "{n}: index != disk");
            if n == "Foo" {
                assert_eq!(
                    html.matches("<h1").count(),
                    2,
                    "both body headings must render — nothing is stripped: {html}"
                );
            }
            if n == "Same Name" {
                assert!(
                    html.contains("Same Name"),
                    "a heading equal to the filename is still the user's text (Q6): {html}"
                );
            }
        }
        drop(ix); // CLOSE the vault

        assert_eq!(before, snap(&root), "open+render+close rewrote a user file");
        // and re-opening the vault is not a second chance to rewrite anything
        let ix2 = Index::build(&root);
        assert_eq!(ix2.content("Foo"), Some(foo));
        drop(ix2);
        assert_eq!(before, snap(&root), "re-opening the vault rewrote a user file");

        // source level: no migration may exist, in either language. The note
        // OPEN path in the UI must not write — `setInlineTitle` publishes a
        // dataset attribute, it does not touch bytes.
        const UI: &str = include_str!("../../ui/main.js");
        let i = UI.find("async function loadActive(g) {").expect("loadActive moved");
        let end = UI[i..].find("\nasync function ").unwrap_or(UI.len() - i);
        let open_path = &UI[i..i + end];
        assert!(
            !open_path.contains("write_note"),
            "opening a note must never write it back"
        );
        assert!(open_path.contains("setInlineTitle"), "the open path draws the title");
        const RS2: &str = include_str!("main.rs");
        // needles ASSEMBLED AT RUNTIME: include_str!("main.rs") contains THIS
        // test, so a literal needle matches itself and the assertion fails no
        // matter what the production code does (it did, first run).
        for needle in [["fn mig", "rate"].concat(), ["strip_", "heading"].concat(), ["strip", "Title"].concat()] {
            assert!(!RS2.contains(&needle), "no migration may exist ({needle})");
            assert!(!UI.contains(&needle), "no migration may exist ({needle})");
        }
        let _ = fs::remove_dir_all(&root);
    }

    /* feedback #20 [acceptance 3]: the title left the bytes — FINDING must not
       have left with it. Before this goal the `# <name>` heading meant every
       note's own title was searchable as BODY TEXT; a zero-byte note has no
       body at all, so both find-by-title paths now rest on the FILENAME:

         search        -> search_docs' name arm (`name.contains(q)`), fed by
                          Index::docs(), which yields a name for every note
                          however empty its content is.
         quick switcher-> ui/main.js qsItems(), whose labels are notesCache =
                          the `list_notes` command = Index::names(); mdFilter
                          scores `it.label`, never a body.

       So the Rust half drives the REAL matcher over notes whose bodies cannot
       help (one zero-byte, one whose text never mentions its name), and the
       switcher half pins its data source + the field it matches on. */
    #[test]
    fn f20_a_note_is_findable_by_its_title_now_that_the_title_is_not_in_the_body() {
        let root = tmp_vault("f20find");
        let mut ix = Index::build(&root);
        // created the way the app creates them now: NOTHING on disk
        create_note_in(&root, &mut ix, "Project Ideas", "").unwrap();
        create_note_in(&root, &mut ix, "sub/Deep Thought", "").unwrap();
        // a note whose body never mentions its own name
        fs::write(root.join("Meeting Notes.md"), "agenda\ndiscussed the budget\n").unwrap();
        let ix = Index::build(&root);
        assert_eq!(fs::metadata(root.join("Project Ideas.md")).unwrap().len(), 0);

        // SEARCH: the query is title text and nothing else on disk carries it
        let hits = search_docs(ix.docs(), "project ideas");
        assert_eq!(hits.len(), 1, "a zero-byte note must still be findable by its title");
        assert_eq!(
            (hits[0].note.as_str(), hits[0].line, hits[0].snippet.as_str()),
            ("Project Ideas", 0, "Project Ideas"),
            "the hit is the NAME arm: line 0, snippet = the title itself"
        );
        // partial + case-insensitive + a nested note matched by its basename
        assert_eq!(search_docs(ix.docs(), "IDEAS")[0].note, "Project Ideas");
        assert_eq!(search_docs(ix.docs(), "thought")[0].note, "sub/Deep Thought");
        // ...and the body-less note is findable by title while its neighbour
        // is findable by body — the two arms are independent
        let m = search_docs(ix.docs(), "meeting");
        assert_eq!(m.len(), 1, "title hit only; the body never says 'meeting'");
        assert_eq!((m[0].note.as_str(), m[0].line), ("Meeting Notes", 0));
        let b = search_docs(ix.docs(), "budget");
        assert_eq!((b.len(), b[0].note.as_str(), b[0].line), (1, "Meeting Notes", 1));

        // and the name hit does NOT come from content: prove it on a doc stream
        // whose content is empty by construction.
        let docs = vec![("Only A Name".to_string(), String::new())];
        let h = search_docs(docs_ref(&docs), "only a name");
        assert_eq!((h.len(), h[0].snippet.as_str()), (1, "Only A Name"));

        // QUICK SWITCHER: its source is Index::names() via `list_notes`, which
        // lists a zero-byte note exactly like any other.
        assert_eq!(ix.names(), ["Meeting Notes", "Project Ideas", "sub/Deep Thought"]);
        assert_eq!(index::notes_of(&root), ix.names(), "the walk and the index agree");
        assert!(ix.content("Project Ideas").unwrap().is_empty(), "found by name, not by bytes");

        // the JS half: the switcher's items are FILENAMES and the filter scores
        // that label. A switcher that had matched on body text would need a
        // second source here; there is none.
        const UI: &str = include_str!("../../ui/main.js");
        let qs = {
            let i = UI.find("function qsItems() {").expect("qsItems moved");
            let end = UI[i..].find("\nfunction ").unwrap_or(UI.len() - i);
            &UI[i..i + end]
        };
        assert!(qs.contains("notesCache"), "the switcher lists the vault's NOTE NAMES");
        assert!(qs.contains("label: n,"), "each item's label IS the note name");
        assert!(!qs.contains("content") && !qs.contains("read_note"), "it never reads bodies");
        assert!(
            UI.contains(r#"mdItems = mdSrc().map((it, i) => [fuzzy(q, it.label), i, it])"#),
            "the filter scores the label"
        );
        assert!(
            UI.contains(r#"await Promise.all([inv("list_folders"), inv("list_notes"), inv("list_images")]);"#)
                && UI.contains("notesCache = notes;"),
            "notesCache is the list_notes command = Index::names()"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /* ---- R31 drop-to-attach (feedback #18) -----------------------------
       These hammer `attach_drop`, the ONE function the drop handler wraps.
       WHAT THEY DO NOT COVER, stated once here and in docs/features.md: the
       wry/GTK -> `tauri://drag-drop` transport. An OS drop cannot be
       synthesized (xdotool has no XDND), so faking a DOM drag event would
       test a code path that does not exist in production.               */

    /// a 40x40-ish png-shaped blob; content is never sniffed (S4), the
    /// EXTENSION decides, so the bytes only have to be recognisable again.
    fn src_file(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        fs::write(&p, body).unwrap();
        p
    }

    fn drop_vault(tag: &str) -> (PathBuf, PathBuf) {
        let root = tmp_vault(tag);
        fs::write(root.join("Note.md"), "# Note\n\nEND\n").unwrap();
        let src = std::env::temp_dir().join(format!("rustidian-{tag}-src-{}", std::process::id()));
        let _ = fs::remove_dir_all(&src);
        fs::create_dir_all(&src).unwrap();
        (root, src)
    }

    /// R31.1/R31.2/R31.3: one dropped png is COPIED (never moved) into the
    /// vault root and the inserted text is stock's wikilink embed, byte exact.
    #[test]
    fn drop_copies_the_file_and_returns_the_stock_link() {
        let (root, srcd) = drop_vault("drop-happy");
        let s = src_file(&srcd, "cat.png", b"\x89PNG\r\n\x1a\nCAT");
        let a = attach_drop(&root, "Note", &[s.clone()]).unwrap();
        assert_eq!(a.text, "![[cat.png]]", "R31.2: stock inserts ![[name]] and nothing else");
        assert_eq!(a.copied, vec!["cat.png".to_string()]);
        assert!(a.refused.is_empty());
        assert_eq!(fs::read(root.join("cat.png")).unwrap(), b"\x89PNG\r\n\x1a\nCAT");
        assert!(s.exists(), "R31.1: the source is COPIED, never moved");
        // and the byte server will serve exactly what we wrote (R29 reused)
        assert_eq!(serve_image(&root, "cat.png").unwrap().1, b"\x89PNG\r\n\x1a\nCAT".to_vec());
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&srcd);
    }

    /// R31.4 + S5: three drops of the SAME name give `cat.png`, `cat 1.png`,
    /// `cat 2.png` (stock's convention, verified against 1.13.7), and the
    /// earlier files are BYTE-IDENTICAL afterwards. A drop that overwrites a
    /// file the user already had is data loss, which outranks everything.
    #[test]
    fn drop_never_overwrites_and_renames_like_stock() {
        let (root, srcd) = drop_vault("drop-coll");
        fs::write(root.join("cat.png"), b"ALREADY-MINE").unwrap();
        let s = src_file(&srcd, "cat.png", b"\x89PNGnew");
        let a1 = attach_drop(&root, "Note", &[s.clone()]).unwrap();
        let a2 = attach_drop(&root, "Note", &[s.clone()]).unwrap();
        let a3 = attach_drop(&root, "Note", &[s.clone()]).unwrap();
        assert_eq!((a1.text.as_str(), a2.text.as_str(), a3.text.as_str()),
                   ("![[cat 1.png]]", "![[cat 2.png]]", "![[cat 3.png]]"));
        assert_eq!(fs::read(root.join("cat.png")).unwrap(), b"ALREADY-MINE",
                   "S5: the pre-existing file was CLOBBERED — that is the data-loss bug this test owns");
        for n in ["cat 1.png", "cat 2.png", "cat 3.png"] {
            assert_eq!(fs::read(root.join(n)).unwrap(), b"\x89PNGnew", "{n}");
        }
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&srcd);
    }

    /// S1: a file manager can hand us anything. A symlink is REFUSED, not
    /// followed (following it is how a drop reads /etc/shadow into the vault),
    /// and so is a directory, a fifo and a path that is simply gone.
    #[test]
    fn drop_refuses_symlink_dir_fifo_and_missing_sources() {
        use std::os::unix::fs::symlink;
        let (root, srcd) = drop_vault("drop-s1");
        let real = src_file(&srcd, "real.png", b"\x89PNGreal");
        let link = srcd.join("link.png");
        symlink(&real, &link).unwrap();
        let dir = srcd.join("dir.png");
        fs::create_dir_all(&dir).unwrap();
        let gone = srcd.join("gone.png");
        let a = attach_drop(&root, "Note", &[link, dir, gone]).unwrap();
        assert_eq!(a.text, "", "nothing may be inserted when everything was refused");
        assert!(a.copied.is_empty());
        assert_eq!(a.refused.len(), 3, "{:?}", a.refused);
        assert!(a.refused[0].contains("symlinks are not copied"), "{:?}", a.refused);
        assert!(a.refused[1].contains("not a regular file"), "{:?}", a.refused);
        assert!(a.refused[2].contains("not a regular file"), "{:?}", a.refused);
        assert_eq!(fs::read_dir(&root).unwrap().filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "png")).count(), 0);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&srcd);
    }

    /// the cap is MAX_IMG_BYTES, enforced on the SOURCE metadata before a byte
    /// is copied — an oversized image would not be served anyway (R29 S5), so
    /// copying it would only fill the user's disk.
    #[test]
    fn drop_refuses_an_oversized_source_before_copying() {
        let (root, srcd) = drop_vault("drop-big");
        let big = src_file(&srcd, "big.png", b"\x89PNG");
        let f = fs::OpenOptions::new().write(true).open(&big).unwrap();
        f.set_len(MAX_IMG_BYTES + 1).unwrap();
        let a = attach_drop(&root, "Note", &[big]).unwrap();
        assert_eq!(a.text, "");
        assert!(a.refused[0].contains("over the 32 MB limit"), "{:?}", a.refused);
        assert!(!root.join("big.png").exists(), "not one byte may land");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&srcd);
    }

    /// S4: the EXTENSION allowlist decides what is copied (IMG_TYPES == the
    /// list img_path_in serves), never a content sniff. svg included: stock
    /// renders it after sanitizing, we have no sanitizer (R29.8).
    #[test]
    fn drop_refuses_everything_outside_img_types() {
        let (root, srcd) = drop_vault("drop-ext");
        let mut paths = Vec::new();
        for n in ["doc.pdf", "arch.zip", "vec.svg", "pic.bmp", "note.md", "noext", "script.png.sh"] {
            paths.push(src_file(&srcd, n, b"\x89PNGdisguised-as-an-image"));
        }
        let a = attach_drop(&root, "Note", &paths).unwrap();
        assert_eq!(a.text, "", "a disguised payload is still refused: the extension decides");
        assert_eq!(a.refused.len(), 7, "{:?}", a.refused);
        assert!(a.refused[0].contains(".pdf is not an image"), "{:?}", a.refused);
        assert!(a.refused[5].contains("is not an image"), "{:?}", a.refused);
        // and the positive control in the same test: png/jpg/jpeg/gif/webp pass
        let ok: Vec<PathBuf> = IMG_TYPES.iter().map(|(e, _)| src_file(&srcd, &format!("ok.{e}"), b"\x89PNGok")).collect();
        let b = attach_drop(&root, "Note", &ok).unwrap();
        assert_eq!(b.copied.len(), IMG_TYPES.len(), "{:?}", b.refused);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&srcd);
    }

    /// S3: the basename is attacker-controlled TEXT. Traversal, NUL, control
    /// characters and a hidden leading dot are refused — safe_rel's rules,
    /// reused rather than re-invented.
    #[test]
    fn drop_refuses_unsafe_basenames() {
        let (root, srcd) = drop_vault("drop-name");
        let real = src_file(&srcd, "real.png", b"\x89PNGreal");
        // the OS cannot produce these as file_name(), a hostile drop source can
        let cases: Vec<PathBuf> = vec![
            srcd.join(".."),
            PathBuf::from(format!("{}/..%2f..%2fetc/../../evil.png", srcd.display())),
            PathBuf::from("/tmp/a\0b.png"),
            PathBuf::from("/tmp/bell\u{7}.png"),
            srcd.join(".hidden.png"),
        ];
        for c in &cases {
            let a = attach_drop(&root, "Note", std::slice::from_ref(c)).unwrap();
            assert_eq!(a.text, "", "{c:?} was ACCEPTED");
            assert_eq!(a.copied.len(), 0, "{c:?} was copied");
            assert_eq!(a.refused.len(), 1, "{c:?}: {:?}", a.refused);
        }
        // nothing landed, and the vault still has only the note + the control
        let a = attach_drop(&root, "Note", &[real]).unwrap();
        assert_eq!(a.copied, vec!["real.png".to_string()]);
        let pngs = fs::read_dir(&root).unwrap().filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x == "png")).count();
        assert_eq!(pngs, 1, "an unsafe basename created a file");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&srcd);
    }

    /// S2 the destination half, which R29 never needed: `attachmentFolderPath`
    /// pointing at a SYMLINK out of the vault must refuse the whole drop.
    /// Threat: vault/attachments -> /home/user, and every drop writes there.
    /// The positive control (a real subfolder works, and the link is still the
    /// BASENAME, which resolves by suffix) is in the same test on purpose.
    #[test]
    fn drop_refuses_a_destination_that_leaves_the_vault() {
        use std::os::unix::fs::symlink;
        let (root, srcd) = drop_vault("drop-dest");
        let outside = std::env::temp_dir().join(format!("rustidian-drop-out-{}", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        let s = src_file(&srcd, "cat.png", b"\x89PNGcat");
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        let set = |v: &str| fs::write(root.join(".obsidian/app.json"), format!("{{\"attachmentFolderPath\":\"{v}\"}}")).unwrap();
        // 1. a symlinked attachment dir: refused, and NOTHING is written outside
        symlink(&outside, root.join("att")).unwrap();
        set("att");
        assert_eq!(attach_drop(&root, "Note", &[s.clone()]), Err(DropErr::BadAttachDir));
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0, "a byte landed OUTSIDE the vault");
        // 2. traversal in the config value: not supported -> vault root + a
        //    VISIBLE notice, never a silent write somewhere else
        set("../evil");
        let a = attach_drop(&root, "Note", &[s.clone()]).unwrap();
        assert_eq!(a.copied, vec!["cat.png".to_string()]);
        assert!(a.refused[0].contains("is not supported"), "{:?}", a.refused);
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        // 3. positive control: a plain subfolder is honoured and created
        set("files/img");
        let b = attach_drop(&root, "Note", &[s.clone()]).unwrap();
        assert_eq!(b.text, "![[cat.png]]", "the LINK stays the basename (stock)");
        assert_eq!(b.copied, vec!["files/img/cat.png".to_string()], "the INDEX key is the relative path");
        assert!(root.join("files/img/cat.png").is_file());
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&srcd);
        let _ = fs::remove_dir_all(&outside);
    }

    /// a multi-file drop: every file copied, ORDER preserved, one link per
    /// line; and the two whole-drop failures (empty drop, no note open).
    #[test]
    fn drop_handles_multi_file_empty_and_noteless_drops() {
        let (root, srcd) = drop_vault("drop-multi");
        let p: Vec<PathBuf> = ["a.png", "b.jpg", "c.gif"].iter().map(|n| src_file(&srcd, n, format!("PNG-{n}").as_bytes())).collect();
        let mut all = p.clone();
        all.insert(2, src_file(&srcd, "nope.exe", b"MZ"));       // mixed drop
        let a = attach_drop(&root, "Note", &all).unwrap();
        assert_eq!(a.text, "![[a.png]]\n![[b.jpg]]\n![[c.gif]]", "one link per line, drop order");
        assert_eq!(a.copied, vec!["a.png", "b.jpg", "c.gif"]);
        assert_eq!(a.refused.len(), 1, "the refusal is REPORTED, the rest still attach");
        for n in ["a.png", "b.jpg", "c.gif"] {
            assert_eq!(fs::read(root.join(n)).unwrap(), format!("PNG-{n}").into_bytes());
        }
        assert_eq!(attach_drop(&root, "Note", &[]), Err(DropErr::Empty));
        assert_eq!(attach_drop(&root, "", &p).unwrap_err(), DropErr::NoNote);
        assert_eq!(attach_drop(&root, "../outside", &p).unwrap_err(), DropErr::NoNote);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&srcd);
    }

    /// R31.8 / R29.10: a browser drag delivers a URL, not a file. Refused —
    /// and the refusal SAYS "remote images are disabled" instead of reusing
    /// R29.4's "could not be found", which is an error that misdescribes its
    /// own cause.
    #[test]
    fn drop_refuses_a_remote_drag_with_an_honest_reason() {
        let (root, srcd) = drop_vault("drop-remote");
        let urls = ["https://evil.test/cat.png", "http://evil.test/cat.png", "data:image/png;base64,AAAA"];
        for u in urls {
            let a = attach_drop(&root, "Note", &[PathBuf::from(u)]).unwrap();
            assert_eq!(a.text, "");
            assert_eq!(a.refused, vec!["remote images are disabled".to_string()], "{u}");
            assert!(!a.refused[0].contains("could not be found"), "{u}: the R29.4 wording misdescribes a remote drag");
        }
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&srcd);
    }

    /// R29.11 for the drop path: a file copied but not INDEXED is invisible to
    /// both renderers, so `Index::add_image` must produce exactly what a fresh
    /// walk would (sorted, deduped) — otherwise the link paints [xi:1/0/1].
    #[test]
    fn dropped_images_enter_the_index_like_a_fresh_walk() {
        let (root, srcd) = drop_vault("drop-index");
        let mut ix = Index::build(&root);
        assert_eq!(ix.images(), Vec::<String>::new().as_slice());
        let a = attach_drop(&root, "Note", &[src_file(&srcd, "zed.png", b"\x89PNGz"), src_file(&srcd, "abe.png", b"\x89PNGa")]).unwrap();
        for r in &a.copied {
            ix.add_image(r);
            ix.add_image(r); // idempotent: a double drop must not double the list
        }
        assert_eq!(ix.images(), ["abe.png", "zed.png"], "sorted, deduped");
        assert_eq!(ix.images(), Index::build(&root).images(), "== a fresh walk");
        // and the resolver the renderers use finds it
        assert!(image_html(ix.images(), "zed.png", "").contains("rustidian-img://localhost/zed.png"));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&srcd);
    }

    /// R31.1 THE WIRING IS A STRING, and a string drifts. Rust emits
    /// DROP_EVENT from the WindowEvent::DragDrop handler; ui/main.js listens
    /// for that exact name and calls `attach_files`. Rename either end and the
    /// feature dies in total silence — the build is still green, the tests are
    /// still green, and a drop simply does nothing. Nothing else in this repo
    /// would notice, because ui/ is not a cargo input (docs/features.md trap).
    ///
    /// It also pins the handler as a PASS-THROUGH, which is the structural
    /// claim the whole design rests on: an OS drop cannot be synthesized in a
    /// test, so the part that IS tested must be the part that has the
    /// behaviour (`attach_drop`). A handler that grows a body is that claim
    /// quietly becoming false — this test fails at 9 lines.
    #[test]
    fn the_drop_handler_is_a_pass_through_and_the_ui_listens_for_the_same_name() {
        let src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs")).unwrap();
        let ui = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/main.js")).unwrap();
        let lines: Vec<&str> = src.lines().collect();
        let i = lines.iter().position(|l| l.contains(".on_window_event(")).expect("no OS drop is wired at all");
        let j = i + lines[i..].iter().position(|l| l.trim() == "})").expect("unterminated handler");
        let body = &lines[i..=j];
        assert!(body.len() <= 8, "the drop handler is {} lines: it is meant to be a pass-through", body.len());
        assert!(body.iter().any(|l| l.contains("WindowEvent::DragDrop")), "the handler must be the REAL OS drop event");
        assert!(body.iter().any(|l| l.contains("emit(DROP_EVENT")), "the handler must forward under the shared name");
        for forbidden in ["fs::", "copy", "canonicalize", "if !", "unwrap()"] {
            assert!(!body.iter().any(|l| l.contains(forbidden)), "policy or I/O ({forbidden}) leaked into the drop handler");
        }
        assert_eq!(DROP_EVENT, "drop-files");
        assert!(ui.contains(&format!("listen(\"{DROP_EVENT}\"")), "ui/main.js must listen for the name Rust emits");
        assert!(ui.contains("inv(\"attach_files\""), "the UI reaches attach_drop through the attach_files command");
        assert!(ui.contains("function attachDrop("), "the UI half of the drop has a name to grep for");
    }

    // ---------- R33 frameless window ----------
    /// The gesture math is the whole resize/move behaviour, and it is pure:
    /// anchor rect + cursor delta -> new outer rect. Every edge and corner is
    /// here because "resizable from every edge and corner" is the acceptance
    /// criterion an undecorated window is most likely to quietly lose.
    #[test]
    fn a_gesture_moves_only_the_edges_it_grabbed() {
        let r = (100.0, 50.0, 1000.0, 700.0);
        assert_eq!(gesture_rect("move", r, 30.0, -20.0), Some((130.0, 30.0, 1000.0, 700.0)), "move: position only");
        assert_eq!(gesture_rect("e", r, 40.0, 999.0), Some((100.0, 50.0, 1040.0, 700.0)), "east: width only, dy ignored");
        assert_eq!(gesture_rect("w", r, -60.0, 0.0), Some((40.0, 50.0, 1060.0, 700.0)), "west: x AND width (the east edge holds)");
        assert_eq!(gesture_rect("s", r, 999.0, 30.0), Some((100.0, 50.0, 1000.0, 730.0)), "south: height only, dx ignored");
        assert_eq!(gesture_rect("n", r, 0.0, -25.0), Some((100.0, 25.0, 1000.0, 725.0)), "north: y AND height (the south edge holds)");
        assert_eq!(gesture_rect("se", r, 10.0, 20.0), Some((100.0, 50.0, 1010.0, 720.0)), "SE corner");
        assert_eq!(gesture_rect("sw", r, -10.0, 20.0), Some((90.0, 50.0, 1010.0, 720.0)), "SW corner");
        assert_eq!(gesture_rect("ne", r, 10.0, -20.0), Some((100.0, 30.0, 1010.0, 720.0)), "NE corner");
        assert_eq!(gesture_rect("nw", r, -10.0, -20.0), Some((90.0, 30.0, 1010.0, 720.0)), "NW corner");
    }

    /// The clamp must PIN the dragged edge, not keep walking: dragging the west
    /// edge far to the right is the classic "the window slides away instead of
    /// refusing to shrink" bug. The opposite edge is the invariant.
    #[test]
    fn the_minimum_pins_the_dragged_edge_and_holds_the_opposite_one() {
        let r = (100.0, 50.0, 1000.0, 700.0);
        let (x, y, w, h) = gesture_rect("nw", r, 5000.0, 5000.0).unwrap();
        assert_eq!((w, h), (WIN_MIN_W, WIN_MIN_H), "clamped to the R22 floor");
        assert_eq!(x + w, 1100.0, "the east edge never moved");
        assert_eq!(y + h, 750.0, "the south edge never moved");
        let c = gesture_rect("se", r, -5000.0, -5000.0).unwrap();
        assert_eq!(c, (100.0, 50.0, WIN_MIN_W, WIN_MIN_H), "an SE clamp leaves the origin alone");
    }

    /// R33.6b THE PATH CHOICE, as a pure function so it is testable without a
    /// display: a probe needs a live GdkDisplay, a table of cases does not.
    /// Each row is one of the environments docs/recon-hdrdrag measured.
    #[test]
    fn the_drag_path_is_chosen_from_the_session_not_the_platform() {
        // E3, every Xvfb gate rig: no WM advertises the move protocol, so the
        // app's own geometry is the ONLY thing that can move the window. This is
        // the row that keeps phase panedrag passing for the reason it passes today
        // — and it holds whatever the empirical probe would say, which is the belt
        // on the braces: a wrong verdict cannot take the gate off its path.
        assert_eq!(drag_path(true, false, None), "none", "X11 with no WM: keep the anchor+delta path");
        assert_eq!(drag_path(true, false, Some(false)), "none", "no WM to hand to, whatever was measured");
        // E1, X11 + openbox: a WM offering _NET_WM_MOVERESIZE, and MEASURED to let
        // the window position itself exactly. Handing over there regresses it —
        // five consecutive handover drags went MOVED,DEAD,MOVED,DEAD,MOVED — so a
        // WM that merely EXISTS is not a reason to hand the press over.
        assert_eq!(drag_path(true, true, None), "none", "a WM exists, but nothing says our geometry fails");
        assert_eq!(drag_path(true, true, Some(true)), "none", "measured: the window moved when asked");
        // E2, the operator's bug: the request was issued and the window did not
        // move. Someone else owns the position — hand the press to them.
        assert_eq!(drag_path(true, true, Some(false)), "wm", "measured: client positioning is ignored");
        // Native Wayland: a client cannot position itself at all, whatever else is
        // advertised — there is no X11 root window to advertise it on.
        assert_eq!(drag_path(false, false, None), "wayland", "Wayland: only the compositor may move a toplevel");
        assert_eq!(drag_path(false, true, Some(true)), "wayland", "Wayland stays Wayland");
    }

    /// An unknown direction is an ERROR, never a silent no-op: win_gesture turns
    /// None into a rejected IPC call, so a renamed handle shows up as a broken
    /// gesture in the log instead of as a window that mysteriously will not move.
    #[test]
    fn an_unknown_gesture_direction_is_rejected() {
        let r = (0.0, 0.0, 800.0, 600.0);
        for bad in ["", "N", "en", "nn", "ns", "ew", "north", "resize", "moveto"] {
            assert_eq!(gesture_rect(bad, r, 10.0, 10.0), None, "{bad:?} must not be accepted as a direction");
        }
    }

    /// R33 THE CONFIG IS THE FEATURE, and on a display with no window manager it
    /// is the only thing that can prove it: without a WM every window renders
    /// undecorated, so no screenshot can tell `decorations: false` from the
    /// default. This pins the config, and the strip that has to replace the frame
    /// it removes — the DOM ids and the command names that move, resize, minimise
    /// and close the window. Delete any one of them and the window becomes
    /// unmovable or unclosable, which outranks every fidelity argument.
    #[test]
    fn the_window_is_undecorated_and_the_ui_replaces_every_affordance() {
        let conf = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&conf).unwrap();
        assert_eq!(v["app"]["windows"][0]["decorations"], serde_json::json!(false), "R33: the window must ship undecorated");
        let html = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/index.html")).unwrap();
        let js = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/main.js")).unwrap();
        let src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs")).unwrap();
        for id in ["wframe", "wf-min", "wf-max", "wf-close", "wrz"] {
            assert!(html.contains(&format!("id=\"{id}\"")), "ui/index.html lost #{id}");
        }
        for cmd in ["win_rect", "win_gesture", "win_move_proto", "win_drag_start", "win_minimize", "win_toggle_max", "win_close"] {
            assert!(js.contains(&format!("inv(\"{cmd}\"")), "ui/main.js no longer calls {cmd}");
            assert!(src.contains(&format!("fn {cmd}(")), "{cmd} is called by the UI but not implemented");
        }
        // all eight resize handles, or the undecorated window has lost an edge
        for d in ["n", "s", "e", "w", "ne", "nw", "se", "sw"] {
            assert!(html.contains(&format!("data-d=\"{d}\"")), "no resize handle for {d}");
        }
        // R33.9: the controls must be operable by KEYBOARD, not mouse only. Three
        // independent pieces, and losing any one of them makes the strip mouse-only:
        // real <button>s (Enter/Space activate them), a chord that REACHES the strip
        // from wherever focus is, and a focus ring so the user can see where they are.
        for id in ["wf-min", "wf-max", "wf-close"] {
            assert!(
                html.contains(&format!("<button id=\"{id}\"")),
                "#{id} is no longer a <button> — Enter/Space would stop activating it (R33.9)"
            );
            assert!(html.contains("aria-label"), "the window controls lost their aria-labels");
        }
        assert!(
            js.contains("ev.altKey") && js.contains("\"Spacebar\""),
            "ui/main.js lost the Alt+Space handler that focuses the frame strip — the controls would be mouse-only (R33.9)"
        );
        // and the SECOND chord, which is the one that survives a real desktop: openbox
        // (and most WMs) GRAB Alt+Space for their own client menu, so on a managed
        // display that chord never reaches the app. Losing F10 would make the strip
        // keyboard-operable only on a WM-less rig like our smoke display — i.e. green
        // here and mouse-only for the user (R33.9).
        assert!(
            js.contains("\"F10\""),
            "ui/main.js lost the F10 chord — Alt+Space alone is grabbed by the window manager, so the strip would be unreachable by keyboard on a real desktop (R33.9)"
        );
        assert!(
            js.contains("ArrowRight") && js.contains("#wframe button"),
            "ui/main.js lost the arrow-key roving focus between the window controls (R33.9)"
        );
        let css = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/style.css")).unwrap();
        assert!(
            css.contains("#wframe button:focus"),
            "ui/style.css lost the focus ring on the window controls: keyboard focus nobody can see is not operable (R33.9)"
        );
    }

    // ---------- tabclose: the instrumentation cannot drift ----------
    /// The operator's report was "it randomly closes", and the reason that
    /// could not be answered is that a removed tab left NO record. The fix for
    /// that is only as good as its weakest call site: one path that removes a
    /// tab without naming a cause rebuilds the blind spot exactly, and it does
    /// so silently, because a missing log line looks like a quiet run.
    ///
    /// So this test pins the structure, not the behaviour:
    ///   1. the five causes are the SAME five in both languages, in the same
    ///      order (Rust `TAB_REMOVAL_CAUSES` <-> JS `TAB_CAUSES`). ui/ is not a
    ///      cargo input, so nothing else would notice them diverging;
    ///   2. each of the four removal functions records one — the body of
    ///      `closeTab`, `dropTab`, `collapseGroup` and `enterVault` contains a
    ///      `tabGone(` call;
    ///   3. a tab only ever leaves a group through `g.tabs.splice(`, and every
    ///      occurrence of that splice is inside `closeTab` or `dropTab`. A new
    ///      fifth remover would fail here instead of in a bug report.
    #[test]
    fn tabclose_every_removal_path_names_a_cause() {
        let ui = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/main.js")).unwrap();

        // 1. the closed set, both sides, same order.
        let js_line = ui
            .lines()
            .find(|l| l.trim_start().starts_with("const TAB_CAUSES"))
            .expect("ui/main.js lost the TAB_CAUSES list — the causes are no longer a closed set");
        let js_causes: Vec<String> = js_line
            .split('"')
            .skip(1)
            .step_by(2)
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            js_causes,
            TAB_REMOVAL_CAUSES.to_vec(),
            "the JS and Rust cause lists have drifted: JS says {js_causes:?}, Rust says {TAB_REMOVAL_CAUSES:?} — a cause the backend rejects makes the log line disappear at the moment it matters"
        );

        // 2. every removal function records one. The body is taken from the
        //    function header to the next top-level `\n}` so a call in a
        //    NEIGHBOURING function cannot satisfy the assertion.
        let body_of = |header: &str| -> String {
            let at = ui
                .find(header)
                .unwrap_or_else(|| panic!("ui/main.js no longer contains `{header}`"));
            let rest = &ui[at..];
            let end = rest.find("\n}").unwrap_or(rest.len());
            rest[..end].to_string()
        };
        for header in [
            "async function closeTab(",
            "async function dropTab(",
            "async function collapseGroup(",
            "async function enterVault(",
        ] {
            assert!(
                body_of(header).contains("tabGone("),
                "`{header}` removes tabs without recording a cause — that is the blind spot this instrumentation exists to close"
            );
        }

        // 3. and nothing else takes a tab out of a group. THREE splices, each
        //    accounted for by name: two destroy the tab (closeTab, dropTab)
        //    and one MOVES it to another group (tabDragStart) — the moved tab
        //    still exists, so it is not a removal cause, but the pane it left
        //    may collapse, and that collapse passes its own `via`.
        let splices = ui.matches("g.tabs.splice(").count();
        assert_eq!(
            splices, 3,
            "ui/main.js has {splices} `g.tabs.splice(` sites, not the 3 (closeTab, dropTab, tabDragStart) this test knows how to account for — a new one must name a cause before it ships"
        );
        for header in ["async function closeTab(", "async function dropTab("] {
            assert!(
                body_of(header).contains("g.tabs.splice("),
                "`{header}` no longer splices the tab out — the splice census above is measuring the wrong functions"
            );
        }
        let drag = body_of("function tabDragStart(");
        assert!(
            drag.contains("g.tabs.splice("),
            "the third splice is no longer the drag — re-derive the census above"
        );
        for via in ["\"tabdrag-strip\"", "\"tabdrag-edge\""] {
            assert!(
                drag.contains(&format!("collapseGroup(g, {via})")),
                "a drag that empties its source pane must say which drag did it ({via}) — 'the pane vanished' with no via is the report we could not answer"
            );
        }
    }

    /// The backend half: an unknown cause is an ERROR, never a shrug. A
    /// pass-through would log `cause=whatever` and read as evidence, which is
    /// worse than no line at all — the census would agree with a caller that
    /// invented its own vocabulary.
    #[test]
    fn tabclose_an_unknown_cause_is_rejected() {
        let ok = tab_removed(
            "external-delete".into(),
            "ZZ-Note".into(),
            true,
            false,
            true,   // preserved: the watcher path parks the bytes it may not write back
            "onVaultChanged#1".into(),
            1,
            0,
            1,
        );
        assert!(ok.is_ok(), "a legitimate cause must be accepted: {ok:?}");
        let bad = tab_removed(
            "mystery".into(),
            "ZZ-Note".into(),
            true,
            false,
            false,
            "?".into(),
            2,
            0,
            1,
        );
        assert!(
            bad.is_err(),
            "an unknown cause must be an error — 'something else removed it' is the diagnosis that was missing"
        );
        assert!(
            bad.unwrap_err().contains("mystery"),
            "the rejection must name the cause it refused, or the log cannot identify the bad caller"
        );
    }
}
