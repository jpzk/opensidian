// SPDX-License-Identifier: GPL-3.0-or-later
//! #59 slice 1 — Obsidian callouts in the reading render.
//!
//! A blockquote whose FIRST source line is `[!type]`, `[!type]+` or `[!type]-`
//! (optionally `[!type|metadata]`, then an optional title) renders as
//! Obsidian's own DOM, so stock theme CSS matches it unchanged:
//!
//! ```text
//! div.callout[data-callout=<type>][data-callout-metadata][data-callout-fold=+|-|""]
//!   (+ .is-collapsible when folded, + .is-collapsed for "-")
//!   > div.callout-title > div.callout-icon (svg) + div.callout-title-inner
//!                         (+ div.callout-fold when foldable)
//!   > div.callout-content            (omitted when the callout has no body)
//! ```
//!
//! The type is matched case-insensitively and lowercased into data-callout; an
//! unknown type keeps its own name there (custom CSS callouts) and takes the
//! default "note" icon, as in Obsidian. Colours and the icon NAME come from
//! CSS (--callout-color / --callout-icon, ui/style.css), never from here.
//!
//! Security: this module only ever emits its OWN fixed markup plus `esc`aped
//! attribute values. Title and body text keep flowing through the caller's
//! normal event path (raw HTML demoted to text, links scheme-checked), so a
//! callout is exactly as inert as the blockquote it replaces.
use crate::esc;

/// what the first line of a blockquote says, when it opens a callout
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// lowercased type, e.g. "note", "tldr", "my-custom"
    pub ty: String,
    /// text after `|` inside the brackets ("" when none)
    pub meta: String,
    /// "+", "-" or ""
    pub fold: &'static str,
    /// byte length of `[!type|meta]` + fold char at the start of the line
    pub len: usize,
}

/// parse `line` (the quote's first line with its `>` marker already removed)
pub fn head(line: &str) -> Option<Head> {
    let line = line.trim_start_matches([' ', '\t']);
    let s = line.strip_prefix("[!")?;
    let close = s.find(']')?;
    let inner = &s[..close];
    if inner.is_empty() || inner.contains(['[', '\n', '\r']) {
        return None;
    }
    let (ty, meta) = match inner.find('|') {
        Some(i) => (&inner[..i], &inner[i + 1..]),
        None => (inner, ""),
    };
    if ty.is_empty() || ty.trim() != ty || ty.contains(char::is_whitespace) {
        return None;
    }
    let after = &s[close + 1..];
    let fold = if after.starts_with('+') {
        "+"
    } else if after.starts_with('-') {
        "-"
    } else {
        ""
    };
    let len = 2 + close + 1 + fold.len();
    let rest = &line[len..];
    // `[!note]x` is not a callout: the marker must end the line or be followed by blank
    if !(rest.is_empty() || rest.starts_with([' ', '\t', '\r', '\n'])) {
        return None;
    }
    Some(Head { ty: ty.to_lowercase(), meta: meta.trim().to_string(), fold, len })
}

/// the first source line of a blockquote starting at byte `start` of `src`
/// (which points at its `>`), without the marker and its one optional space
pub fn first_line(src: &str, start: usize) -> &str {
    let s = src.get(start..).unwrap_or("");
    let s = &s[..s.find('\n').unwrap_or(s.len())];
    let s = s.trim_start_matches([' ', '\t']);
    let s = s.strip_prefix('>').unwrap_or(s);
    s.strip_prefix([' ', '\t']).unwrap_or(s)
}

/// Obsidian's built-in default icon per type (aliases included). Unknown -> pencil (note).
pub fn icon(ty: &str) -> &'static str {
    match ty {
        "abstract" | "summary" | "tldr" => "clipboard-list",
        "info" => "info",
        "todo" => "check-circle-2",
        "tip" | "hint" | "important" => "flame",
        "success" | "check" | "done" => "check",
        "question" | "help" | "faq" => "help-circle",
        "warning" | "caution" | "attention" => "alert-triangle",
        "failure" | "fail" | "missing" => "x",
        "danger" | "error" => "zap",
        "bug" => "bug",
        "example" => "list",
        "quote" | "cite" => "quote",
        _ => "pencil",
    }
}

/// icon bodies: lucide (ISC licence, https://lucide.dev) 24x24 stroke paths, the
/// same family ui/plugins/api-functions.js already draws. Inline, no icon package.
fn icon_paths(name: &str) -> &'static str {
    match name {
        "clipboard-list" => r#"<rect width="8" height="4" x="8" y="2" rx="1" ry="1"/><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/><path d="M12 11h4"/><path d="M12 16h4"/><path d="M8 11h.01"/><path d="M8 16h.01"/>"#,
        "info" => r#"<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>"#,
        "check-circle-2" => r#"<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>"#,
        "flame" => r#"<path d="M8.5 14.5A2.5 2.5 0 0 0 11 12c0-1.38-.5-2-1-3-1.072-2.143-.224-4.054 2-6 .5 2.5 2 4.9 4 6.5 2 1.6 3 3.5 3 5.5a7 7 0 1 1-14 0c0-1.153.433-2.294 1-3a2.5 2.5 0 0 0 2.5 2.5z"/>"#,
        "check" => r#"<path d="M20 6 9 17l-5-5"/>"#,
        "help-circle" => r#"<circle cx="12" cy="12" r="10"/><path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3"/><path d="M12 17h.01"/>"#,
        "alert-triangle" => r#"<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>"#,
        "x" => r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#,
        "zap" => r#"<path d="M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z"/>"#,
        "bug" => r#"<path d="m8 2 1.88 1.88"/><path d="M14.12 3.88 16 2"/><path d="M9 7.13v-1a3.003 3.003 0 1 1 6 0v1"/><path d="M12 20c-3.3 0-6-2.7-6-6v-3a4 4 0 0 1 4-4h4a4 4 0 0 1 4 4v3c0 3.3-2.7 6-6 6"/><path d="M12 20v-9"/><path d="M6.53 9C4.6 8.8 3 7.1 3 5"/><path d="M6 13H2"/><path d="M3 21c0-2.1 1.7-3.9 3.8-4"/><path d="M20.97 5c0 2.1-1.6 3.8-3.5 4"/><path d="M22 13h-4"/><path d="M17.2 17c2.1.1 3.8 1.9 3.8 4"/>"#,
        "list" => r#"<path d="M3 12h.01"/><path d="M3 18h.01"/><path d="M3 6h.01"/><path d="M8 12h13"/><path d="M8 18h13"/><path d="M8 6h13"/>"#,
        "quote" => r#"<path d="M16 3a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2 1 1 0 0 1 1 1v1a2 2 0 0 1-2 2 1 1 0 0 0-1 1v2a1 1 0 0 0 1 1 6 6 0 0 0 6-6V5a2 2 0 0 0-2-2z"/><path d="M5 3a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2 1 1 0 0 1 1 1v1a2 2 0 0 1-2 2 1 1 0 0 0-1 1v2a1 1 0 0 0 1 1 6 6 0 0 0 6-6V5a2 2 0 0 0-2-2z"/>"#,
        "chevron-down" => r#"<path d="m6 9 6 6 6-6"/>"#,
        _ => r#"<path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/><path d="m15 5 4 4"/>"#,
    }
}

pub fn svg(name: &'static str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\" class=\"svg-icon lucide-{name}\" data-icon=\"{name}\">{}</svg>",
        icon_paths(name)
    )
}

/// the opening markup up to (and including) the open title-inner div
pub fn open(h: &Head) -> String {
    let cls = match h.fold {
        "-" => "callout is-collapsible is-collapsed",
        "+" => "callout is-collapsible",
        _ => "callout",
    };
    format!(
        "<div data-callout-metadata=\"{}\" data-callout-fold=\"{}\" data-callout=\"{}\" class=\"{cls}\"><div class=\"callout-title\" dir=\"auto\"><div class=\"callout-icon\">{}</div><div class=\"callout-title-inner\">",
        esc(&h.meta),
        h.fold,
        esc(&h.ty),
        svg(icon(&h.ty))
    )
}

/// Obsidian's title when none was written: the type with its first letter upper-cased
pub fn default_title(ty: &str) -> String {
    let mut cs = ty.chars();
    match cs.next() {
        Some(c) => esc(&(c.to_uppercase().collect::<String>() + cs.as_str())),
        None => String::new(),
    }
}

/// closes title-inner (+ fold chevron) and the title row
pub fn close_title(h: &Head) -> String {
    let fold = match h.fold {
        "-" => format!("<div class=\"callout-fold is-collapsed\">{}</div>", svg("chevron-down")),
        "+" => format!("<div class=\"callout-fold\">{}</div>", svg("chevron-down")),
        _ => String::new(),
    };
    format!("</div>{fold}</div>")
}

pub const CONTENT_OPEN: &str = "<div class=\"callout-content\">";

/// strip the `[!type]f` marker (and the blank after it) from the header text
/// buffer. `buf` is pulldown's text, which equals the source unless the marker
/// held an escape/entity; then fall back to "everything up to the first `]`".
pub fn strip_marker(buf: &mut String, src_line: &str, h: &Head) {
    let src_line = src_line.trim_start_matches([' ', '\t']);
    let n = if buf.starts_with(&src_line[..h.len]) {
        h.len
    } else if let Some(i) = buf.find(']') {
        i + 1 + if buf[i + 1..].starts_with(['+', '-']) && !h.fold.is_empty() { 1 } else { 0 }
    } else {
        0
    };
    buf.drain(..n);
    let ws = buf.len() - buf.trim_start_matches([' ', '\t']).len();
    buf.drain(..ws);
}
