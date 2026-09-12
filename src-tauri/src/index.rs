//! perf-index: in-memory vault index. Built ONCE per set_vault/create_vault/
//! boot (one walk + one read per .md), then kept in lock-step with disk by
//! the only writer — this process (write_note / rename_note). search, graph,
//! backlinks, resolve and render_blocks serve from here: zero disk reads.
//! LATER: file watcher for external edits (docs/perf.md).
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// S5: notes larger than this are not part of the vault (walk skips them,
/// read_note returns ""); a multi-GB .md in a synced vault must not OOM us
pub const MAX_NOTE_BYTES: u64 = 32 * 1024 * 1024;

/// S5: the watcher re-walks every second — complain once per oversized path
pub fn warn_oversized(p: &Path) {
    static SEEN: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let mut seen = SEEN.get_or_init(|| Mutex::new(HashSet::new())).lock().unwrap();
    if seen.insert(p.to_path_buf()) {
        eprintln!("rustidian: skipping {} (> {} MiB)", p.display(), MAX_NOTE_BYTES >> 20);
    }
}

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
    /// R29.6: sorted vault-relative image paths (with extension). NOT notes —
    /// they never enter `notes`, they only answer "does this embed resolve?"
    /// for BOTH renderers, so the two agree by construction.
    images: Vec<String>,
    /// R19: graph (nodes + edges + adjacency) built lazily from memory and
    /// kept until the edge set changes (link edit, new/removed/renamed note).
    /// graph / graph_local serve from here: no per-call rebuild, no disk.
    pub(crate) graph: Option<GraphCache>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct GNode {
    pub name: String,
    pub resolved: bool,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Graph {
    pub nodes: Vec<GNode>,
    pub edges: Vec<(usize, usize)>,
}

/// graph + per-node adjacency (out / in lists) for the local-graph BFS
#[derive(Debug, Default)]
pub struct GraphCache {
    pub graph: Graph,
    pub out: Vec<Vec<usize>>,
    pub inc: Vec<Vec<usize>>,
}

impl GraphCache {
    fn new(graph: Graph) -> GraphCache {
        let n = graph.nodes.len();
        let (mut out, mut inc) = (vec![Vec::new(); n], vec![Vec::new(); n]);
        for &(a, b) in &graph.edges {
            out[a].push(b);
            inc[b].push(a);
        }
        GraphCache { graph, out, inc }
    }

    /// R7.2 neighbourhood of `center` (same shape as the old JS lgFilter):
    /// BFS `depth` hops over outgoing (`out`) / incoming (`inc`) edges, keeps
    /// EVERY edge among the surviving nodes, node order = discovery order,
    /// indices remapped. Unknown centre -> empty graph.
    pub fn local(&self, center: &str, depth: usize, inc: bool, out: bool) -> Graph {
        let Some(ci) = self.graph.nodes.iter().position(|n| n.name == center) else {
            return Graph::default();
        };
        let mut keep = vec![usize::MAX; self.graph.nodes.len()];
        let mut order = vec![ci];
        keep[ci] = 0;
        let mut frontier = vec![ci];
        for _ in 0..depth {
            if frontier.is_empty() {
                break;
            }
            let mut next = Vec::new();
            for &f in &frontier {
                let mut visit = |j: usize, next: &mut Vec<usize>| {
                    if keep[j] == usize::MAX {
                        keep[j] = order.len();
                        order.push(j);
                        next.push(j);
                    }
                };
                if out {
                    for &j in &self.out[f] {
                        visit(j, &mut next);
                    }
                }
                if inc {
                    for &j in &self.inc[f] {
                        visit(j, &mut next);
                    }
                }
            }
            frontier = next;
        }
        Graph {
            nodes: order.iter().map(|&i| self.graph.nodes[i].clone()).collect(),
            edges: self
                .graph
                .edges
                .iter()
                .filter(|(a, b)| keep[*a] != usize::MAX && keep[*b] != usize::MAX)
                .map(|(a, b)| (keep[*a], keep[*b]))
                .collect(),
        }
    }
}

/// R4.2: notes = resolved nodes; wikilinks to nonexistent notes become
/// unresolved nodes (deduped by link text), so the graph shows ghost targets.
/// Resolution = resolve()'s rule (first sorted note equal to the link or
/// ending in "/link") served from two maps instead of a scan per link; edge
/// dedupe via a set (was Vec::contains, O(E) per link).
pub fn build_graph(notes: &[String], links: &[&[String]]) -> Graph {
    let mut nodes: Vec<GNode> = notes
        .iter()
        .map(|n| GNode { name: n.clone(), resolved: true })
        .collect();
    let mut full: HashMap<&str, usize> = HashMap::with_capacity(notes.len());
    let mut base: HashMap<&str, usize> = HashMap::new();
    for (i, n) in notes.iter().enumerate() {
        full.entry(n.as_str()).or_insert(i);
        if let Some((_, b)) = n.rsplit_once('/') {
            base.entry(b).or_insert(i);
        }
    }
    let mut ghosts: HashMap<String, usize> = HashMap::new();
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    let mut edges = Vec::new();
    for (i, ls) in links.iter().enumerate() {
        for l in ls.iter() {
            let l = link_parts(l).0; // ghost nodes carry the note part only
            if l.is_empty() {
                continue; // [[#heading]] = self-link
            }
            // "a/b" links (a longer path suffix) keep the scan: rare, and resolve() is the rule
            let hit = if l.contains('/') {
                resolve(notes, l)
            } else {
                match (full.get(l), base.get(l)) {
                    (Some(a), Some(b)) => Some(*a.min(b)),
                    (a, b) => a.or(b).copied(),
                }
            };
            let j = match hit {
                Some(j) => j,
                None => match ghosts.get(l) {
                    Some(&j) => j,
                    None => {
                        nodes.push(GNode { name: l.to_string(), resolved: false });
                        ghosts.insert(l.to_string(), nodes.len() - 1);
                        nodes.len() - 1
                    }
                },
            };
            if i != j && seen.insert((i, j)) {
                edges.push((i, j));
            }
        }
    }
    Graph { nodes, edges }
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

fn walk(dir: &Path, base: &Path, out: &mut Vec<String>, imgs: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let p = e.path();
        // S2: lstat, never follow — a symlinked dir/file is not part of the
        // vault (it would index and serve whatever it points at)
        let Ok(m) = fs::symlink_metadata(&p) else { continue };
        if m.is_symlink() {
            continue;
        }
        if m.is_dir() {
            walk(&p, base, out, imgs);
        } else if let Some(rel) = img_rel(&p, base, &name) {
            // R29.6: image files are vault MEMBERS but never notes. They ride
            // the note walk (no second traversal, no second symlink policy) and
            // land in their own list, keyed by vault-relative path WITH the
            // extension — which is what `resolve` matches on for `![[pic.png]]`.
            imgs.push(rel);
        } else if let Some(stem) = name.strip_suffix(".md") {
            if m.len() > MAX_NOTE_BYTES {
                warn_oversized(&p);
                continue;
            }
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

/// R29.8: the extensions v1 treats as vault images — stock's list minus `svg`
/// (scriptable, no sanitizer here), `bmp`, `avif` (scope). main.rs::IMG_TYPES
/// maps the SAME set to Content-Types and a unit test pins the two together:
/// a name the index resolves but the byte server refuses is a broken image.
pub const IMG_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// vault-relative path of `p` when its extension is an R29.8 image, else None
fn img_rel(p: &Path, base: &Path, name: &str) -> Option<String> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if !IMG_EXTS.contains(&ext.as_str()) {
        return None;
    }
    let dir = p.parent().and_then(|d| d.strip_prefix(base).ok())?;
    Some(if dir.as_os_str().is_empty() {
        name.to_string()
    } else {
        format!("{}/{name}", dir.display())
    })
}

/// sorted note names under root (the one and only vault walk)
pub fn notes_of(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, root, &mut out, &mut Vec::new());
    out.sort();
    out
}

impl Index {
    /// one walk + one read per note; links parsed once. The SAME walk yields
    /// the R29.6 image members — no second traversal, no second symlink policy.
    pub fn build(root: &Path) -> Index {
        let mut ix = Index::default();
        let (mut notes, mut imgs) = (Vec::new(), Vec::new());
        walk(root, root, &mut notes, &mut imgs);
        notes.sort();
        imgs.sort();
        ix.images = imgs;
        for n in notes {
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

    /// R31.11: an image that JUST landed in the vault becomes a member without a
    /// re-walk. Both renderers resolve `![[x.png]]` against this list
    /// (`list_images` hands it out verbatim), so a dropped file that is not in
    /// here is copied but invisible — the R29.11 gap, which a DROP must not
    /// have: the link is inserted in the same gesture. Sorted + deduped, so the
    /// list stays byte-identical to what a fresh `Index::build` would produce.
    pub fn add_image(&mut self, rel: &str) {
        if let Err(i) = self.images.binary_search(&rel.to_string()) {
            self.images.insert(i, rel.to_string());
        }
    }

    /// R29.6: sorted vault-relative image paths (with extension)
    pub fn images(&self) -> &[String] {
        &self.images
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

    /// R19: cached graph (built on first use after any edge change)
    pub fn graph(&mut self) -> &GraphCache {
        if self.graph.is_none() {
            let g = build_graph(&self.names, &self.link_lists());
            self.graph = Some(GraphCache::new(g));
        }
        self.graph.as_ref().unwrap()
    }

    /// full edge recompute from memory: O(total links x notes), no disk
    fn rebuild_backlinks(&mut self) {
        self.backlinks.clear();
        self.graph = None;
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
                self.graph = None;
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

    /// watcher: an external delete dropped this note — key + its edges go
    pub fn remove(&mut self, name: &str) {
        if self.notes.remove(name).is_some() {
            self.refresh_names();
            self.rebuild_backlinks();
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

/* R10.5 unlinked mentions: plain-text occurrences of a note's basename,
   case-insensitive, one per occurrence, skipping [[wikilinks]] and fenced
   code. Returns (0-based line, byte col, byte len) so the caller can wrap
   exactly the matched text. Lines whose lowercase form changes byte length
   (rare non-ASCII case folds) fall back to a case-sensitive scan. */
pub fn mentions_in(content: &str, base: &str) -> Vec<(u32, u32, u32)> {
    let mut out = Vec::new();
    if base.is_empty() {
        return out;
    }
    let lb = base.to_lowercase();
    let mut fence = false;
    for (ln, line) in content.lines().enumerate() {
        let t = line.trim_start_matches(' ');
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        let ll = line.to_lowercase();
        let (hay, needle): (&str, &str) = if ll.len() == line.len() { (&ll, &lb) } else { (line, base) };
        // [[..]] spans on this line: a mention inside one is already a link
        let mut spans = Vec::new();
        let mut rest = 0;
        while let Some(a) = line[rest..].find("[[") {
            let a = rest + a;
            match line[a..].find("]]") {
                Some(b) => {
                    spans.push((a, a + b + 2));
                    rest = a + b + 2;
                }
                None => break,
            }
        }
        let mut at = 0;
        while let Some(i) = hay[at..].find(needle) {
            let i = at + i;
            if !spans.iter().any(|&(a, b)| i >= a && i < b) {
                out.push((ln as u32, i as u32, needle.len() as u32));
            }
            at = i + needle.len().max(1);
        }
    }
    out
}

/// wrap the (line, col, len) slice in [[ ]] — None if the slice no longer
/// matches `base` case-insensitively (stale offsets after an edit)
pub fn link_mention(content: &str, base: &str, line: u32, col: u32, len: u32) -> Option<String> {
    let mut lines: Vec<&str> = content.split('\n').collect();
    let l = *lines.get(line as usize)?;
    let (a, b) = (col as usize, (col + len) as usize);
    let hit = l.get(a..b)?;
    if hit.to_lowercase() != base.to_lowercase() {
        return None;
    }
    let nl = format!("{}[[{}]]{}", &l[..a], hit, &l[b..]);
    lines[line as usize] = &nl;
    Some(lines.join("\n"))
}
