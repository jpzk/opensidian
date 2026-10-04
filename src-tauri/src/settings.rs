// SPDX-License-Identifier: GPL-3.0-or-later
/*! R30 — the settings row DATA TABLE.

feedback #19 wants Obsidian's settings structure 1:1, with every control
whose behaviour does not exist rendered consistently disabled. The invariant
that keeps that from rotting is:

    a row is ENABLED if and only if it names a config key that really exists.

So the rows are DATA, not hand-written HTML. And the data is not hand-written
Rust either: it is a black-box transcript of the settings rows,
`data/settings-layout/structure.tsv`, embedded with `include_str!` and parsed
once. One source of truth for the table we render; editing the
transcript re-renders the pane, and the tests below walk it.

Backing is declared in exactly one place, `BACKED`. Everything absent from it
is keyless, and keyless renders disabled (no tab stop, aria-disabled,
inert) — see ui/main.js. Implementing a setting later = one line here.
*/

use std::sync::OnceLock;

/// the settings row table itself (data/settings-layout/structure.tsv)
pub const STRUCTURE_TSV: &str = include_str!("../data/settings-layout/structure.tsv");
/// the settings left nav, in Obsidian's order
pub const NAV_TSV: &str = include_str!("../data/settings-layout/nav.tsv");

/// Keys that really exist in ~/.opensidian.json handling (main.rs cfg_value()).
/// `config_keys_are_all_read_or_written_by_main` pins this list to the source.
/// (themeone item 8: "palette" LEFT this list with its readers — main.rs no
/// longer has a get_palette/set_palette, so the key would fail the test below.
/// A stale key in an existing ~/.opensidian.json is round-tripped, not read.)
pub const CONFIG_KEYS: &[&str] = &["last", "list", "sidebar_w", "rside_tab", "hotkeys", "theme", "zoom"];

/// (tab, label) -> config key. A row named here is the ONLY kind that renders
/// enabled. Today: the Hotkeys tab, whose filter, scope chips and command rows
/// are live (ui/main.js CMDS + get_hotkeys/set_hotkeys, persisted under
/// "hotkeys"). Nothing else in the pane changes app behaviour yet.
const BACKED: &[(&str, &str, &str)] = &[
    ("hotkeys", "(filter field)", "hotkeys"),
    ("hotkeys", "(scope chips)", "hotkeys"),
    ("hotkeys", "(command rows)", "hotkeys"),
    // themefs R5. Obsidian's Appearance > Themes row IS the active-theme picker —
    // a dropdown showing the painting theme (AnuPpuccin for an empty cssTheme), wired to the
    // vault's own appearance.json "cssTheme" (T1/T2) — so with vault themes
    // real (src-tauri/src/themefs.rs themes_scan/load_theme, the oracle's
    // predicate), the drop-in charter puts that picker back on Obsidian's row.
    // THIS IS NOW THE ONLY THEME CONTROL IN THE PANE (themeone item 7 / C1).
    // The PALETTE axis (goal/theme-1984) used to park a SECOND live dropdown
    // one row down, on "Current community themes" — a recorded delta from
    // Obsidian. It is gone: its colours ship as theme files the row above
    // selects, so that row goes back to Obsidian's inert status line and the
    // pane publishes exactly one theme control. "Base color scheme" one row
    // above stays disabled: that is the MODE axis and its control is Ctrl+P.
    ("appearance", "Themes", "cssTheme"),
    // themefs R3. Obsidian's Appearance > CSS snippets row manages the vault's
    // .obsidian/snippets/*.css toggles, persisted in the VAULT's own
    // appearance.json "enabledCssSnippets" array (T3) — a vault file, not a
    // ~/.opensidian.json key, so it lives in VAULT_KEYS below and is really
    // read/written by src-tauri/src/themefs.rs (enabled_snippets /
    // set_snippet_enabled), pinned by `vault_keys_are_all_touched_by_themefs`.
    ("appearance", "CSS snippets", "enabledCssSnippets"),
    // goal/linebreak REQ-1. Obsidian's Editor > Display "Strict line breaks" toggle,
    // wired to the VAULT's .obsidian/app.json "strictLineBreaks" (Obsidian's file and
    // key, default off). main.rs reads it on every reading render and writes it
    // by merge (strict_line_breaks / set_strict_line_breaks); APP_KEYS below.
    ("editor", "Strict line breaks", "strictLineBreaks"),
    // goal fontwheel (docs/ctrlzoom/recon.md REQ-1): Obsidian's Appearance > Font
    // "Quick font size adjustment" toggle, the vault's appearance.json
    // "baseFontSizeAction" (themefs::quickfont / set_quickfont; absent = ON,
    // the operator exception REQ-2). Its partner "baseFontSize" is written by
    // the Ctrl+wheel gesture AND (goal fontset) by the "Font size" slider below.
    ("appearance", "Quick font size adjustment", "baseFontSizeAction"),
    // goal fontset (docs/fontset/recon.md REQ-1..21): Obsidian's three font rows open the
    // in-Settings chooser sub-page over the vault's appearance.json font keys
    // (themefs::fonts / set_font), and the Font size slider is live on the SAME
    // "baseFontSize" the Ctrl+wheel gesture writes — one source of truth (REQ-19).
    ("appearance", "Interface font", "interfaceFontFamily"),
    ("appearance", "Text font", "textFontFamily"),
    ("appearance", "Monospace font", "monospaceFontFamily"),
    ("appearance", "Font size", "baseFontSize"),
];

/// Keys that back a settings row and live in the VAULT's .obsidian/app.json,
/// read and merge-written by main.rs (app_bool_in / set_app_bool_in).
/// `app_keys_are_all_touched_by_main` pins this list to the source.
pub const APP_KEYS: &[&str] = &["strictLineBreaks"];

/// Keys that back a settings row but live in the VAULT's .obsidian/appearance.json
/// (Obsidian's file, byte-wise round-trip — src-tauri/src/themefs.rs), not in
/// ~/.opensidian.json. Same invariant as CONFIG_KEYS, different home:
/// `vault_keys_are_all_touched_by_themefs` pins this list to the source.
pub const VAULT_KEYS: &[&str] = &[
    "enabledCssSnippets",
    "cssTheme",
    "baseFontSizeAction",
    "baseFontSize",
    "interfaceFontFamily",
    "textFontFamily",
    "monospaceFontFamily",
];

/// nav entry -> tab id used in structure.tsv's first column
const OPTIONS_TABS: &[(&str, &str)] = &[
    ("General", "general"),
    ("Appearance", "appearance"),
    ("Interface", "interface"),
    ("Editor", "editor"),
    ("Files and links", "fileslinks"),
    ("Hotkeys", "hotkeys"),
    ("Keychain", "keychain"),
];

/* goal/noplugins (operator 2026-10-01: "remove the core and community plugin
   items in the settings menu. all of them also the sub section."). A
   DELIBERATE deviation from Obsidian 1.13.7. The recon transcript stays the
   untouched Obsidian oracle (nav.tsv still lists them, structure.tsv still holds
   their rows); the model simply does not carry them. Nothing is lost: every
   row on these panes was keyless (none is in BACKED) and the per-plugin panes
   had no rows at all — docs/noplugins/inventory.md has the census and the
   grep evidence. */
/// nav names dropped from the model: matches the two Options ENTRIES and the
/// "Core plugins" GROUP (heading + every entry under it)
pub const REMOVED_NAV: &[&str] = &["Core plugins", "Community plugins"];
/// structure.tsv tabs whose rows are dropped with their nav entries
pub const REMOVED_TABS: &[&str] = &["coreplugins", "community"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Control {
    Toggle,
    Dropdown,
    Slider,
    Text,
    Button,
    Buttons,
    Nav,
    Color,
    List,
    None,
}

impl Control {
    fn parse(s: &str) -> Option<Control> {
        Some(match s {
            "toggle" => Control::Toggle,
            "dropdown" => Control::Dropdown,
            "slider" => Control::Slider,
            "text" => Control::Text,
            "button" => Control::Button,
            "buttons" => Control::Buttons,
            "nav" => Control::Nav,
            "color" => Control::Color,
            "list" => Control::List,
            "none" => Control::None,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Row {
    /// tab id (structure.tsv column 1), e.g. "fileslinks"
    pub tab: &'static str,
    /// section heading inside the tab, or None for the tab's first (unheaded) card
    pub section: Option<&'static str>,
    pub label: &'static str,
    pub desc: &'static str,
    pub control: Control,
    /// what Obsidian showed as the value — rendered as dead text on disabled rows
    pub default_shown: &'static str,
    /// the recon screenshot this row was transcribed from
    pub shot: &'static str,
    /// the config key backing this row, or None
    pub key: Option<&'static str>,
    /// enabled == key.is_some(); carried explicitly so the frontend never
    /// re-derives the rule, and asserted in `enabled_iff_backed_by_a_real_key`
    pub enabled: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct NavEntry {
    pub group: &'static str,
    pub order: u32,
    pub entry: &'static str,
    /// pane id: the structure.tsv tab for Options entries, "cp-<slug>" for the
    /// per-core-plugin entries (nav entry + empty pane is in scope, the pane
    /// contents are not — see the brief, section 2 OUT)
    pub id: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Counts {
    /// nav entries = panes
    pub tabs: usize,
    pub rows: usize,
    pub enabled: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Model {
    pub nav: Vec<NavEntry>,
    pub rows: Vec<Row>,
    pub counts: Counts,
}

fn key_for(tab: &str, label: &str) -> Option<&'static str> {
    BACKED
        .iter()
        .find(|(t, l, _)| *t == tab && *l == label)
        .map(|(_, _, k)| *k)
}

fn parse_rows() -> Vec<Row> {
    let mut out = Vec::new();
    for line in STRUCTURE_TSV.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 7 {
            continue; // shape is pinned by `transcript_shape_is_exactly_seven_columns`
        }
        if REMOVED_TABS.contains(&f[0]) {
            continue; // goal/noplugins: no nav entry opens these panes any more
        }
        let control = match Control::parse(f[4]) {
            Some(c) => c,
            None => continue, // pinned by `every_control_word_is_known`
        };
        let key = key_for(f[0], f[2]);
        out.push(Row {
            tab: f[0],
            section: if f[1] == "-" { None } else { Some(f[1]) },
            label: f[2],
            desc: f[3],
            control,
            default_shown: f[5],
            shot: f[6],
            key,
            enabled: key.is_some(),
        });
    }
    out
}

fn slug(entry: &str) -> String {
    entry
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn parse_nav() -> Vec<NavEntry> {
    let mut out = Vec::new();
    for line in NAV_TSV.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 3 {
            continue;
        }
        if REMOVED_NAV.contains(&f[0]) || REMOVED_NAV.contains(&f[2]) {
            continue; // goal/noplugins: the group heading, its entries, and the two Options entries
        }
        let Ok(order) = f[1].parse::<u32>() else { continue };
        let id = match OPTIONS_TABS.iter().find(|(e, _)| *e == f[2]) {
            Some((_, tab)) if f[0] == "Options" => (*tab).to_string(),
            _ => format!("cp-{}", slug(f[2])),
        };
        out.push(NavEntry { group: f[0], order, entry: f[2], id });
    }
    out
}

pub fn rows() -> &'static [Row] {
    static ROWS: OnceLock<Vec<Row>> = OnceLock::new();
    ROWS.get_or_init(parse_rows)
}

pub fn nav() -> &'static [NavEntry] {
    static NAV: OnceLock<Vec<NavEntry>> = OnceLock::new();
    NAV.get_or_init(parse_nav)
}

pub fn model() -> Model {
    let nav: Vec<NavEntry> = nav().to_vec();
    let rows: Vec<Row> = rows().to_vec();
    let counts = Counts {
        tabs: nav.len(),
        rows: rows.len(),
        enabled: rows.iter().filter(|r| r.enabled).count(),
    };
    Model { nav, rows, counts }
}

/// the whole table, for the frontend. Cheap (parse is memoised); the pane is
/// still built lazily per tab in ui/main.js — see the 100 ms first-paint rule.
#[tauri::command]
pub fn settings_model() -> Model {
    model()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_shape_is_exactly_seven_columns() {
        let data: Vec<&str> = STRUCTURE_TSV
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .collect();
        for l in &data {
            assert_eq!(l.split('\t').count(), 7, "not 7 columns: {l}");
        }
        // the transcript itself is still Obsidian's 105 rows (the oracle is untouched);
        // the model drops exactly the plugin panes' rows and nothing else (goal/noplugins)
        assert_eq!(data.len(), 105, "the Obsidian recon transcript changed — it is the oracle, keep it");
        let dropped = data.iter().filter(|l| REMOVED_TABS.contains(&l.split('\t').next().unwrap_or(""))).count();
        assert_eq!(dropped, 34, "31 Core plugins rows + 3 Community plugins rows");
        assert_eq!(data.len() - dropped, rows().len(), "a transcript line was silently dropped");
        assert_eq!(rows().len(), 71, "row count changed — update R30 and the smoke census");
    }

    #[test]
    fn every_control_word_is_known() {
        for l in STRUCTURE_TSV.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            let w = l.split('\t').nth(4).unwrap_or("");
            assert!(Control::parse(w).is_some(), "unknown control {w:?} in: {l}");
        }
    }

    /* THE invariant of feedback #19 (brief section 1). Both directions:
       enabled => names a key that really exists; no key => renders disabled. */
    #[test]
    fn enabled_iff_backed_by_a_real_key() {
        for r in rows() {
            match r.key {
                Some(k) => {
                    assert!(
                        CONFIG_KEYS.contains(&k) || VAULT_KEYS.contains(&k) || APP_KEYS.contains(&k),
                        "row {}/{} is enabled on key {:?}, which is not a real config key",
                        r.tab,
                        r.label,
                        k
                    );
                    assert!(r.enabled, "row {}/{} has a key but is not enabled", r.tab, r.label);
                }
                None => assert!(
                    !r.enabled,
                    "row {}/{} is ENABLED with no config key behind it — either wire it or grey it",
                    r.tab,
                    r.label
                ),
            }
        }
        let enabled = rows().iter().filter(|r| r.enabled).count();
        // 6 -> 5 (themeone item 7 / C1): the palette dropdown on Appearance >
        // "Current community themes" is DELETED, so BACKED no longer names
        // that row and it renders the way every other unimplemented row does.
        // The pane publishes exactly one theme control ("Themes"). Recorded in
        // docs/goal/themeone progress.md as R30 requires.
        // 5 -> 6 (goal/linebreak REQ-1): Editor > "Strict line breaks" is backed
        // by app.json strictLineBreaks. Recorded in goal/linebreak progress.md.
        // 6 -> 7 (goal fontwheel, rebased onto linebreak): Appearance > "Quick font size adjustment" is
        // live on baseFontSizeAction (recon.md REQ-1), recorded in
        // /workspace/goal/fontwheel/progress.md as R30 requires.
        // 7 -> 11 (goal fontset): Interface / Text / Monospace font + Font size are
        // live (docs/fontset/recon.md REQ-1, REQ-16), recorded in
        // /workspace/goal/fonts/progress.md as R30 requires.
        assert_eq!(enabled, 11, "enabled-row count changed — say why in progress.md (R30)");
    }

    /// a key is "real" only if main.rs actually reads or writes it
    #[test]
    fn config_keys_are_all_read_or_written_by_main() {
        let src = include_str!("main.rs");
        for k in CONFIG_KEYS {
            assert!(
                src.contains(&format!("[\"{k}\"]")),
                "CONFIG_KEYS names {k:?} but main.rs never touches it"
            );
        }
    }

    /// same bar for the vault-file keys: "backed" means themefs.rs really
    /// reads/writes that key in .obsidian/appearance.json, not that a row
    /// borrowed a plausible name
    #[test]
    fn vault_keys_are_all_touched_by_themefs() {
        let src = include_str!("themefs.rs");
        for k in VAULT_KEYS {
            assert!(
                src.contains(&format!("\"{k}\"")),
                "VAULT_KEYS names {k:?} but themefs.rs never touches it"
            );
        }
    }

    /// the app.json keys: main.rs really reads AND writes each one
    #[test]
    fn app_keys_are_all_touched_by_main() {
        let src = include_str!("main.rs");
        for k in APP_KEYS {
            assert!(
                src.matches(&format!("\"{k}\"")).count() >= 2,
                "APP_KEYS names {k:?} but main.rs does not both read and write it"
            );
        }
    }

    #[test]
    fn every_row_tab_is_reachable_from_the_nav() {
        for r in rows() {
            assert!(
                nav().iter().any(|n| n.id == r.tab),
                "rows exist for tab {:?} but no nav entry opens it",
                r.tab
            );
        }
    }

    #[test]
    fn nav_is_obsidian_order_and_complete() {
        let opts: Vec<&NavEntry> = nav().iter().filter(|n| n.group == "Options").collect();
        // Obsidian 1.13.7 shows 9 Options entries + a Core plugins group of 10;
        // goal/noplugins keeps the first 7 Options entries in Obsidian's order
        assert_eq!(opts.len(), 7, "General..Keychain");
        assert_eq!(opts[0].entry, "General");
        assert_eq!(opts[6].entry, "Keychain");
        let mut seen: Vec<u32> = opts.iter().map(|e| e.order).collect();
        seen.sort_unstable();
        assert_eq!(seen, (1..=7u32).collect::<Vec<u32>>(), "Options order is not 1..7");
        assert_eq!(nav().len(), 7, "nav entry count changed — update the smoke census");
    }

    /// goal/noplugins: no plugin entry, no plugin group heading, no plugin pane
    /// row survives in the model, while the Obsidian oracle still carries all of them
    #[test]
    fn no_plugin_nav_entries_groups_or_rows() {
        for n in nav() {
            assert_eq!(n.group, "Options", "a second nav group {:?} survived (its heading would render)", n.group);
            assert!(!REMOVED_NAV.contains(&n.entry), "plugin nav entry {:?} survived", n.entry);
            assert!(!n.id.starts_with("cp-") && !REMOVED_TABS.contains(&n.id.as_str()), "plugin pane id {:?}", n.id);
            assert!(!n.entry.to_lowercase().contains("plugin"), "nav entry {:?} mentions plugins", n.entry);
        }
        for r in rows() {
            assert!(!REMOVED_TABS.contains(&r.tab), "row {}/{} of a removed pane survived", r.tab, r.label);
        }
        // the oracle is kept, not edited: the removal is this filter, not a recon change
        assert_eq!(NAV_TSV.lines().filter(|l| l.starts_with("Core plugins\t")).count(), 10);
        assert!(NAV_TSV.contains("Options\t9\tCommunity plugins"));
        // nothing live was lost: the 5 enabled rows all sit on kept panes
        assert!(rows().iter().filter(|r| r.enabled).all(|r| nav().iter().any(|n| n.id == r.tab)));
    }

    #[test]
    fn rows_carry_their_recon_shot() {
        for r in rows() {
            assert!(!r.shot.is_empty(), "row {}/{} has no source shot", r.tab, r.label);
            assert!(!r.label.is_empty() && !r.desc.is_empty());
        }
    }
}
