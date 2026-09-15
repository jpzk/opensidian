// SPDX-License-Identifier: GPL-3.0-or-later
/*! R30 — the settings row DATA TABLE.

feedback #19 wants stock Obsidian's settings structure 1:1, with every control
whose behaviour does not exist rendered consistently disabled. The invariant
that keeps that from rotting is:

    a row is ENABLED if and only if it names a config key that really exists.

So the rows are DATA, not hand-written HTML. And the data is not hand-written
Rust either: it is the black-box recon transcript
`docs/stock-settings-recon/structure.tsv` (105 rows read off screenshots of
stock 1.13.7) embedded with `include_str!` and parsed once. One source of
truth for the pixels we copied and for the table we render; editing the
transcript re-renders the pane, and the tests below walk it.

Backing is declared in exactly one place, `BACKED`. Everything absent from it
is keyless, and keyless renders disabled (no tab stop, aria-disabled,
inert) — see ui/main.js. Implementing a setting later = one line here.
*/

use std::sync::OnceLock;

/// the recon transcript — the row table itself (see docs/stock-settings-recon/)
pub const STRUCTURE_TSV: &str = include_str!("../../docs/stock-settings-recon/structure.tsv");
/// stock's left nav, in stock's order
pub const NAV_TSV: &str = include_str!("../../docs/stock-settings-recon/nav.tsv");

/// Keys that really exist in ~/.rustidian.json handling (main.rs cfg_value()).
/// `config_keys_are_all_read_or_written_by_main` pins this list to the source.
pub const CONFIG_KEYS: &[&str] = &["last", "list", "sidebar_w", "rside_tab", "hotkeys", "theme", "palette", "zoom"];

/// (tab, label) -> config key. A row named here is the ONLY kind that renders
/// enabled. Today: the Hotkeys tab, whose filter, scope chips and command rows
/// are live (ui/main.js CMDS + get_hotkeys/set_hotkeys, persisted under
/// "hotkeys"). Nothing else in the pane changes app behaviour yet.
const BACKED: &[(&str, &str, &str)] = &[
    ("hotkeys", "(filter field)", "hotkeys"),
    ("hotkeys", "(scope chips)", "hotkeys"),
    ("hotkeys", "(command rows)", "hotkeys"),
    // the PALETTE axis (goal/theme-1984). Stock's Appearance > Themes row is
    // literally "Manage installed themes" — a dropdown naming the active theme —
    // so the palette selector belongs on the row stock already put it on, not on
    // a row we invent. It is backed by "palette", which main.rs really reads and
    // writes (get_palette / set_palette); "Base color scheme" one row above
    // stays disabled because that is the MODE axis and its control is Ctrl+P.
    ("appearance", "Themes", "palette"),
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
    ("Core plugins", "coreplugins"),
    ("Community plugins", "community"),
];

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
    /// what stock showed as the value — rendered as dead text on disabled rows
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
        assert_eq!(data.len(), rows().len(), "a transcript line was silently dropped");
        assert_eq!(rows().len(), 105, "row count changed — update R30 and the smoke census");
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
                        CONFIG_KEYS.contains(&k),
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
        assert_eq!(enabled, 4, "enabled-row count changed — say why in progress.md (R30)");
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
    fn nav_is_stock_order_and_complete() {
        let opts: Vec<&NavEntry> = nav().iter().filter(|n| n.group == "Options").collect();
        assert_eq!(opts.len(), 9, "stock 1.13.7 shows 9 Options entries");
        assert_eq!(opts[0].entry, "General");
        assert_eq!(opts[8].entry, "Community plugins");
        for (g, n) in [("Options", 9usize), ("Core plugins", 10)] {
            let mut seen: Vec<u32> = nav().iter().filter(|e| e.group == g).map(|e| e.order).collect();
            seen.sort_unstable();
            assert_eq!(seen, (1..=n as u32).collect::<Vec<u32>>(), "{g} order is not 1..{n}");
        }
        assert_eq!(nav().len(), 19, "nav entry count changed — update the smoke census");
    }

    #[test]
    fn rows_carry_their_recon_shot() {
        for r in rows() {
            assert!(!r.shot.is_empty(), "row {}/{} has no source shot", r.tab, r.label);
            assert!(!r.label.is_empty() && !r.desc.is_empty());
        }
    }
}
