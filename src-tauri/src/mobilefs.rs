// SPDX-License-Identifier: GPL-3.0-or-later
/* mobilefs — android-p2b: what the phone's drawer and long-press sheets need from the vault
   and the backend had no command for (rubric M4.4 M4.5 M7.5 M7.6).

   - `stats`: every real file and folder below the root (dot entries such as .obsidian and
     .trash skipped, symlinks skipped: DirEntry::file_type does not follow them), each with its
     modified and created time in ms. The drawer's "N files, M folders" line, the Files sort
     orders and the file sheet's "Created at" read this. A platform with no birth time (Android
     bionic: std has no statx there) reports the modified time as the created time.
   - `folder_rename` / `folder_trash`: a folder rename and a folder delete. Both are ONE
     renameat through vaultfs (parent dirs walked no-follow from the vault root, a symlinked leaf
     refused), the delete into the vault's own .trash/ like delete_note. Links into the folder
     are NOT rewritten (a note rename's link update is per note).
   Nothing here runs on a desktop path: only ui/mobile.js calls these commands. */
use crate::vaultfs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

fn ms(t: std::io::Result<std::time::SystemTime>) -> Option<i64> {
    t.ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as i64)
}

pub fn stats(root: &Path) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    walk_at(root, root, &mut out, 0);
    out
}

fn walk_at(dir: &Path, base: &Path, out: &mut Vec<serde_json::Value>, depth: usize) {
    if depth >= vaultfs::MAX_WALK_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if e.file_name().as_bytes().first() == Some(&b'.') {
            continue;
        }
        let Ok(t) = e.file_type() else { continue };
        if !(t.is_dir() || t.is_file()) {
            continue;
        }
        let p = e.path();
        let Ok(md) = std::fs::symlink_metadata(&p) else { continue };
        let mtime = ms(md.modified()).unwrap_or(0);
        let ctime = ms(md.created()).unwrap_or(mtime);
        if let Ok(rel) = p.strip_prefix(base) {
            out.push(serde_json::json!({ "path": rel.display().to_string(), "dir": t.is_dir(), "mtime": mtime, "ctime": ctime }));
        }
        if t.is_dir() {
            walk_at(&p, base, out, depth + 1);
        }
    }
}

fn is_folder(vfs: &vaultfs::Vault, rel: &Path) -> Result<(), String> {
    match vfs.p_stat(rel) {
        Ok(st) if st.kind == "folder" => Ok(()),
        Ok(_) => Err("not a folder".into()),
        Err(e) => Err(e.to_string()),
    }
}

pub fn folder_rename(root: &Path, from: &Path, to: &Path) -> Result<(), String> {
    let vfs = vaultfs::Vault::open(root).map_err(|e| e.to_string())?;
    is_folder(&vfs, from)?;
    if to.starts_with(from) {
        return Err("a folder cannot move into itself".into());
    }
    if vfs.p_stat(to).is_ok() {
        return Err("a file or folder with that name already exists".into());
    }
    vfs.p_rename(from, to).map_err(|e| e.to_string())
}

pub fn folder_trash(root: &Path, rel: &Path) -> Result<String, String> {
    let vfs = vaultfs::Vault::open(root).map_err(|e| e.to_string())?;
    is_folder(&vfs, rel)?;
    let tdir = Path::new(".trash");
    vfs.mkdir_p(tdir).map_err(|e| e.to_string())?;
    let base = rel.file_name().map(|b| b.to_string_lossy().into_owned()).ok_or("invalid folder")?;
    for n in 0..1000 {
        let dest = tdir.join(if n == 0 { base.clone() } else { format!("{base} {n}") });
        if vfs.p_stat(&dest).is_ok() {
            continue;
        }
        vfs.p_rename(rel, &dest).map_err(|e| e.to_string())?;
        return Ok(dest.display().to_string());
    }
    Err("no free name in .trash".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn stats_rename_trash() {
        let root = std::env::temp_dir().join(format!("mobilefs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        fs::write(root.join("a/n.md"), "x").unwrap();
        fs::write(root.join("a/b/m.md"), "y").unwrap();
        fs::write(root.join(".obsidian/app.json"), "{}").unwrap();
        std::os::unix::fs::symlink(root.join("a"), root.join("ln")).unwrap();
        let s = stats(&root);
        let files = s.iter().filter(|e| e["dir"] == false).count();
        let dirs = s.iter().filter(|e| e["dir"] == true).count();
        assert_eq!((files, dirs), (2, 2));
        assert!(s.iter().all(|e| e["ctime"].as_i64().unwrap() > 0));
        assert!(folder_rename(&root, Path::new("a"), Path::new("a/b/c")).is_err());
        assert!(folder_rename(&root, Path::new("ln"), Path::new("z")).is_err());
        folder_rename(&root, Path::new("a"), Path::new("z")).unwrap();
        assert!(root.join("z/b/m.md").is_file());
        assert_eq!(folder_trash(&root, Path::new("z/b")).unwrap(), ".trash/b");
        fs::create_dir_all(root.join("z/b")).unwrap();
        assert_eq!(folder_trash(&root, Path::new("z/b")).unwrap(), ".trash/b 1");
        assert!(folder_trash(&root, Path::new("z/n.md")).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
