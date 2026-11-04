// SPDX-License-Identifier: GPL-3.0-or-later
/*! R30 — the settings row DATA TABLE.

feedback #19 wants Obsidian's settings structure 1:1, with every control
whose behaviour does not exist rendered consistently disabled. The invariant
that keeps that from rotting is:

```text
a row is ENABLED if and only if it names a config key that really exists.
```

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

/* goal/settings-general (operator 2026-10-19): General shows OPENSIDIAN, not the
   reference app. Another DELIBERATE deviation applied as an overlay, like the
   plugin panes above: structure.tsv stays the untouched recon oracle, the
   model rewrites, adds or drops the rows named here and nothing else.
     - the top row is Opensidian's own version (CARGO_PKG_VERSION), with no
       stock installer version in its description
     - a "Plugin API version" row right under it, read from the ONE source
       (plugin_store::COMPAT_APP_VERSION via compat_app_version!)
     - Automatic updates: shown off and inert, deactivated for now; no
       background update check exists anywhere
     - Receive early access versions and Account > Commercial license: gone
     - Help: opens https://www.opensidian.org (an ACTION row, see ACTIONS)
   The description deliberately does not name the reference app: the nob probe
   (ui/main.js nobTok) walks every model string and refuses that word. */
/// the General pane's top row: Opensidian's version where stock shows its own
pub const VERSION_LABEL: &str = concat!("Version ", env!("CARGO_PKG_VERSION"));
const VERSION_DESC: &str = "The Opensidian release you are running. Check for updates looks for a newer signed release on GitHub.";
/// the plugin API row, inserted right under the version row
pub const API_LABEL: &str = concat!("Plugin API version ", crate::compat_app_version!());
const API_DESC: &str = "Plugin API this build implements; plugins requiring a newer minAppVersion are refused.";
const AUTO_UPDATES_DESC: &str = "Automatic updates are deactivated for now. Use Check for updates to look for a new signed release.";
const HELP_DESC: &str = "Learn how to use Opensidian and get help at www.opensidian.org.";
/// the Help row's target, opened through open_external (xdg-open)
pub const HELP_URL: &str = "https://www.opensidian.org";
/// (tab, label) rows the model drops from a kept pane
pub const REMOVED_ROWS: &[(&str, &str)] = &[
    ("general", "Receive early access versions"),
    ("general", "Commercial license"),
];
/// (tab, label) -> action: a row whose control runs an app ACTION rather than
/// writing a config key. Enabled like a backed row; ui/main.js SACT must carry a
/// handler for every action named here (`actions_are_all_handled_by_the_ui`).
const ACTIONS: &[(&str, &str, &str)] = &[("general", "Help", "help"), ("general", VERSION_LABEL, "check_updates")];

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
    /// the app action this row's control runs (ACTIONS), or None
    pub action: Option<&'static str>,
    /// enabled == key.is_some() || action.is_some(); carried explicitly so the frontend never
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
        if REMOVED_ROWS.iter().any(|(t, l)| *t == f[0] && *l == f[2]) {
            continue; // goal/settings-general: early access + Commercial license
        }
        let key = key_for(f[0], f[2]);
        let action = action_for(f[0], f[2]);
        let mut row = Row {
            tab: f[0],
            section: if f[1] == "-" { None } else { Some(f[1]) },
            label: f[2],
            desc: f[3],
            control,
            default_shown: f[5],
            shot: f[6],
            key,
            action,
            enabled: key.is_some() || action.is_some(),
        };
        let version_row = f[0] == "general" && f[1] == "-" && f[2].starts_with("Version ");
        if version_row {
            row.label = VERSION_LABEL;
            row.desc = VERSION_DESC;
            // Check for updates: an ACTION row (updates.rs update_check), keyed by the NEW label
            row.action = action_for("general", VERSION_LABEL);
            row.enabled = row.key.is_some() || row.action.is_some();
        } else if f[0] == "general" && f[2] == "Automatic updates" {
            row.desc = AUTO_UPDATES_DESC;
            row.default_shown = "off"; // drawn off; keyless, so inert
        } else if f[0] == "general" && f[2] == "Help" {
            row.desc = HELP_DESC;
        }
        out.push(row);
        if version_row {
            out.push(Row {
                tab: "general",
                section: None,
                label: API_LABEL,
                desc: API_DESC,
                control: Control::None,
                default_shown: "-",
                shot: f[6],
                key: None,
                action: None,
                enabled: false,
            });
        }
    }
    out
}

fn action_for(tab: &str, label: &str) -> Option<&'static str> {
    ACTIONS
        .iter()
        .find(|(t, l, _)| *t == tab && *l == label)
        .map(|(_, _, a)| *a)
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

/* goal cvf-board (CVR-A-013/014/015/016/079/092/093, CVR-C-103): the Canvas core plugin's two VAULT
   config files, in stock's format — <vault>/.obsidian/canvas.json (the Canvas settings page and the
   board's snap toggles) and <vault>/.obsidian/core-plugins.json (the Canvas on/off switch). They work in
   a restricted vault: this is app code, not a plugin, so nothing here goes through plugin_vfs.
   Reached through the `settings_model` command's optional `vcfg` argument (the one settings command the
   app already registers): absent = the settings model as before; present = read (no patch) or merge a
   patch into one of the two files and return the file's whole object afterwards.
   Same rules as graph.json in main.rs: a MERGE into the parsed object (keys we do not own round-trip),
   an unparsable or non-object file is REFUSED rather than replaced, only our keys with their types are
   accepted, and the write goes through the vault dir fd (no-follow `.obsidian`, random temp, renameat),
   2-space pretty JSON like stock. */
pub const CANVAS_REL: &str = ".obsidian/canvas.json";
pub const CORE_REL: &str = ".obsidian/core-plugins.json";

/// one vault-config request from the frontend (ui/plugins/settings.js)
#[derive(Debug, serde::Deserialize)]
pub struct VaultCfg {
    /// "canvas" | "core-plugins"
    pub file: String,
    /// None = read; Some(object) = merge these keys
    #[serde(default)]
    pub patch: Option<serde_json::Value>,
}

fn vcfg_rel(file: &str) -> Result<&'static str, String> {
    match file {
        "canvas" => Ok(CANVAS_REL),
        "core-plugins" => Ok(CORE_REL),
        f => Err(format!("vault config {f:?} is not one of ours")),
    }
}

/// the keys each file accepts from us, each with its type
fn vcfg_key_ok(file: &str, k: &str, v: &serde_json::Value) -> Result<(), String> {
    let one_of = |set: &[&str]| v.as_str().is_some_and(|s| set.contains(&s));
    let ok = match (file, k) {
        ("canvas", "newFileLocation") => one_of(&["same", "current", "root", "folder"]),
        ("canvas", "newFileFolderPath") => v.as_str().is_some_and(|s| s.len() <= 1024 && !s.contains('\0') && !s.split('/').any(|c| c == "..")),
        ("canvas", "defaultWheelBehavior") => one_of(&["pan", "zoom"]),
        ("canvas", "defaultModDragBehavior") => one_of(&["menu", "text", "note", "media", "web", "group"]),
        ("canvas", "showCardNames") => one_of(&["always", "hover", "never"]),
        ("canvas", "zoomThreshold") => v.as_f64().is_some_and(|x| x.is_finite() && (0.0..=400.0).contains(&x)),
        ("canvas", "snapToGrid") | ("canvas", "snapToObjects") => v.is_boolean(),
        ("core-plugins", "canvas") => v.is_boolean(),
        _ => return Err(format!("{file}.json: key {k:?} is not ours to write")),
    };
    if ok { Ok(()) } else { Err(format!("{file}.json: bad value for {k:?}")) }
}

/// the whole file as an object; absent / empty = {}; unparsable / not an object = Err (never replaced)
fn vcfg_load(root: &std::path::Path, rel: &str) -> Result<serde_json::Value, String> {
    let p = root.join(rel);
    let cur = match std::fs::symlink_metadata(&p) {
        Ok(m) if m.len() > 1 << 20 => return Err(format!("{rel} is over 1 MiB — refusing it")),
        Ok(_) => std::fs::read_to_string(&p).map_err(|e| format!("{rel}: {e}"))?,
        Err(_) => String::new(),
    };
    if cur.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    let v: serde_json::Value = serde_json::from_str(&cur).map_err(|e| format!("{rel} is not JSON ({e}) — refusing to overwrite it"))?;
    if v.is_object() { Ok(v) } else { Err(format!("{rel} is not a JSON object — refusing to overwrite it")) }
}

pub fn vcfg_in(root: &std::path::Path, req: &VaultCfg) -> Result<serde_json::Value, String> {
    let rel = vcfg_rel(&req.file)?;
    let Some(patch) = &req.patch else {
        // a read never fails the caller: an unreadable file reads as {} (defaults), the write refuses it
        return Ok(vcfg_load(root, rel).unwrap_or_else(|_| serde_json::json!({})));
    };
    let p = patch.as_object().ok_or("vault config patch is not an object")?;
    for (k, val) in p {
        vcfg_key_ok(&req.file, k, val)?;
    }
    let mut v = vcfg_load(root, rel)?;
    let o = v.as_object_mut().ok_or("not an object")?;
    for (k, val) in p {
        o.insert(k.clone(), val.clone());
    }
    let body = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    crate::vaultfs::mkdir_p(root, std::path::Path::new(".obsidian")).map_err(|e| e.to_string())?;
    crate::vaultfs::write_atomic(root, std::path::Path::new(rel), body.as_bytes()).map_err(|e| e.to_string())?;
    Ok(v)
}

/// the whole table, for the frontend. Cheap (parse is memoised); the pane is
/// still built lazily per tab in ui/main.js — see the 100 ms first-paint rule.
/// With `vcfg` (goal cvf-board): one vault config read / merge instead, see VaultCfg.
#[tauri::command]
pub(crate) fn settings_model(v: tauri::State<crate::Vault>, vcfg: Option<VaultCfg>) -> Result<serde_json::Value, String> {
    match vcfg {
        None => serde_json::to_value(model()).map_err(|e| e.to_string()),
        Some(req) => {
            let root = crate::cur_vault(&v).ok_or("no vault open")?;
            vcfg_in(&root, &req)
        }
    }
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
        // and the General overlay (goal/settings-general): -2 dropped rows, +1 plugin API row
        let removed = data.iter().filter(|l| { let f: Vec<&str> = l.split('\t').collect(); REMOVED_ROWS.iter().any(|(t, r)| *t == f[0] && *r == f[2]) }).count();
        assert_eq!(removed, REMOVED_ROWS.len(), "a REMOVED_ROWS entry names no transcript row");
        assert_eq!(data.len() - dropped - removed + 1, rows().len(), "a transcript line was silently dropped");
        assert_eq!(rows().len(), 70, "row count changed — update R30 and the smoke census");
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
            if let Some(a) = r.action {
                // an ACTION row: enabled because the UI runs a real handler for it
                assert!(r.key.is_none(), "row {}/{} has both a key and an action", r.tab, r.label);
                assert!(ACTIONS.iter().any(|(_, _, x)| *x == a), "row {}/{} names unknown action {a:?}", r.tab, r.label);
                assert!(r.enabled, "action row {}/{} is not enabled", r.tab, r.label);
                continue;
            }
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
        // 11 -> 12 (goal settings-general): General > Help is an ACTION row that
        // opens https://www.opensidian.org, recorded in /workspace/goal/settings-general/progress.md.
        // 12 -> 13 (goal settings-general): the General top row's "Check for updates" is an
        // ACTION row (updates.rs: signed GitHub release check), same progress.md.
        assert_eq!(enabled, 13, "enabled-row count changed — say why in progress.md (R30)");
    }

    /// a key is "real" only if main.rs actually reads or writes it
    #[test]
    fn config_keys_are_all_read_or_written_by_main() {
        let src = include_str!("lib.rs");
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
        let src = include_str!("lib.rs");
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

    /// goal/settings-general: the General pane shows Opensidian, not the stock build
    #[test]
    fn general_shows_opensidian_version_and_plugin_api() {
        let g: Vec<&Row> = rows().iter().filter(|r| r.tab == "general").collect();
        assert_eq!(g[0].label, format!("Version {}", env!("CARGO_PKG_VERSION")), "top row is not the Opensidian version");
        assert!(!g[0].desc.contains("1.13") && !g[0].desc.contains("Installer"), "stock installer version leaked: {}", g[0].desc);
        assert_eq!((g[0].action, g[0].enabled, g[0].default_shown), (Some("check_updates"), true, "Check for updates"));
        assert_eq!(g[1].label, format!("Plugin API version {}", crate::plugin_store::COMPAT_APP_VERSION));
        assert_eq!(g[1].label, "Plugin API version 1.14.4");
        assert!(g[1].desc.contains("minAppVersion") && g[1].control == Control::None && !g[1].enabled);
        let au = g.iter().find(|r| r.label == "Automatic updates").expect("Automatic updates row");
        assert!(au.control == Control::Toggle && au.default_shown == "off" && !au.enabled, "auto-updates must be off and inert");
        assert!(au.desc.contains("deactivated"));
        for gone in ["Receive early access versions", "Commercial license"] {
            assert!(!g.iter().any(|r| r.label == gone), "{gone} row survived");
        }
        let help = g.iter().find(|r| r.label == "Help").expect("Help row");
        assert_eq!((help.action, help.enabled), (Some("help"), true));
        assert!(help.desc.contains("Opensidian"));
        for r in &g {
            assert!(!r.label.to_lowercase().contains("obsidian") || r.label.contains("Opensidian"));
            assert!(!r.desc.replace("Opensidian", "").to_lowercase().contains("obsidian"), "{} names the reference app", r.label);
        }
    }

    /// every ACTION the model names has a handler in the UI, and Help's goes to HELP_URL
    #[test]
    fn actions_are_all_handled_by_the_ui() {
        let js = include_str!("../../ui/main.js");
        for (_, _, a) in ACTIONS {
            assert!(js.contains(&format!("  {a}: ")), "ui/main.js SACT has no handler for action {a:?}");
        }
        assert_eq!(HELP_URL, "https://www.opensidian.org");
        assert!(js.contains(&format!("help: () => sOpenExt(\"{HELP_URL}\")")), "SACT.help does not open HELP_URL");
        assert!(js.contains("inv(\"update_check\")"), "SACT.check_updates does not call the update_check command");
    }

    /* goal cvf-board: canvas.json / core-plugins.json — merge keeps foreign keys, refuses what is not ours,
       never replaces an unparsable file, writes 2-space JSON */
    #[test]
    fn vault_cfg_merges_refuses_and_keeps_foreign_keys() {
        let root = std::env::temp_dir().join(format!("opensidian-vcfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let req = |file: &str, patch: Option<serde_json::Value>| VaultCfg { file: file.into(), patch };
        // absent file reads as {}
        assert_eq!(vcfg_in(&root, &req("canvas", None)).unwrap(), serde_json::json!({}));
        // a write creates .obsidian and the file, 2-space indent
        vcfg_in(&root, &req("canvas", Some(serde_json::json!({"snapToGrid": false, "snapToObjects": true})))).unwrap();
        let body = std::fs::read_to_string(root.join(CANVAS_REL)).unwrap();
        assert!(body.contains("\n  \"snapToGrid\": false"), "not 2-space JSON: {body}");
        // a foreign key written by stock survives our merge
        std::fs::write(root.join(CANVAS_REL), "{\"stockOnly\": 7, \"snapToGrid\": true}").unwrap();
        let v = vcfg_in(&root, &req("canvas", Some(serde_json::json!({"defaultWheelBehavior": "zoom"})))).unwrap();
        assert_eq!(v, serde_json::json!({"stockOnly": 7, "snapToGrid": true, "defaultWheelBehavior": "zoom"}));
        // not our key / wrong type / bad enum value: refused, file untouched
        let before = std::fs::read_to_string(root.join(CANVAS_REL)).unwrap();
        assert!(vcfg_in(&root, &req("canvas", Some(serde_json::json!({"stockOnly": 1})))).is_err());
        assert!(vcfg_in(&root, &req("canvas", Some(serde_json::json!({"snapToGrid": "no"})))).is_err());
        assert!(vcfg_in(&root, &req("canvas", Some(serde_json::json!({"showCardNames": "sometimes"})))).is_err());
        assert!(vcfg_in(&root, &req("canvas", Some(serde_json::json!({"newFileFolderPath": "../out"})))).is_err());
        assert!(vcfg_in(&root, &req("app", Some(serde_json::json!({"canvas": true})))).is_err());
        assert_eq!(std::fs::read_to_string(root.join(CANVAS_REL)).unwrap(), before);
        // core-plugins.json: only "canvas", a bool; an array-format file (old stock) is refused, not replaced
        let v = vcfg_in(&root, &req("core-plugins", Some(serde_json::json!({"canvas": false})))).unwrap();
        assert_eq!(v, serde_json::json!({"canvas": false}));
        assert!(vcfg_in(&root, &req("core-plugins", Some(serde_json::json!({"graph": false})))).is_err());
        std::fs::write(root.join(CORE_REL), "[\"canvas\"]").unwrap();
        assert!(vcfg_in(&root, &req("core-plugins", Some(serde_json::json!({"canvas": true})))).is_err());
        assert_eq!(std::fs::read_to_string(root.join(CORE_REL)).unwrap(), "[\"canvas\"]");
        assert_eq!(vcfg_in(&root, &req("core-plugins", None)).unwrap(), serde_json::json!({}));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn vcfg_canvas_location_values() {
        let s = |x: &str| serde_json::json!(x);
        for ok in ["current", "same", "root", "folder"] {
            assert!(vcfg_key_ok("canvas", "newFileLocation", &s(ok)).is_ok(), "{ok}");
        }
        assert!(vcfg_key_ok("canvas", "newFileLocation", &s("elsewhere")).is_err());
        assert!(vcfg_key_ok("canvas", "newFileFolderPath", &s("Projects/Deep")).is_ok());
        assert!(vcfg_key_ok("canvas", "newFileFolderPath", &s("../x")).is_err());
    }
}
