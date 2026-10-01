// SPDX-License-Identifier: GPL-3.0-or-later
//! rsidebar: Outline (TOC) — ATX headings from a note's source. Setext
//! headings are ignored (the stock outline lists them, but they're rare in
//! wikilink vaults and would need lookahead); lines inside fenced code are
//! skipped; inline markdown is stripped from the heading text.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    /// 0-based source line (matches lp row dataset.l0 / editor line)
    pub line: u32,
}

/// fence opener/closer: up to 3 spaces of indent then ``` or ~~~
fn fence(l: &str) -> Option<char> {
    let t = l.trim_start_matches(' ');
    if l.len() - t.len() > 3 {
        return None;
    }
    for c in ['`', '~'] {
        if t.starts_with(&c.to_string().repeat(3)) {
            return Some(c);
        }
    }
    None
}

/// strip inline markdown for display: emphasis/strike/code markers,
/// [[target|alias]] -> alias (or target), [text](url) -> text, `code` -> code
pub fn strip_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("[[") {
            if let Some(j) = r.find("]]") {
                let inner = &r[..j];
                let shown = inner.rsplit_once('|').map(|(_, a)| a).unwrap_or(inner);
                out.push_str(shown.split('#').next().unwrap_or(shown));
                rest = &r[j + 2..];
                continue;
            }
        }
        if let Some(r) = rest.strip_prefix('[') {
            if let Some(j) = r.find("](") {
                if let Some(k) = r[j + 2..].find(')') {
                    out.push_str(&strip_inline(&r[..j]));
                    rest = &r[j + 2 + k + 1..];
                    continue;
                }
            }
        }
        let c = rest.chars().next().unwrap();
        if matches!(c, '*' | '_' | '`' | '~') {
            rest = &rest[c.len_utf8()..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn parse(src: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut in_fence: Option<char> = None;
    for (i, l) in src.lines().enumerate() {
        if let Some(f) = fence(l) {
            match in_fence {
                None => in_fence = Some(f),
                Some(o) if o == f => in_fence = None,
                _ => {}
            }
            continue;
        }
        if in_fence.is_some() {
            continue;
        }
        let t = l.trim_start_matches(' ');
        if l.len() - t.len() > 3 || !t.starts_with('#') {
            continue;
        }
        let level = t.chars().take_while(|&c| c == '#').count();
        if level > 6 {
            continue;
        }
        let body = &t[level..];
        if !body.is_empty() && !body.starts_with(' ') && !body.starts_with('\t') {
            continue; // "#hashtag" is not a heading
        }
        // closing sequence: trailing #s preceded by a space (or the whole body)
        let mut body = body.trim();
        let trimmed = body.trim_end_matches('#');
        if trimmed.len() != body.len() && (trimmed.is_empty() || trimmed.ends_with(' ')) {
            body = trimmed.trim_end();
        }
        out.push(Heading { level: level as u8, text: strip_inline(body), line: i as u32 });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lv(v: &[Heading]) -> Vec<(u8, &str, u32)> {
        v.iter().map(|h| (h.level, h.text.as_str(), h.line)).collect()
    }

    #[test]
    fn nested_levels_and_lines() {
        let s = "# One\ntext\n## Two\n### Three\n#### Four\n## Two-b\n###### Six\n####### seven";
        assert_eq!(
            lv(&parse(s)),
            vec![(1, "One", 0), (2, "Two", 2), (3, "Three", 3), (4, "Four", 4), (2, "Two-b", 5), (6, "Six", 6)]
        );
    }

    #[test]
    fn fenced_code_ignored() {
        let s = "# A\n```sh\n# not a heading\n## nope\n```\n## B\n~~~\n# also not\n~~~\n### C";
        assert_eq!(lv(&parse(s)), vec![(1, "A", 0), (2, "B", 5), (3, "C", 9)]);
        // mismatched fence chars don't close each other
        let s = "```\n~~~\n# inside\n```\n# out";
        assert_eq!(lv(&parse(s)), vec![(1, "out", 4)]);
    }

    #[test]
    fn inline_markdown_stripped() {
        let s = "# **Bold** and *em* `code` ~~gone~~\n## [[Note|Alias]] + [[Plain#sec]]\n### [link](http://x) __u__ ##\n#### closing #s ###";
        assert_eq!(
            lv(&parse(s)),
            vec![(1, "Bold and em code gone", 0), (2, "Alias + Plain", 1), (3, "link u", 2), (4, "closing #s", 3)]
        );
    }

    #[test]
    fn setext_and_hashtags_ignored() {
        let s = "Title\n=====\nSub\n-----\n#tag line\n   # indented ok\n    # four spaces is code\n#\n# real";
        assert_eq!(lv(&parse(s)), vec![(1, "indented ok", 5), (1, "", 7), (1, "real", 8)]);
    }

    #[test]
    fn empty() {
        assert!(parse("").is_empty());
        assert!(parse("no headings\nhere").is_empty());
    }
}
