//! perf-index: in-memory vault index. Built ONCE per set_vault/create_vault/
//! boot (one walk + one read per .md), then kept in lock-step with disk by
//! the only writer — this process (write_note / rename_note). search, graph,
//! backlinks, resolve and render_blocks serve from here: zero disk reads.
//! LATER: file watcher for external edits (docs/perf.md).
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct NoteMeta {
    pub content: String,
    /// raw [[link]] texts in source order (aliases/anchors NOT stripped —
    /// same tokens links_in has always produced; resolve() gets them raw)
    pub links: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Index {
    /// sorted by name — BTreeMap iteration order == the old notes_of() order
    notes: BTreeMap<String, NoteMeta>,
    /// resolved target -> sorted sources (self-links excluded)
    backlinks: HashMap<String, Vec<String>>,
    /// sorted keys, cached: render_md/resolve want a &[String]
    names: Vec<String>,
}

pub fn links_in(s: &str) -> Vec<String> {
    let (mut out, mut rest) = (Vec::new(), s);
    while let Some(a) = rest.find("[[") {
        rest = &rest[a + 2..];
        match rest.find("]]") {
            Some(b) => {
                out.push(rest[..b].to_string());
                rest = &rest[b + 2..];
            }
            None => break,
        }
    }
    out
}

/// wikilinks resolve by full relative path or basename (first sorted match)
pub fn resolve(notes: &[String], l: &str) -> Option<usize> {
    notes
        .iter()
        .position(|x| *x == l || x.ends_with(&format!("/{l}")))
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            walk(&p, base, out);
        } else if let Some(stem) = name.strip_suffix(".md") {
            let rel = p
                .parent()
                .and_then(|d| d.strip_prefix(base).ok())
                .unwrap_or(Path::new(""));
            out.push(if rel.as_os_str().is_empty() {
                stem.to_string()
            } else {
                format!("{}/{}", rel.display(), stem)
            });
        }
    }
}

/// sorted note names under root (the one and only vault walk)
pub fn notes_of(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

impl Index {
    /// one walk + one read per note; links parsed once
    pub fn build(root: &Path) -> Index {
        let mut ix = Index::default();
        for n in notes_of(root) {
            let c = fs::read_to_string(format!("{}.md", root.join(&n).display()))
                .unwrap_or_default();
            ix.notes.insert(n, NoteMeta { links: links_in(&c), content: c });
        }
        ix.refresh_names();
        ix.rebuild_backlinks();
        ix
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.notes.len()
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn content(&self, name: &str) -> Option<&str> {
        self.notes.get(name).map(|m| m.content.as_str())
    }

    pub fn links(&self, name: &str) -> &[String] {
        self.notes.get(name).map(|m| m.links.as_slice()).unwrap_or(&[])
    }

    /// (name, content) in sorted order — search's document stream
    pub fn docs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.notes.iter().map(|(n, m)| (n.as_str(), m.content.as_str()))
    }

    /// (name, links) in sorted order — graph's edge stream
    pub fn link_lists(&self) -> Vec<&[String]> {
        self.notes.values().map(|m| m.links.as_slice()).collect()
    }

    /// notes linking to `name`, sorted (self excluded)
    pub fn backlinks(&self, name: &str) -> Vec<String> {
        self.backlinks.get(name).cloned().unwrap_or_default()
    }

    fn refresh_names(&mut self) {
        self.names = self.notes.keys().cloned().collect();
    }

    /// full edge recompute from memory: O(total links x notes), no disk
    fn rebuild_backlinks(&mut self) {
        self.backlinks.clear();
        let pairs: Vec<(String, Vec<String>)> = self
            .notes
            .iter()
            .map(|(n, m)| (n.clone(), m.links.clone()))
            .collect();
        for (src, links) in pairs {
            for l in &links {
                self.add_edge(&src, l);
            }
        }
    }

    fn add_edge(&mut self, src: &str, link: &str) {
        let Some(j) = resolve(&self.names, link) else { return };
        let tgt = self.names[j].clone();
        if tgt == src {
            return;
        }
        let v = self.backlinks.entry(tgt).or_default();
        if let Err(pos) = v.binary_search_by(|x| x.as_str().cmp(src)) {
            v.insert(pos, src.to_string());
        }
    }

    fn drop_edge(&mut self, src: &str, link: &str) {
        let Some(j) = resolve(&self.names, link) else { return };
        let tgt = &self.names[j];
        if let Some(v) = self.backlinks.get_mut(tgt) {
            v.retain(|x| x != src);
            if v.is_empty() {
                let t = tgt.clone();
                self.backlinks.remove(&t);
            }
        }
    }

    /// write_note: reparse that note; existing key -> drop its old outgoing
    /// edges, add the new ones. New key -> the name set changed, so previously
    /// unresolved (or basename-ambiguous) links anywhere may now resolve here:
    /// full edge rebuild from memory (rare path, no disk).
    pub fn upsert(&mut self, name: &str, content: &str) {
        let links = links_in(content);
        match self.notes.get_mut(name) {
            Some(m) => {
                let old = std::mem::replace(&mut m.links, links.clone());
                m.content = content.to_string();
                for l in &old {
                    self.drop_edge(name, l);
                }
                for l in &links {
                    self.add_edge(name, l);
                }
            }
            None => {
                self.notes.insert(name.to_string(), NoteMeta { content: content.to_string(), links });
                self.refresh_names();
                self.rebuild_backlinks();
            }
        }
    }

    /// rename_note (after the fs move): move the key, rewrite [[old]] targets
    /// in every note (same ambiguity rule as before: basename links only when
    /// no other note carries old's basename and none but the renamed note
    /// carries new's), rebuild edges. Returns (name, new content) for every
    /// note whose text changed — the caller writes those to disk.
    pub fn rename(&mut self, old: &str, new: &str, fallback: Option<String>) -> Vec<(String, String)> {
        let meta = self
            .notes
            .remove(old)
            .or_else(|| fallback.map(|c| NoteMeta { links: links_in(&c), content: c }))
            .unwrap_or_default();
        self.notes.insert(new.to_string(), meta);
        self.refresh_names();
        let ob = old.rsplit('/').next().unwrap_or(old);
        let nb = new.rsplit('/').next().unwrap_or(new);
        let bn_ok = !self.names.iter().any(|n| {
            let b = n.rsplit('/').next().unwrap_or(n);
            b == ob || (b == nb && n != new)
        });
        let mut changed = Vec::new();
        for (n, m) in self.notes.iter_mut() {
            let (nc, did) = rewrite_links(&m.content, old, new, bn_ok);
            if did {
                m.links = links_in(&nc);
                m.content = nc.clone();
                changed.push((n.clone(), nc));
            }
        }
        self.rebuild_backlinks();
        changed
    }
}

/* ux-3: vault-wide wikilink rewrite on rename. [[Old]] -> [[New]],
   [[Old|alias]] keeps alias, [[Old#h]] keeps anchor. Basename-style links
   ([[A]] for sub/A) stay basename-style; full-path links get the full new
   path. bn_ok=false disables basename matching (caller found ANOTHER note
   with the same basename — those links now resolve elsewhere, leave them). */
pub fn rewrite_links(s: &str, old: &str, new: &str, bn_ok: bool) -> (String, bool) {
    let ob = old.rsplit('/').next().unwrap_or(old);
    let nb = new.rsplit('/').next().unwrap_or(new);
    let (mut out, mut changed) = (String::new(), false);
    let mut rest = s;
    while let Some(a) = rest.find("[[") {
        out.push_str(&rest[..a + 2]);
        rest = &rest[a + 2..];
        let Some(b) = rest.find("]]") else { break };
        let inner = &rest[..b];
        rest = &rest[b + 2..];
        let (tgt, alias) = match inner.find('|') {
            Some(i) => (&inner[..i], &inner[i..]),
            None => (inner, ""),
        };
        let (base, anchor) = match tgt.find('#') {
            Some(i) => (&tgt[..i], &tgt[i..]),
            None => (tgt, ""),
        };
        let rep = if base == old {
            Some(new)
        } else if bn_ok && base == ob {
            Some(nb)
        } else {
            None
        };
        match rep {
            Some(r) => {
                changed = true;
                out.push_str(r);
                out.push_str(anchor);
                out.push_str(alias);
            }
            None => out.push_str(inner),
        }
        out.push_str("]]");
    }
    out.push_str(rest);
    (out, changed)
}
