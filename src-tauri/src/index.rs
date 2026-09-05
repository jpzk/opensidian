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
    /// tags: unique, sorted. inline #tag tokens outside code/URLs plus
    /// frontmatter `tags:` (YAML list or comma string), no leading '#'
    pub tags: Vec<String>,
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

/// tag char set (Obsidian): unicode letters/digits, '-', '_', '/'
fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_' || c == '/'
}

/// a tag token must start with a letter or '_' (so #1 / #123 are not tags);
/// trailing '/' is trimmed. Returns the cleaned tag or None.
fn clean_tag(t: &str) -> Option<&str> {
    let t = t.trim_end_matches('/');
    let first = t.chars().next()?;
    if !(first.is_alphabetic() || first == '_') || !t.chars().all(is_tag_char) {
        return None;
    }
    Some(t)
}

/// inline #tag spans in plain text: '#' at start or after whitespace, then a
/// tag token. `https://x/y#frag` has 'y' before '#', so URL fragments never
/// match; `# Heading` has a space after '#'. Yields (byte start, byte end),
/// end excludes any trimmed trailing '/'.
pub fn tag_spans(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut prev_ws = true;
    let mut i = 0;
    while i < s.len() {
        let c = s[i..].chars().next().unwrap();
        if c == '#' && prev_ws {
            let body = &s[i + 1..];
            let n: usize = body.chars().take_while(|&c| is_tag_char(c)).map(char::len_utf8).sum();
            if let Some(t) = clean_tag(&body[..n]) {
                out.push((i, i + 1 + t.len()));
                i += 1 + n;
                prev_ws = false;
                continue;
            }
        }
        prev_ws = c.is_whitespace();
        i += c.len_utf8();
    }
    out
}

/// frontmatter block: content starting with a `---` line up to the next
/// `---`/`...` line. Returns (yaml lines, byte offset of the body).
fn frontmatter(s: &str) -> Option<(Vec<&str>, usize)> {
    let mut lines = s.split_inclusive('\n');
    let first = lines.next()?;
    if first.trim_end() != "---" {
        return None;
    }
    let mut off = first.len();
    let mut yaml = Vec::new();
    for l in lines {
        off += l.len();
        let t = l.trim_end();
        if t == "---" || t == "..." {
            return Some((yaml, off));
        }
        yaml.push(t);
    }
    None
}

/// frontmatter `tags:`/`tag:` -> cleaned tags. Accepts `tags: a, b`,
/// `tags: [a, "b"]`, `tags: #a #b`, and the block-list form (`- a` lines).
fn frontmatter_tags(yaml: &[&str], out: &mut Vec<String>) {
    let mut push = |raw: &str| {
        let t = raw.trim().trim_matches(|c| c == '"' || c == '\'').trim_start_matches('#');
        if let Some(t) = clean_tag(t) {
            out.push(t.to_string());
        }
    };
    let mut i = 0;
    while i < yaml.len() {
        let l = yaml[i];
        i += 1;
        let Some(rest) = l.strip_prefix("tags:").or_else(|| l.strip_prefix("tag:")) else { continue };
        let rest = rest.trim().trim_start_matches('[').trim_end_matches(']');
        if !rest.is_empty() {
            rest.split(|c: char| c == ',' || c.is_whitespace()).for_each(&mut push);
            continue;
        }
        // block list: following `- item` lines (indent allowed)
        while i < yaml.len() {
            let Some(item) = yaml[i].trim_start().strip_prefix("- ") else { break };
            push(item);
            i += 1;
        }
    }
}

/// every tag of a note: frontmatter + inline (fenced ``` / ~~~ blocks and
/// `inline code` spans skipped), deduped + sorted
pub fn tags_in(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let body = match frontmatter(s) {
        Some((yaml, off)) => {
            frontmatter_tags(&yaml, &mut out);
            &s[off..]
        }
        None => s,
    };
    let mut fence: Option<&str> = None;
    for l in body.lines() {
        let t = l.trim_start();
        if let Some(f) = fence {
            if t.starts_with(f) {
                fence = None;
            }
            continue;
        }
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = Some(&t[..3]);
            continue;
        }
        // odd segments between backticks are code spans (unbalanced tick:
        // the tail counts as code, like a stray ` would in most renderers)
        for (k, seg) in l.split('`').enumerate() {
            if k % 2 == 1 {
                continue;
            }
            for (a, b) in tag_spans(seg) {
                out.push(seg[a + 1..b].to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// content -> NoteMeta (links + tags parsed once)
pub fn parse(c: String) -> NoteMeta {
    NoteMeta { links: links_in(&c), tags: tags_in(&c), content: c }
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

/// R10.1: `[[note#anchor|alias]]` -> (note, anchor incl. its leading '#' or
/// "", alias or ""). `[[#Heading]]` yields an empty note = the current note.
pub fn link_parts(l: &str) -> (&str, &str, &str) {
    let (tgt, alias) = match l.find('|') {
        Some(i) => (&l[..i], &l[i + 1..]),
        None => (l, ""),
    };
    let (note, anchor) = match tgt.find('#') {
        Some(i) => (&tgt[..i], &tgt[i..]),
        None => (tgt, ""),
    };
    (note, anchor, alias)
}

/// wikilinks resolve by full relative path or basename (first sorted match);
/// the raw token may carry #anchor / |alias — only the note part is matched
pub fn resolve(notes: &[String], l: &str) -> Option<usize> {
    let l = link_parts(l).0;
    if l.is_empty() {
        return None;
    }
    // one suffix alloc per call — it used to be one per note per link, so a
    // write_note on a 9KB note walked ~50k allocs (2ms debug); graph paid it
    // per edge too
    let suffix = format!("/{l}");
    notes.iter().position(|x| *x == l || x.ends_with(&suffix))
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
            ix.notes.insert(n, parse(c));
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

    /// tags of one note, sorted + unique (no leading '#')
    pub fn tags(&self, name: &str) -> &[String] {
        self.notes.get(name).map(|m| m.tags.as_slice()).unwrap_or(&[])
    }

    /// tag -> number of notes carrying it (exact tag; nested tags count on
    /// their own key, so a/b does not bump a). O(notes) from memory.
    pub fn tag_counts(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        for m in self.notes.values() {
            for t in &m.tags {
                *out.entry(t.clone()).or_insert(0) += 1;
            }
        }
        out
    }

    /// (name, content, tags) in sorted order — search's document stream
    pub fn docs(&self) -> impl Iterator<Item = (&str, &str, &[String])> {
        self.notes.iter().map(|(n, m)| (n.as_str(), m.content.as_str(), m.tags.as_slice()))
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
                m.content = content.to_string();
                m.tags = tags_in(content);
                if m.links == links {
                    // plain save, link set unchanged (the common case): no
                    // edge work at all
                    return;
                }
                let old = std::mem::replace(&mut m.links, links.clone());
                for l in &old {
                    self.drop_edge(name, l);
                }
                for l in &links {
                    self.add_edge(name, l);
                }
            }
            None => {
                self.notes.insert(name.to_string(), parse(content.to_string()));
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
            .or_else(|| fallback.map(parse))
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
        let (base, anchor, alias) = link_parts(inner);
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
                if inner.contains('|') {
                    out.push('|');
                    out.push_str(alias);
                }
            }
            None => out.push_str(inner),
        }
        out.push_str("]]");
    }
    out.push_str(rest);
    (out, changed)
}
