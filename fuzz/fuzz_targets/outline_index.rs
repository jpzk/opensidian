// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
/* outline_index — note-content parsers: outline (ATX headings, fences, inline
   strip) and index (wikilinks, tags + frontmatter, link parts, unlinked
   mentions, rename rewrite, in-memory index + graph).
   Attacker reach: every .md file in the vault (shared/synced/cloned vaults are
   untrusted). Input = name \0 content (name is the note key the index uses).
   Oracles: no panic; every returned span/offset is in bounds and on a UTF-8
   boundary; heading lines exist; a rewrite with old == new is the identity. */
#![no_main]
use libfuzzer_sys::fuzz_target;
use opensidian_lib::{index, outline};

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    let (name, content) = s.split_once('\0').unwrap_or(("fz", s));
    let lines = content.split('\n').count() as u32;

    for h in outline::parse(content) {
        assert!((1..=6).contains(&h.level), "level {}", h.level);
        assert!(h.line < lines, "heading line {} of {lines}", h.line);
    }
    let _ = outline::strip_inline(content);

    for (a, b) in index::tag_spans(content) {
        assert!(a <= b && b <= content.len(), "tag span {a}..{b} of {}", content.len());
        assert!(content.is_char_boundary(a) && content.is_char_boundary(b), "tag span {a}..{b} not on char boundary");
    }
    let meta = index::parse(content.to_string());
    for l in &meta.links {
        let _ = index::link_parts(l);
    }

    let base = name.rsplit('/').next().unwrap_or(name);
    for (line, col, len) in index::mentions_in(content, base) {
        assert!(line < lines, "mention line {line} of {lines}");
        let l = content.split('\n').nth(line as usize).unwrap();
        assert!(l.get(col as usize..col as usize + len as usize).is_some(), "mention {col}+{len} not a slice of line {line}");
        assert!(index::link_mention(content, base, line, col, len).is_some(), "fresh mention offsets rejected");
    }

    if !name.is_empty() {
        let (same, n) = index::rewrite_links_n(content, name, name, true, true);
        assert!(same == content, "{n} hits: rewrite {name:?}->{name:?} changed the text");
        let _ = index::rewrite_links(content, name, "renamed/target", true);
    }

    let mut ix = index::Index::default();
    ix.upsert(name, content);
    ix.upsert("other", "[[fz]] #tag\n");
    for l in meta.links.iter().take(16) {
        let _ = index::resolve(ix.names(), l);
    }
    let _ = ix.backlinks(name);
    let _ = ix.tag_counts();
    let _ = ix.graph();
});
