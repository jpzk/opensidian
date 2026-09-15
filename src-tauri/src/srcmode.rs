// SPDX-License-Identifier: GPL-3.0-or-later
//! R12 source mode: "live preview with every marker revealed". One escaped
//! span-html string per lp block; the row's textContent equals the block's
//! source byte-for-byte (markers are emitted as text inside `span.mk`), so
//! the lp click->column mapping (lpCol) is exact. No raw html ever reaches
//! the output: every source char goes through `esc`.
use crate::index::tag_spans;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn mk(s: &str) -> String {
    format!("<span class=\"mk\">{}</span>", esc(s))
}

/// one lp block (a single line, or a whole fenced code block) -> span html
pub fn highlight_block(text: &str) -> String {
    if text.starts_with("```") || text.starts_with("~~~") {
        // fence: every line visible, fence lines grey, inside the shaded block
        let lines: Vec<&str> = text.split('\n').collect();
        let n = lines.len();
        let body: Vec<String> = lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let is_fence = i == 0 || (i == n - 1 && n > 1 && (l.starts_with("```") || l.starts_with("~~~")));
                if is_fence { mk(l) } else { esc(l) }
            })
            .collect();
        return format!("<span class=\"fence\">{}</span>", body.join("\n"));
    }
    highlight_line(text)
}

/// block-level prefix (heading / quote / hr / list / task / table), then inline
fn highlight_line(s: &str) -> String {
    let t = s.trim_start();
    let ind = &s[..s.len() - t.len()];
    // heading: '#'.. + space; size class from the level
    let hashes = t.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') && ind.is_empty() {
        return format!(
            "<span class=\"h h{hashes}\">{} {}</span>",
            mk(&t[..hashes]),
            inline(&t[hashes + 1..])
        );
    }
    // hr: --- / *** / ___ (3+), nothing else on the line
    let tt = t.trim_end();
    if tt.len() >= 3 && (tt.bytes().all(|b| b == b'-') || tt.bytes().all(|b| b == b'*') || tt.bytes().all(|b| b == b'_')) {
        return format!("<span class=\"hr\">{}</span>", mk(s));
    }
    // blockquote: '>' (optionally followed by one space) per line
    if let Some(rest) = t.strip_prefix('>') {
        let (sp, rest) = if let Some(r) = rest.strip_prefix(' ') { (" ", r) } else { ("", rest) };
        return format!("<span class=\"bq\">{}{}{}{}</span>", esc(ind), mk(">"), sp, inline(rest));
    }
    // table row: raw pipes, monospace
    if t.starts_with('|') {
        return format!("<span class=\"tbl\">{}</span>", esc(s));
    }
    // list item: -/*/+ or N. then space, then optional task box
    let marker_len = if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") {
        1
    } else {
        let d = t.bytes().take_while(|b| b.is_ascii_digit()).count();
        if d > 0 && t[d..].starts_with(". ") { d + 1 } else { 0 }
    };
    if marker_len > 0 {
        let rest = &t[marker_len + 1..];
        let task = rest.len() >= 4 && rest.starts_with('[') && rest.as_bytes()[2] == b']' && rest.as_bytes()[3] == b' '
            && matches!(rest.as_bytes()[1], b' ' | b'x' | b'X');
        let (task_html, rest) = if task {
            (format!("<span class=\"task\">{}</span> ", esc(&rest[..3])), &rest[4..])
        } else {
            (String::new(), rest)
        };
        return format!("{}{} {}{}", esc(ind), mk(&t[..marker_len]), task_html, inline(rest));
    }
    format!("{}{}", esc(ind), inline(t))
}

/// find `close` after `from` (byte index into s), never matching at `from`
fn find_close(s: &str, from: usize, close: &str) -> Option<usize> {
    if from >= s.len() { return None; }
    s[from..].find(close).map(|k| from + k)
}

/// emphasis-style wrapper: mk(open) span(inner) mk(close)
fn wrap(class: &str, open: &str, inner: &str, close: &str) -> String {
    format!("{}<span class=\"{class}\">{}</span>{}", mk(open), inline(inner), mk(close))
}

/// inline markers: code span, wikilink/embed, md link, bare url, #tag,
/// ***/**/*/_/~~/== emphasis. Unmatched markers fall through as plain text.
fn inline(s: &str) -> String {
    let tags = tag_spans(s);
    let mut out = String::new();
    let mut plain = String::new(); // pending plain text (flushed escaped)
    let b = s.as_bytes();
    let mut i = 0;
    let flush = |plain: &mut String, out: &mut String| {
        if !plain.is_empty() { out.push_str(&esc(plain)); plain.clear(); }
    };
    while i < s.len() {
        if !s.is_char_boundary(i) { i += 1; continue; }
        // #tag (index.rs rules: whitespace-preceded, not a heading/url fragment)
        if let Some(&(a, e)) = tags.iter().find(|&&(a, _)| a == i) {
            flush(&mut plain, &mut out);
            out.push_str(&format!("<span class=\"tag\">{}</span>", esc(&s[a..e])));
            i = e;
            continue;
        }
        let rest = &s[i..];
        // code span: no inline parsing inside
        if b[i] == b'`' {
            if let Some(j) = find_close(s, i + 1, "`") {
                flush(&mut plain, &mut out);
                out.push_str(&format!("<span class=\"code\">{}{}{}</span>", mk("`"), esc(&s[i + 1..j]), mk("`")));
                i = j + 1;
                continue;
            }
        }
        // ![[embed]] / [[wikilink]] : whole raw target in accent
        if rest.starts_with("![[") || rest.starts_with("[[") {
            let bang = if rest.starts_with('!') { 1 } else { 0 };
            if let Some(j) = find_close(s, i + bang + 2, "]]") {
                flush(&mut plain, &mut out);
                out.push_str(&format!(
                    "{}<span class=\"wl\">{}</span>{}",
                    mk(&s[i..i + bang + 2]),
                    esc(&s[i + bang + 2..j]),
                    mk("]]")
                ));
                i = j + 2;
                continue;
            }
        }
        // [text](url)
        if b[i] == b'[' {
            if let Some(j) = find_close(s, i + 1, "](") {
                if let Some(k) = find_close(s, j + 2, ")") {
                    if !s[i + 1..j].contains('[') {
                        flush(&mut plain, &mut out);
                        out.push_str(&format!(
                            "{}<span class=\"lt\">{}</span>{}<span class=\"url\">{}</span>{}",
                            mk("["), esc(&s[i + 1..j]), mk("]("), esc(&s[j + 2..k]), mk(")")
                        ));
                        i = k + 1;
                        continue;
                    }
                }
            }
        }
        // bare url
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let n: usize = rest.chars().take_while(|c| !c.is_whitespace()).map(char::len_utf8).sum();
            flush(&mut plain, &mut out);
            out.push_str(&format!("<span class=\"url\">{}</span>", esc(&rest[..n])));
            i += n;
            continue;
        }
        // emphasis family: opener must be followed by a non-space char
        let mut done = false;
        for (open, class) in [("***", "bi"), ("**", "b"), ("~~", "s"), ("==", "hl"), ("*", "i"), ("_", "i")] {
            if rest.starts_with(open) {
                if open == "_" && i > 0 && !s[..i].ends_with(char::is_whitespace) { break; } // intraword _ is plain
                let a = i + open.len();
                let ok_open = s[a..].chars().next().map_or(false, |c| !c.is_whitespace());
                if ok_open {
                    if let Some(j) = find_close(s, a, open) {
                        if j > a && !s[..j].ends_with(char::is_whitespace) {
                            flush(&mut plain, &mut out);
                            out.push_str(&wrap(class, open, &s[a..j], open));
                            i = j + open.len();
                            done = true;
                        }
                    }
                }
                break; // longest matching opener decides; no fallback to a shorter one
            }
        }
        if done { continue; }
        let c = rest.chars().next().unwrap();
        plain.push(c);
        i += c.len_utf8();
    }
    flush(&mut plain, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// strip tags -> must equal the source (caret mapping relies on it)
    fn text_of(html: &str) -> String {
        let mut out = String::new();
        let mut in_tag = false;
        for c in html.chars() {
            match c {
                '<' => in_tag = true,
                '>' if in_tag => in_tag = false,
                _ if !in_tag => out.push(c),
                _ => {}
            }
        }
        out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&amp;", "&")
    }

    #[test]
    fn heading_keeps_marker_and_size_class() {
        let h = highlight_block("## Two words");
        assert!(h.starts_with("<span class=\"h h2\"><span class=\"mk\">##</span> "), "{h}");
        assert_eq!(text_of(&h), "## Two words");
        assert!(!highlight_block("#nospace").contains("class=\"h "));
    }

    #[test]
    fn inline_markers_visible_text_styled() {
        let h = highlight_block("Some **bold** and *it* and ***both*** ~~s~~ ==hl== `co de`");
        for c in ["\"b\"", "\"i\"", "\"bi\"", "\"s\"", "\"hl\"", "\"code\""] {
            assert!(h.contains(&format!("class={c}")), "{c} missing in {h}");
        }
        assert_eq!(text_of(&h), "Some **bold** and *it* and ***both*** ~~s~~ ==hl== `co de`");
        // unmatched / space-led markers stay plain
        assert!(!highlight_block("a * b * c").contains("class=\"i\""));
        assert!(!highlight_block("2 ** 3").contains("class=\"b\""));
    }

    #[test]
    fn links_full_raw_target() {
        let h = highlight_block("A [[Second Note]] and [[T#Alpha|al]] and [ext](https://e.com) bare https://x.org end ![[Emb]]");
        assert!(h.contains("<span class=\"wl\">Second Note</span>"));
        assert!(h.contains("<span class=\"wl\">T#Alpha|al</span>"));
        assert!(h.contains("<span class=\"lt\">ext</span>"));
        assert!(h.contains("<span class=\"url\">https://e.com</span>"));
        assert!(h.contains("<span class=\"url\">https://x.org</span>"));
        assert!(h.contains("<span class=\"mk\">![[</span><span class=\"wl\">Emb</span>"));
        assert_eq!(text_of(&h), "A [[Second Note]] and [[T#Alpha|al]] and [ext](https://e.com) bare https://x.org end ![[Emb]]");
    }

    #[test]
    fn lists_tasks_quote_hr_table_tag() {
        assert!(highlight_block("- item").starts_with("<span class=\"mk\">-</span> item"));
        assert!(highlight_block("  - nested").starts_with("  <span class=\"mk\">-</span> nested"));
        assert!(highlight_block("2. two").starts_with("<span class=\"mk\">2.</span> two"));
        let t = highlight_block("- [x] done");
        assert!(t.contains("<span class=\"task\">[x]</span> done"), "{t}");
        assert!(highlight_block("> quoted").starts_with("<span class=\"bq\"><span class=\"mk\">&gt;</span> quoted"));
        assert!(highlight_block("---").starts_with("<span class=\"hr\">"));
        assert!(highlight_block("| a | b |").starts_with("<span class=\"tbl\">| a | b |"));
        assert!(highlight_block("see #tag here").contains("<span class=\"tag\">#tag</span>"));
        assert!(!highlight_block("# Heading").contains("class=\"tag\""));
    }

    #[test]
    fn fence_block_lines_visible() {
        let h = highlight_block("```rust\nfn main() {}\n```");
        assert!(h.starts_with("<span class=\"fence\"><span class=\"mk\">```rust</span>\n"));
        assert!(h.ends_with("\n<span class=\"mk\">```</span></span>"));
        assert_eq!(text_of(&h), "```rust\nfn main() {}\n```");
        // unterminated fence (last block of the note): only the first line is a marker
        let u = highlight_block("```\ncode");
        assert!(u.ends_with("\ncode</span>"), "{u}");
    }

    #[test]
    fn raw_html_is_escaped_never_emitted() {
        for src in ["<img src=x onerror=1>", "# <b>h</b>", "- <script>x</script>", "[[<i>]]", "`<x>`", "```\n<img src=x onerror=1>\n```"] {
            let h = highlight_block(src);
            assert!(!h.contains("<img") && !h.contains("<b>") && !h.contains("<script") && !h.contains("<i>") && !h.contains("<x>"), "{h}");
            assert!(h.contains("&lt;"), "{h}");
            assert_eq!(text_of(&h), src);
        }
    }

    #[test]
    fn text_roundtrip_unicode_and_blank() {
        for src in ["", "   ", "héllo **wörld** [[ünï]]", "emoji 🎉 *x*", "a_b_c"] {
            assert_eq!(text_of(&highlight_block(src)), src, "{src}");
        }
    }
}
