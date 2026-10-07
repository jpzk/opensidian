// SPDX-License-Identifier: GPL-3.0-or-later
//! graphctx: the graph Filters search + Groups query language and the filtered
//! graph view (harness docs/graphctx-requirements.md GC4-GC20, measured on
//! stock 1.13.7 in docs/recon-graphctx/README.md §2.6, §3, §5).
//!
//! Query: whitespace = AND, `OR` = or, `-x` = not, `( )` grouping, `"phrase"`,
//! `/regex/`, `op:value` / `op:(sub query)` for tag path file content line
//! section match-case ignore-case, `[prop]` / `[prop:value]` frontmatter
//! property. A plain term matches the content or the path, case-insensitive.
//! An incomplete operator (`tag:`) matches nothing.

use crate::index::{Graph, GNode};
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Field {
    Any,
    Content,
    File,
    Path,
    Tag,
}

#[derive(Debug)]
enum Pat {
    Text(String), // lower-cased unless the term is case-sensitive
    Re(Regex),
}

#[derive(Debug)]
pub enum Q {
    All,
    Never,
    And(Vec<Q>),
    Or(Vec<Q>),
    Not(Box<Q>),
    Term { f: Field, p: Pat, case: bool },
    Line(Box<Q>),
    Section(Box<Q>),
    Prop(String, Option<String>),
}

/// one document a query is tested against: a note (path without `.md`,
/// content, tags) or an attachment (path with extension, no content)
pub struct Doc<'a> {
    pub path: &'a str,
    pub content: &'a str,
    pub tags: &'a [String],
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Phrase(String),
    Re(String),
    Op(String),   // `name:` immediately followed by a value token (or nothing)
    Prop(String), // the inside of [..]
    Open,
    Close,
    Neg,
    OrKw,
}

fn lex(s: &str) -> Vec<Tok> {
    let c: Vec<char> = s.chars().collect();
    let (mut i, mut out) = (0, Vec::new());
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '(' {
            out.push(Tok::Open);
            i += 1;
        } else if ch == ')' {
            out.push(Tok::Close);
            i += 1;
        } else if ch == '-' && i + 1 < c.len() && !c[i + 1].is_whitespace() {
            out.push(Tok::Neg);
            i += 1;
        } else if ch == '"' {
            let j = (i + 1..c.len()).find(|&j| c[j] == '"').unwrap_or(c.len());
            out.push(Tok::Phrase(c[i + 1..j].iter().collect()));
            i = j + 1;
        } else if ch == '/' && (i + 1..c.len()).any(|j| c[j] == '/') {
            let j = (i + 1..c.len()).find(|&j| c[j] == '/').unwrap();
            out.push(Tok::Re(c[i + 1..j].iter().collect()));
            i = j + 1;
        } else if ch == '[' {
            let j = (i + 1..c.len()).find(|&j| c[j] == ']').unwrap_or(c.len());
            out.push(Tok::Prop(c[i + 1..j].iter().collect()));
            i = j + 1;
        } else {
            let st = i;
            while i < c.len() && !c[i].is_whitespace() && c[i] != '(' && c[i] != ')' && c[i] != '"' {
                if c[i] == ':' {
                    break;
                }
                i += 1;
            }
            let w: String = c[st..i].iter().collect();
            if i < c.len() && c[i] == ':' {
                let name = w.to_ascii_lowercase();
                if matches!(name.as_str(), "tag" | "path" | "file" | "content" | "line" | "section" | "match-case" | "ignore-case") {
                    out.push(Tok::Op(name));
                    i += 1;
                    continue;
                }
                // not an operator: the colon is part of the word
                while i < c.len() && !c[i].is_whitespace() && c[i] != '(' && c[i] != ')' {
                    i += 1;
                }
                out.push(Tok::Word(c[st..i].iter().collect()));
                continue;
            }
            if w == "OR" {
                out.push(Tok::OrKw);
            } else if !w.is_empty() {
                out.push(Tok::Word(w));
            } else {
                i += 1; // lone stray char
            }
        }
    }
    out
}

struct P {
    t: Vec<Tok>,
    i: usize,
}

impl P {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    // or := and (OR and)*
    fn or(&mut self, f: Field, case: bool) -> Q {
        let mut v = vec![self.and(f, case)];
        while self.peek() == Some(&Tok::OrKw) {
            self.i += 1;
            v.push(self.and(f, case));
        }
        if v.len() == 1 { v.pop().unwrap() } else { Q::Or(v) }
    }
    // and := unary+  (stops at ')' / OR / end)
    fn and(&mut self, f: Field, case: bool) -> Q {
        let mut v = Vec::new();
        while let Some(t) = self.peek() {
            if *t == Tok::Close || *t == Tok::OrKw {
                break;
            }
            v.push(self.unary(f, case));
        }
        match v.len() {
            0 => Q::All,
            1 => v.pop().unwrap(),
            _ => Q::And(v),
        }
    }
    fn unary(&mut self, f: Field, case: bool) -> Q {
        if self.peek() == Some(&Tok::Neg) {
            self.i += 1;
            if self.peek().is_none() {
                return Q::All;
            }
            return Q::Not(Box::new(self.unary(f, case)));
        }
        self.atom(f, case)
    }
    fn group(&mut self, f: Field, case: bool) -> Q {
        // after Open
        let q = self.or(f, case);
        if self.peek() == Some(&Tok::Close) {
            self.i += 1;
        }
        q
    }
    fn atom(&mut self, f: Field, case: bool) -> Q {
        let Some(t) = self.peek().cloned() else { return Q::All };
        self.i += 1;
        match t {
            Tok::Open => self.group(f, case),
            Tok::Close | Tok::OrKw | Tok::Neg => Q::All,
            Tok::Word(w) | Tok::Phrase(w) => text(f, &w, case),
            Tok::Re(r) => match Regex::new(&if case { r } else { format!("(?i){r}") }) {
                Ok(re) => Q::Term { f, p: Pat::Re(re), case },
                Err(_) => Q::Never,
            },
            Tok::Prop(p) => {
                let (k, v) = match p.split_once(':') {
                    Some((k, v)) => (k.trim().to_lowercase(), Some(v.trim().trim_matches('"').to_lowercase())),
                    None => (p.trim().to_lowercase(), None),
                };
                if k.is_empty() { Q::Never } else { Q::Prop(k, v) }
            }
            Tok::Op(op) => {
                // the value: a group, a phrase/word/regex — or nothing (incomplete -> matches nothing)
                let starts_value = matches!(self.peek(), Some(Tok::Open | Tok::Word(_) | Tok::Phrase(_) | Tok::Re(_)));
                if !starts_value {
                    return Q::Never;
                }
                let (nf, ncase) = match op.as_str() {
                    "tag" => (Field::Tag, case),
                    "path" => (Field::Path, case),
                    "file" => (Field::File, case),
                    "content" | "line" | "section" => (Field::Content, case),
                    "match-case" => (f, true),
                    _ => (f, false), // ignore-case
                };
                let inner = self.atom(nf, ncase);
                match op.as_str() {
                    "line" => Q::Line(Box::new(inner)),
                    "section" => Q::Section(Box::new(inner)),
                    _ => inner,
                }
            }
        }
    }
}

fn text(f: Field, w: &str, case: bool) -> Q {
    let w = if f == Field::Tag { w.trim_start_matches('#') } else { w };
    if w.is_empty() {
        return Q::Never;
    }
    Q::Term { f, p: Pat::Text(if case { w.to_string() } else { w.to_lowercase() }), case }
}

/// parse a query; empty / whitespace-only = matches everything
pub fn parse(s: &str) -> Q {
    let mut p = P { t: lex(s), i: 0 };
    let mut v = vec![p.or(Field::Any, false)];
    while p.i < p.t.len() {
        p.i += 1; // stray ')' — skip, keep ANDing the rest
        v.push(p.or(Field::Any, false));
    }
    if v.len() == 1 { v.pop().unwrap() } else { Q::And(v) }
}

pub fn is_all(q: &Q) -> bool {
    matches!(q, Q::All)
}

fn base(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |x| x.1)
}

/// frontmatter as (lower-cased key, lower-cased raw value lines)
fn props(content: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut lines = content.lines();
    if lines.next().map(|l| l.trim_end()) != Some("---") {
        return out;
    }
    for l in lines {
        let t = l.trim_end();
        if t == "---" || t == "..." {
            return out;
        }
        if let Some(item) = t.trim_start().strip_prefix("- ") {
            if let Some(last) = out.last_mut() {
                last.1.push(' ');
                last.1.push_str(&item.to_lowercase());
            }
        } else if let Some((k, v)) = t.split_once(':') {
            out.push((k.trim().to_lowercase(), v.trim().to_lowercase()));
        }
    }
    Vec::new() // unterminated block = no frontmatter
}

struct Ctx<'a> {
    d: &'a Doc<'a>,
    lc: std::cell::OnceCell<String>,
}

impl Ctx<'_> {
    fn lower(&self) -> &str {
        self.lc.get_or_init(|| self.d.content.to_lowercase())
    }
}

fn hit(p: &Pat, hay: &str, hay_lc: &str, case: bool) -> bool {
    match p {
        Pat::Text(t) => if case { hay.contains(t.as_str()) } else { hay_lc.contains(t.as_str()) },
        Pat::Re(r) => r.is_match(hay),
    }
}

fn eval(q: &Q, c: &Ctx, content: Option<&str>) -> bool {
    match q {
        Q::All => true,
        Q::Never => false,
        Q::And(v) => v.iter().all(|x| eval(x, c, content)),
        Q::Or(v) => v.iter().any(|x| eval(x, c, content)),
        Q::Not(x) => !eval(x, c, content),
        Q::Term { f, p, case } => {
            let path_hit = |s: &str| hit(p, s, &s.to_lowercase(), *case);
            let content_hit = || match content {
                Some(s) => hit(p, s, &s.to_lowercase(), *case),
                None => hit(p, c.d.content, c.lower(), *case),
            };
            match f {
                Field::Any => content_hit() || path_hit(c.d.path),
                Field::Content => content_hit(),
                Field::Path => path_hit(c.d.path),
                Field::File => path_hit(base(c.d.path)),
                Field::Tag => match p {
                    Pat::Text(t) => {
                        let t = t.to_lowercase();
                        c.d.tags.iter().any(|g| {
                            let g = g.to_lowercase();
                            g == t || g.starts_with(&format!("{t}/"))
                        })
                    }
                    Pat::Re(r) => c.d.tags.iter().any(|g| r.is_match(g)),
                },
            }
        }
        Q::Line(x) => c.d.content.lines().any(|l| eval(x, c, Some(l))),
        Q::Section(x) => {
            let mut secs: Vec<String> = vec![String::new()];
            for l in c.d.content.lines() {
                if l.starts_with('#') && l.trim_start_matches('#').starts_with(' ') {
                    secs.push(String::new());
                }
                let s = secs.last_mut().unwrap();
                s.push_str(l);
                s.push('\n');
            }
            secs.iter().any(|s| eval(x, c, Some(s)))
        }
        Q::Prop(k, v) => props(c.d.content).iter().any(|(pk, pv)| {
            pk == k && v.as_ref().map_or(true, |v| pv.contains(v.as_str()))
        }),
    }
}

pub fn matches(q: &Q, d: &Doc) -> bool {
    eval(q, &Ctx { d, lc: std::cell::OnceCell::new() }, None)
}

/// node kinds the filtered view hands the renderer (GNode.kind)
pub const K_NOTE: u8 = 0;
pub const K_GHOST: u8 = 1;
pub const K_TAG: u8 = 2;
pub const K_ATT: u8 = 3;

/// Filters + Groups options of one graph view (global graph.json / local leaf options)
#[derive(Debug, Default, Clone, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ViewOpts {
    pub search: String,
    pub show_tags: bool,
    pub show_attachments: bool,
    pub hide_unresolved: bool,
    /// global only; None = stock default true
    pub show_orphans: Option<bool>,
    /// group queries in row order (colours stay in the frontend)
    pub groups: Vec<String>,
    // local
    pub center: Option<String>,
    pub depth: Option<usize>,
    pub inc: Option<bool>,
    pub out: Option<bool>,
    pub interlinks: bool,
}

/// filtered view graph: `g` = the cached notes+ghosts graph (notes resolved,
/// ghosts unresolved), `doc(i)` = content+tags of resolved node i, `atts` = every
/// non-markdown vault file (path with extension). Order of operations measured
/// on stock (R§2.5/2.6): attachments, existing-only, search, tags, orphans.
/// Returns the graph (GNode.kind set) and per node the first matching group (-1 none).
pub fn view<'a>(
    g: &Graph,
    doc: &dyn Fn(usize) -> (&'a str, &'a [String]),
    atts: &[String],
    o: &ViewOpts,
    local: Option<&str>,
) -> (Graph, Vec<i32>) {
    // 1. node kinds: a ghost whose name is an existing attachment (exact path or basename) is that attachment
    let mut att_of: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for a in atts {
        att_of.entry(a.as_str()).or_insert(a.as_str());
        att_of.entry(base(a)).or_insert(a.as_str());
    }
    let n0 = g.nodes.len();
    let mut kind: Vec<u8> = g.nodes.iter().map(|n| if n.resolved { K_NOTE } else { K_GHOST }).collect();
    let mut name: Vec<String> = g.nodes.iter().map(|n| n.name.clone()).collect();
    let mut att_node: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut remap: Vec<usize> = (0..n0).collect();
    for i in 0..n0 {
        if kind[i] == K_GHOST {
            if let Some(a) = att_of.get(name[i].as_str()) {
                match att_node.get(*a) {
                    Some(&j) => remap[i] = j,
                    None => {
                        kind[i] = K_ATT;
                        name[i] = a.to_string();
                        att_node.insert(a.to_string(), i);
                    }
                }
            }
        }
    }
    let mut edges: Vec<(usize, usize)> = Vec::with_capacity(g.edges.len());
    {
        let mut seen = std::collections::HashSet::new();
        for &(a, b) in &g.edges {
            let (a, b) = (remap[a], remap[b]);
            if a != b && seen.insert((a, b)) {
                edges.push((a, b));
            }
        }
    }
    let mut alive: Vec<bool> = (0..n0).map(|i| remap[i] == i).collect();
    // local: BFS neighbourhood over the (attachment-merged) graph; the centre always stays
    let mut dist: Vec<usize> = vec![usize::MAX; n0];
    let mut center = usize::MAX;
    if let Some(c) = local {
        let Some(ci) = (0..n0).find(|&i| kind[i] == K_NOTE && name[i] == c) else {
            return (Graph::default(), Vec::new());
        };
        center = ci;
        let (mut outs, mut ins) = (vec![Vec::new(); n0], vec![Vec::new(); n0]);
        for &(a, b) in &edges {
            outs[a].push(b);
            ins[b].push(a);
        }
        dist[ci] = 0;
        let mut fr = vec![ci];
        for d in 1..=o.depth.unwrap_or(1).clamp(1, 5) {
            let mut nx = Vec::new();
            for &f in &fr {
                let mut visit = |j: usize| {
                    if dist[j] == usize::MAX {
                        dist[j] = d;
                        nx.push(j);
                    }
                };
                if o.out.unwrap_or(true) {
                    outs[f].iter().for_each(|&j| visit(j));
                }
                if o.inc.unwrap_or(true) {
                    ins[f].iter().for_each(|&j| visit(j));
                }
            }
            fr = nx;
        }
        for i in 0..n0 {
            alive[i] = alive[i] && dist[i] != usize::MAX;
        }
    }
    // 2. attachments: off -> gone; on (global) -> every attachment file is a node, linked or not
    if !o.show_attachments {
        for i in 0..n0 {
            if kind[i] == K_ATT {
                alive[i] = false;
            }
        }
    } else if local.is_none() {
        for a in atts {
            if !att_node.contains_key(a.as_str()) {
                name.push(a.clone());
                kind.push(K_ATT);
                alive.push(true);
                dist.push(usize::MAX);
            }
        }
    }
    // 3. existing files only
    if o.hide_unresolved {
        for i in 0..kind.len() {
            if kind[i] == K_GHOST {
                alive[i] = false;
            }
        }
    }
    let no_tags: Vec<String> = Vec::new();
    // content + tags of node i of kind k (attachments: path only)
    let docf = |k: u8, i: usize| if k == K_NOTE { doc(i) } else { ("", &no_tags[..]) };
    // 4. search selects FILE nodes (notes + attachments); a ghost stays iff a kept file links to it
    let q = parse(&o.search);
    if !is_all(&q) {
        for i in 0..kind.len() {
            if alive[i] && (kind[i] == K_NOTE || kind[i] == K_ATT) && i != center {
                let (content, tags) = docf(kind[i], i);
                alive[i] = matches(&q, &Doc { path: &name[i], content, tags });
            }
        }
        let mut keep_ghost = vec![false; kind.len()];
        for &(a, b) in &edges {
            if alive[a] && kind[a] != K_GHOST && kind[b] == K_GHOST {
                keep_ghost[b] = true;
            }
        }
        for i in 0..kind.len() {
            if kind[i] == K_GHOST && !keep_ghost[i] {
                alive[i] = false;
            }
        }
        // a kept attachment that only came in through a link from a dropped note stays (it is a file the query matched)
    }
    // links among drawn nodes; local without neighbour links keeps only links that cross BFS layers
    let mut ed: Vec<(usize, usize)> = edges
        .iter()
        .copied()
        .filter(|&(a, b)| alive[a] && alive[b])
        .filter(|&(a, b)| local.is_none() || o.interlinks || dist[a] != dist[b])
        .collect();
    // 5. tags: one node per tag of a drawn note (nested tags are their own nodes)
    if o.show_tags {
        let mut tag_ix: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let n = kind.len();
        for i in 0..n {
            if !alive[i] || kind[i] != K_NOTE {
                continue;
            }
            if local.is_some() && i != center {
                continue; // local: the centre's tags (R§5.5)
            }
            for t in doc(i).1.to_vec() {
                let j = *tag_ix.entry(t.clone()).or_insert_with(|| {
                    name.push(format!("#{t}"));
                    kind.push(K_TAG);
                    alive.push(true);
                    dist.push(usize::MAX);
                    name.len() - 1
                });
                ed.push((i, j));
            }
        }
    }
    // 6. orphans (global): nodes without an edge in the DRAWN graph
    if local.is_none() && o.show_orphans == Some(false) {
        let mut deg = vec![0u32; kind.len()];
        for &(a, b) in &ed {
            deg[a] += 1;
            deg[b] += 1;
        }
        for i in 0..kind.len() {
            if deg[i] == 0 {
                alive[i] = false;
            }
        }
    }
    // compact; local keeps discovery order with the centre first
    let mut order: Vec<usize> = (0..kind.len()).filter(|&i| alive[i]).collect();
    if local.is_some() {
        order.sort_by_key(|&i| (if i == center { 0 } else { 1 }, dist[i]));
    }
    let mut ix = vec![usize::MAX; kind.len()];
    for (k, &i) in order.iter().enumerate() {
        ix[i] = k;
    }
    let groups: Vec<Q> = o.groups.iter().map(|s| if s.trim().is_empty() { Q::Never } else { parse(s) }).collect();
    let col: Vec<i32> = order
        .iter()
        .map(|&i| {
            if groups.is_empty() || !(kind[i] == K_NOTE || kind[i] == K_ATT) {
                return -1;
            }
            let (content, tags) = docf(kind[i], i);
            let d = Doc { path: &name[i], content, tags };
            groups.iter().position(|q| matches(q, &d)).map_or(-1, |p| p as i32)
        })
        .collect();
    let out = Graph {
        nodes: order
            .iter()
            .map(|&i| GNode { name: name[i].clone(), resolved: kind[i] != K_GHOST, kind: kind[i] })
            .collect(),
        edges: ed.iter().filter(|&&(a, b)| alive[a] && alive[b]).map(|&(a, b)| (ix[a], ix[b])).collect(),
    };
    (out, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(q: &str, path: &str, content: &str, tags: &[&str]) -> bool {
        let t: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
        matches(&parse(q), &Doc { path, content, tags: &t })
    }

    #[test]
    fn graphctx_query_language() {
        // R§2.6 rows
        assert!(m("banana", "in2", "a Banana here", &[]));
        assert!(m("Banana", "in2", "a banana here", &[]));
        assert!(m("orphan one", "o1", "orphan number one", &[]));
        assert!(!m("\"orphan one\"", "o1", "orphan number one", &[]));
        assert!(m("\"orphan one\"", "o1", "the orphan one", &[]));
        assert!(m("tag:#alpha", "c", "", &["alpha"]));
        assert!(m("tag:alpha", "out2", "", &["alpha/sub"]));
        assert!(!m("tag:#alpha/sub", "c", "", &["alpha"]));
        assert!(!m("tag:#alp", "c", "", &["alpha"]));
        assert!(m("-tag:#alpha", "deep", "", &["beta"]));
        assert!(m("path:f", "f/deep", "", &[]));
        assert!(!m("path:f", "c", "", &[]));
        assert!(m("file:o", "f/out1", "", &[]) && !m("file:f", "o1", "", &[]) && m("file:c", "att/doc.pdf", "", &[]));
        assert!(m("tag:#alpha -file:o2", "c", "", &["alpha"]) && !m("tag:#alpha -file:o2", "o2", "", &["alpha"]));
        assert!(!m("tag:#alpha tag:#beta", "c", "", &["alpha"]));
        assert!(m("tag:#alpha OR tag:#beta", "deep", "", &["beta"]));
        assert!(m("line:(banana)", "x", "l1\nbanana!\n", &[]) && m("section:(banana)", "x", "banana", &[]) && m("content:banana", "x", "banana", &[]));
        assert!(!m("line:(apple banana)", "x", "apple\nbanana\n", &[]) && m("line:(apple banana)", "x", "apple banana\n", &[]));
        assert!(!m("match-case:Banana", "x", "banana", &[]) && m("match-case:banana", "x", "banana", &[]));
        assert!(m("[tags]", "in1", "---\ntags: gamma\n---\nbody", &["gamma"]));
        assert!(!m("[tags:alpha]", "in1", "---\ntags: gamma\n---\nbody", &["gamma"]));
        assert!(m("/ban.na/", "x", "banana", &[]) && !m("/ban.na/", "x", "bana", &[]));
        assert!(!m("tag:", "c", "tag:", &["alpha"]));
        assert!(!m("nosuchthing", "c", "text", &[]));
        assert!(m("", "c", "", &[]) && m("   ", "c", "", &[]));
        assert!(m("(tag:a OR tag:b) -file:z", "q", "", &["b"]));
    }

    fn fixture() -> (Graph, Vec<(String, Vec<String>)>, Vec<String>) {
        // recon fixture shape (R§2, R§5): in2->in1->c->out1->out2, deep->c, out1->in1,
        // c->ghost, c embeds pic.png, linker->ghost2 + doc.pdf, orphans o1, o2(#alpha)
        let notes = ["c", "f/deep", "f/linker", "in1", "in2", "o1", "o2", "out1", "out2"];
        let docs: Vec<(String, Vec<String>)> = vec![
            ("[[out1]] [[ghost]] ![[pic.png]]".into(), vec!["alpha".into()]),
            ("[[c]] #beta".into(), vec!["beta".into()]),
            ("[[ghost2]] [[att/doc.pdf]]".into(), vec![]),
            ("[[c]]".into(), vec!["gamma".into()]),
            ("[[in1]] banana".into(), vec![]),
            ("orphan one".into(), vec![]),
            ("#alpha".into(), vec!["alpha".into()]),
            ("[[out2]] [[in1]] #beta".into(), vec!["beta".into()]),
            ("banana #alpha/sub".into(), vec!["alpha/sub".into()]),
        ];
        let names: Vec<String> = notes.iter().map(|s| s.to_string()).collect();
        let links: Vec<Vec<String>> = docs.iter().map(|(c, _)| crate::index::links_in(c)).collect();
        let lr: Vec<&[String]> = links.iter().map(|v| v.as_slice()).collect();
        let g = crate::index::build_graph(&names, &lr);
        (g, docs, vec!["att/doc.pdf".into(), "att/lone.png".into(), "att/pic.png".into()])
    }

    fn ids(g: &Graph) -> Vec<String> {
        let mut v: Vec<String> = g.nodes.iter().map(|n| n.name.clone()).collect();
        v.sort();
        v
    }

    #[test]
    fn graphctx_view_filters_measured_counts() {
        let (g, docs, atts) = fixture();
        let d = |i: usize| (docs[i].0.as_str(), docs[i].1.as_slice());
        let v = |o: ViewOpts| view(&g, &d, &atts, &o, None).0;
        let base = v(ViewOpts::default());
        assert_eq!(base.nodes.len(), 11, "{:?}", ids(&base)); // 9 notes + ghost + ghost2
        assert_eq!(base.edges.len(), 8);
        let t = v(ViewOpts { show_tags: true, ..Default::default() });
        assert_eq!((t.nodes.len(), t.edges.len()), (15, 14));
        assert_eq!(v(ViewOpts { show_attachments: true, ..Default::default() }).nodes.len(), 14);
        assert_eq!(v(ViewOpts { hide_unresolved: true, ..Default::default() }).nodes.len(), 9);
        assert_eq!(v(ViewOpts { show_orphans: Some(false), ..Default::default() }).nodes.len(), 9);
        assert_eq!(v(ViewOpts { show_orphans: Some(false), show_tags: true, ..Default::default() }).nodes.len(), 14);
        assert_eq!(
            ids(&v(ViewOpts { show_orphans: Some(false), hide_unresolved: true, ..Default::default() })),
            ["c", "f/deep", "in1", "in2", "out1", "out2"]
        );
        assert_eq!(ids(&v(ViewOpts { search: "tag:#alpha".into(), ..Default::default() })), ["c", "ghost", "o2", "out2"]);
        assert_eq!(ids(&v(ViewOpts { search: "-tag:#alpha".into(), ..Default::default() })),
            ["f/deep", "f/linker", "ghost2", "in1", "in2", "o1", "out1"]);
        assert_eq!(ids(&v(ViewOpts { search: "tag:#beta".into(), show_tags: true, ..Default::default() })), ["#beta", "f/deep", "out1"]);
        assert_eq!(ids(&v(ViewOpts { search: "file:c".into(), show_attachments: true, ..Default::default() })),
            ["att/doc.pdf", "att/pic.png", "c", "ghost"]);
        assert_eq!(v(ViewOpts { search: "banana".into(), show_orphans: Some(false), ..Default::default() }).nodes.len(), 0);
        assert_eq!(ids(&v(ViewOpts { search: "ghost".into(), hide_unresolved: true, ..Default::default() })), ["c", "f/linker"]);
        // groups: first match wins, ghosts/tags never coloured
        let (gv, col) = view(&g, &d, &atts, &ViewOpts { groups: vec!["tag:#alpha".into(), "file:c".into(), "".into()], ..Default::default() }, None);
        for (n, c) in gv.nodes.iter().zip(&col) {
            let want = match n.name.as_str() { "c" | "o2" | "out2" => 0, _ => -1 };
            assert_eq!(*c, want, "{}", n.name);
        }
    }

    #[test]
    fn graphctx_view_local_measured_counts() {
        let (g, docs, atts) = fixture();
        let d = |i: usize| (docs[i].0.as_str(), docs[i].1.as_slice());
        let lv = |o: ViewOpts| view(&g, &d, &atts, &o, Some("c")).0;
        let l = |depth, inc, out, inter| ViewOpts { depth: Some(depth), inc: Some(inc), out: Some(out), interlinks: inter, ..Default::default() };
        let d1 = lv(l(1, true, true, false));
        assert_eq!(ids(&d1), ["c", "f/deep", "ghost", "in1", "out1"]);
        assert_eq!(d1.nodes[0].name, "c");
        assert_eq!(d1.edges.len(), 4);
        assert_eq!(lv(l(1, true, true, true)).edges.len(), 5);
        let d2 = lv(l(2, true, true, false));
        assert_eq!((d2.nodes.len(), d2.edges.len()), (7, 6));
        assert_eq!(lv(l(2, true, true, true)).edges.len(), 7);
        assert_eq!(lv(l(5, true, true, false)).nodes.len(), 7);
        assert_eq!(ids(&lv(l(1, false, true, false))), ["c", "ghost", "out1"]);
        assert_eq!(ids(&lv(l(1, true, false, false))), ["c", "f/deep", "in1"]);
        assert_eq!(lv(ViewOpts { show_tags: true, ..l(1, true, true, false) }).nodes.len(), 6);
        assert_eq!(ids(&lv(ViewOpts { show_attachments: true, ..l(1, true, true, false) })), ["att/pic.png", "c", "f/deep", "ghost", "in1", "out1"]);
        assert_eq!(lv(ViewOpts { hide_unresolved: true, ..l(1, true, true, false) }).nodes.len(), 4);
        assert_eq!(ids(&lv(ViewOpts { search: "tag:#beta".into(), ..l(1, true, true, false) })), ["c", "f/deep", "ghost", "out1"]);
        assert_eq!(ids(&lv(ViewOpts { search: "-file:out".into(), ..l(1, true, true, false) })), ["c", "f/deep", "ghost", "in1"]);
    }
}
