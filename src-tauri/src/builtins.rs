// SPDX-License-Identifier: GPL-3.0-or-later
/*! THE BUILT-IN THEMES, AS FILES — not as a table in this binary.

The operator's request (notes/brief-themeone.txt, verbatim): "we want to keep
the themes from the file system ... whenever there are no themes in the
expected obsidian theme folder we would write them out and use them from
there. no hardcoded themes in the source code."

So the colours live in `src-tauri/themes/<Name>/{manifest.json,theme.css}` —
ordinary theme directories in stock's own layout — and this module does the
one thing a binary still has to do: carry those bytes so the app can WRITE
them out on a vault that has no themes (ledger item 4). `include_str!` embeds
the asset; it does not re-declare it. The distinction is the whole point and
it is checkable: the assets are OUTSIDE `src-tauri/src` and `ui/`, so item 9's
`scripts/lint-themes.sh` can grep both trees for palette colour literals and
palette id tables and find none.

WHY THERE IS NO "Default" ASSET. `default` is the ABSENCE of a theme — stock's
`cssTheme: ""` — and its colours are `ui/style.css`'s own `:root` fallbacks,
which item 5 keeps as the defined failure direction R4 demands. A `Default`
theme file would be a SECOND definition of the default that could drift from
the first; `palette.rs` said so before this goal existed and inverting the
dependency does not change it.

PROVENANCE travels WITH the values: each `theme.css` header carries the
attribution owed for its colours and points at the doc that holds the full
table (`docs/palette-slate/SOURCE.md`, `docs/palette-wasp/SOURCE.md`, the
iTerm-2 "1984" scheme by covertbert). Generated once, from the per-palette
blocks that used to sit in `ui/style.css`, by `scripts/gen-builtin-themes.sh`
— which refuses to run once those blocks are gone (item 5), because from then
on the assets ARE the source.
*/

// `is_builtin` has no non-test caller until the settings pane marks its rows
// (item 7). An allow beats inventing a use for the compiler's benefit — and it
// is scoped to this file.
#![allow(dead_code)]

use std::fs;
use std::path::Path;
use std::sync::Mutex;

/// One shipped theme: the bytes of a real theme directory. `name` is BOTH the
/// directory name and the manifest's `name`, because `themefs::themes_scan`
/// (the oracle's predicate, measured from stock in recon T1/T7) lists a theme
/// only when those two agree.
pub struct BuiltinTheme {
    pub name: &'static str,
    pub manifest: &'static str,
    pub css: &'static str,
}

macro_rules! builtin {
    ($name:literal, $dir:literal) => {
        BuiltinTheme {
            name: $name,
            manifest: include_str!(concat!("../themes/", $dir, "/manifest.json")),
            css: include_str!(concat!("../themes/", $dir, "/theme.css")),
        }
    };
}

/// The themes this binary can seed. Order is the order they are written; the
/// scan sorts what it lists, so it is not user-visible.
///
/// THE ONE PLACE A THEME NAME MAY APPEAR IN CODE. The markers below are read by
/// `scripts/lint-themes.sh` (ledger item 9 / criterion 2), which refuses a theme
/// name anywhere else in `src-tauri/src` or `ui/`. The rows are allowed here —
/// and ONLY here — because each one names a FILE: `builtin!` expands to
/// `include_str!("../themes/<dir>/…")`, so the name is a path, not a value. The
/// lint proves that per row: the asset directory must exist and its manifest's
/// `name` must equal the row's name. The moment a row carried colours instead
/// of a path it would be the palette table again, under a new name.
// THEME ASSETS BEGIN
pub const BUILTIN_THEMES: &[BuiltinTheme] = &[
    builtin!("1984", "1984"),
    builtin!("Slate", "Slate"),
    builtin!("Wasp", "Wasp"),
];
// THEME ASSETS END

/// Is `name` one of ours? (For the settings pane, to mark a row; it is NOT a
/// privileged load path — after seeding a built-in is an ordinary file on disk
/// with no second code path. The seeding step below does not consult it: it
/// iterates the assets it ships, which is the same list from the other side.)
pub fn is_builtin(name: &str) -> bool {
    BUILTIN_THEMES.iter().any(|t| t.name == name)
}

/* ============================ R3 — SEEDING ================================
The one thing in this app that writes into `.obsidian/themes/`. The scan
itself still creates NOTHING (T1/T3 RESULT 1: stock creates neither `themes/`
nor `snippets/`), so seeding is a separate, explicit boot step and it is named
as one, on stderr, every time it runs.

THE PREDICATE IS PER *FILE*, and that is the whole contract:

  write `<vault>/.obsidian/themes/<Name>/<file>` if and only if that path does
  not exist.

Read against C3's three legs, which is why it is a file and not a flag:

- *no `.obsidian/themes/`* -> every path is missing -> all three are written,
  and from that moment they are ordinary theme directories: scanned, listed,
  chosen, hot-reloaded and deleted through the same code a third-party theme
  takes. There is no second load path (DESIGN §4).
- *second boot* -> every path exists -> nothing is opened for writing, so
  bytes AND mtime are unchanged. `create_dir_all` is not even called: it
  would be a no-op on the bytes but this way the directory's own mtime is
  untouchable too.
- *one deleted* -> exactly that one comes back. A global "have I seeded this
  vault" flag passes the first two legs and FAILS this one, which is why the
  state lives in the filesystem and nowhere else.

A USER EDIT SURVIVES, because an existing file is never opened. The cost of
that rule, stated plainly: a built-in the user deliberately deleted returns on
the next boot — the same thing stock does with a vault's config files, and the
direction that cannot lose bytes. An edit that makes the theme unlistable is
NOT repaired either; `themes_scan` already reports it with a reason (R6), and
guessing that a broken manifest was not meant would mean overwriting it.

`exists()` follows symlinks on purpose: a built-in the user has symlinked
somewhere else is present, so it is kept, not clobbered.
========================================================================== */

/// What one seeding pass did, per theme. Serialized to the UI for the
/// `[bseed:w<n>k<n>f<n>]` census token, so a gate phase can read the boot's
/// decision off the window title instead of trusting a log line.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct SeedReport {
    /// themes that had at least one missing file, now written
    pub wrote: Vec<String>,
    /// themes already complete on disk — untouched, bytes and mtime
    pub kept: Vec<String>,
    /// `<name>: <io error>` — a vault we could not write to (read-only mount,
    /// a symlinked dir outside the landlock roots). Loud, never silent.
    pub failed: Vec<String>,
}

impl SeedReport {
    const fn empty() -> Self {
        SeedReport { wrote: Vec::new(), kept: Vec::new(), failed: Vec::new() }
    }
    /// the stderr line, and the shape the census token carries
    pub fn line(&self) -> String {
        format!(
            "[seed] themes: wrote {:?} kept {:?} failed {:?}",
            self.wrote, self.kept, self.failed
        )
    }
}

/// The last pass, for the census. One boot or one vault switch = one pass, so
/// this is the CURRENT root's report; a switch replaces it rather than
/// accumulating (the old vault's decision is not a fact about the new one).
static LAST_SEED: Mutex<SeedReport> = Mutex::new(SeedReport::empty());

/// Write the built-ins that are not already on disk. Returns what it did;
/// errors are collected, never propagated — a vault we cannot seed still
/// opens, and the scan then simply lists whatever IS there.
pub fn seed_builtin_themes(root: &Path) -> SeedReport {
    let td = root.join(".obsidian").join("themes");
    let mut rep = SeedReport::default();
    for t in BUILTIN_THEMES {
        let d = td.join(t.name);
        let files: [(&str, &str); 2] = [("manifest.json", t.manifest), ("theme.css", t.css)];
        let missing: Vec<&(&str, &str)> = files.iter().filter(|(f, _)| !d.join(f).exists()).collect();
        if missing.is_empty() {
            rep.kept.push(t.name.to_string());
            continue;
        }
        if let Err(e) = fs::create_dir_all(&d) {
            rep.failed.push(format!("{}: {e}", t.name));
            continue;
        }
        let mut err = None;
        for (f, bytes) in missing {
            if let Err(e) = fs::write(d.join(f), bytes) {
                err = Some(format!("{}/{f}: {e}", t.name));
                break;
            }
        }
        match err {
            Some(e) => rep.failed.push(e),
            None => rep.wrote.push(t.name.to_string()),
        }
    }
    rep
}

/// Seed `root` and record the pass for the census + the console. Called from
/// exactly two places (main.rs): the boot that opens the persisted/`VAULT_DIR`
/// vault, and `open_vault` (the vault switch and the freshly created vault).
pub fn seed_and_record(root: &Path) {
    let rep = seed_builtin_themes(root);
    eprintln!("{}", rep.line());
    if let Ok(mut g) = LAST_SEED.lock() {
        *g = rep;
    }
}

/// The recorded pass (empty before the first one: no vault open).
pub fn last_seed() -> SeedReport {
    LAST_SEED.lock().map(|g| (*g).clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themefs::{sanitize_css, themes_scan};
    use std::fs;
    use std::path::{Path, PathBuf};


    fn tmp_vault(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("opensidian-builtins-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        root
    }

    /// write every shipped asset into `root`'s theme directory — through the
    /// PRODUCTION path (item 4), so the listing test below is an assertion
    /// about what the app actually puts on a user's disk, not about a copy of
    /// it written by the test.
    fn write_assets(root: &Path) {
        let rep = seed_builtin_themes(root);
        assert!(rep.failed.is_empty(), "seeding a tmp vault failed: {:?}", rep.failed);
    }

    /// THE criterion of ledger item 3 (C3's first leg): our OWN listing
    /// predicate — the one measured off stock — accepts every asset we ship.
    /// A built-in that the scan would exclude is a built-in the picker never
    /// shows, and the failure would only surface on a user's disk.
    #[test]
    fn builtin_assets_are_listed_by_themefs_own_predicate() {
        let root = tmp_vault("listed");
        write_assets(&root);
        let s = themes_scan(&root);
        let mut want: Vec<&str> = BUILTIN_THEMES.iter().map(|t| t.name).collect();
        want.sort();
        assert_eq!(s.listed, want, "every shipped asset must be LISTED");
        assert!(s.excluded.is_empty(), "no asset may be excluded: {:?}", s.excluded);
        assert_eq!(BUILTIN_THEMES.len(), 3, "non-vacuity: three built-ins ship");
    }

    /// The predicate's two halves, asserted directly so a failure says WHICH:
    /// the manifest parses as an object, and its `name` is the directory name.
    #[test]
    fn builtin_manifests_carry_stocks_shape_and_name_their_directory() {
        for t in BUILTIN_THEMES {
            let v: serde_json::Value = serde_json::from_str(t.manifest)
                .unwrap_or_else(|e| panic!("{}: manifest.json does not parse: {e}", t.name));
            let o = v.as_object().unwrap_or_else(|| panic!("{}: manifest is not an object", t.name));
            assert_eq!(
                o.get("name").and_then(|n| n.as_str()),
                Some(t.name),
                "{}: manifest name must equal the directory name",
                t.name
            );
            // stock's own theme manifests carry these (docs/fixtures/themefs/
            // vault-minimal/.obsidian/themes/Minimal/manifest.json)
            for k in ["version", "minAppVersion", "author"] {
                assert!(o.contains_key(k), "{}: manifest has no {k:?}", t.name);
            }
        }
    }

    /// A built-in takes the SAME path a third-party theme takes, so it has to
    /// survive the same gate: our sanitizer. `stripped == 0` is the sharper
    /// half — an asset that needed stripping would be shipping a declaration
    /// the loader refuses to hand to the WebView.
    #[test]
    fn builtin_css_passes_our_own_sanitizer_with_nothing_stripped() {
        for t in BUILTIN_THEMES {
            let s = sanitize_css(t.css)
                .unwrap_or_else(|e| panic!("{}: our own asset is refused: {e}", t.name));
            assert_eq!(s.stripped, 0, "{}: an asset must need no stripping", t.name);
        }
    }

    /// WHERE the theme declares. Recon T6 RESULT 3 measured stock's mode switch
    /// as a CLASS on <body>; a `:root` declaration cannot be seen by a read off
    /// <body>, which is cause (1) of the unthemed graph (DESIGN §3). Our own
    /// assets must therefore declare where a real theme declares.
    ///
    /// The assertion is on the SELECTORS (the lines that open a block), not on
    /// the text: the copied provenance comments mention `:root` because that is
    /// where these values used to live, and a substring search would call a
    /// comment a selector.
    #[test]
    fn builtin_css_declares_on_body_not_on_root() {
        for t in BUILTIN_THEMES {
            let sels: Vec<&str> = t
                .css
                .lines()
                .map(|l| l.trim())
                .filter(|l| l.ends_with('{') && !l.starts_with("/*") && !l.starts_with('*'))
                .collect();
            assert_eq!(
                sels,
                vec!["body.theme-dark {", "body.theme-light {"],
                "{}: an asset declares on body, and on nothing else",
                t.name
            );
        }
    }

    // ---- what survives the migration: the assets ARE the source now ----

    /* THREE TESTS DIED HERE, IN THE COMMIT THAT EARNED THEIR DEATH (item 5).
       They all compared an asset against `ui/style.css`'s per-palette blocks,
       and item 5 deletes those blocks — the assets are the definition now, so
       every one of them would have passed VACUOUSLY on an empty left-hand
       side, which is the failure mode this project keeps re-learning:

         * `every_palette_colour_is_carried_by_exactly_one_asset` — the item-3
           migration check. Its non-vacuity floor (>= 100 values) would have
           FIRED rather than lied, which is exactly why it was written that
           way, and firing is its instruction to be deleted with the blocks.
         * `an_asset_invents_no_colour_of_its_own` — "every asset literal is a
           literal style.css declares". After the blocks go, style.css declares
           the DEFAULT's colours and nothing else, so the property is false by
           construction and no weaker form of it is true: a built-in theme's
           colours live in the built-in theme, which is the point of the goal.
         * `every_palette_but_the_default_ships_as_an_asset` — pinned the asset
           set to `palette::PALETTES`, and item 8 deletes that table with the
           module.

       What was REAL in the first of them survives below: the cross-asset half
       of "exactly one" does not depend on style.css at all. Two built-ins
       sharing a literal is what an extraction that crossed a block boundary
       looks like, and that is still checkable — against the assets themselves.
       The per-asset floor keeps it from passing on an empty tree. */
    /* The ONE non-colour declaration the built-in assets are allowed to carry,
       and the argument for it. `Wasp` is the FRAME theme (goal wasp): its
       palette block always declared a MEASURED `--frame-width` beside its
       colours — the frame is part of what that theme IS, not chrome the theme
       merely tints — so item 3's migration carried the length into the asset
       with the colours it belongs to. Everything else an asset declares must
       be a colour a browser can parse. A second entry here cannot be added by
       an extraction accidentally widening: it has to be typed, with a reason,
       which is the whole point of the list being a list and not a predicate
       like "anything with a unit suffix". */
    const NON_COLOUR_TOKENS: &[&str] = &["--frame-width"];

    /// Every `--token: value;` an asset declares, as (token, value), with the
    /// `var()` rows dropped: a var() row carries no value of its own, it
    /// POINTS at one.
    fn asset_values(css: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for line in css.lines() {
            let line = line.trim();
            if !line.starts_with("--") {
                continue;
            }
            let Some((name, rest)) = line.split_once(':') else { continue };
            let Some((val, _)) = rest.split_once(';') else { continue };
            let (name, val) = (name.trim(), val.trim());
            // a var() row carries no colour of its own: it POINTS at one
            if val.starts_with("var(") || val.is_empty() {
                continue;
            }
            out.push((name.to_string(), val.to_string()));
        }
        out.sort();
        out.dedup();
        out
    }

    /// Every COLOUR value an asset declares is a colour a browser can parse,
    /// no two assets declare the SAME one, and the only declarations exempt
    /// from "is a colour" are the named ones in `NON_COLOUR_TOKENS`.
    #[test]
    fn asset_values_are_wellformed_and_no_two_assets_share_one() {
        let mut seen: Vec<(String, &str)> = Vec::new();
        for t in BUILTIN_THEMES {
            let mut colours: Vec<String> = Vec::new();
            for (name, v) in asset_values(t.css) {
                if NON_COLOUR_TOKENS.contains(&name.as_str()) {
                    // A length may legitimately repeat across assets, so it is
                    // held out of the cross-asset uniqueness check too — the
                    // defect that check exists to catch (an extraction
                    // crossing a block boundary) is about colours.
                    continue;
                }
                let ok = v.starts_with('#')
                    || v.starts_with("rgba(")
                    || v.starts_with("rgb(")
                    || v.starts_with("hsl(")
                    || v == "transparent"
                    || v == "currentColor";
                assert!(
                    ok,
                    "{}: {name} = {v:?} is not a colour literal this app knows how to ship, \
                     and {name} is not in NON_COLOUR_TOKENS",
                    t.name
                );
                colours.push(v);
            }
            // sorted BY VALUE before the dedup: one asset naming the same hex
            // from two tokens (or from its dark and its light block) is one
            // colour, not a collision with itself.
            colours.sort();
            colours.dedup();
            assert!(
                colours.len() >= 30,
                "non-vacuity floor: {} declares {} colour values, expected >= 30",
                t.name,
                colours.len()
            );
            for v in colours {
                if let Some((_, other)) = seen.iter().find(|(s, _)| *s == v) {
                    panic!("{v:?} is declared by BOTH {other} and {} — the extraction crossed a block boundary", t.name);
                }
                seen.push((v, t.name));
            }
        }
    }

    // ============ the gate's PIXEL ANCHORS, pinned to the assets ============
    // scripts/smoke.sh's theme phases read the window and compare it to named
    // hexes. Those hexes used to be pinned by a unit test in `palette.rs`
    // against the table the values lived in; that file is deleted (ledger item
    // 8) and the values live in the ASSETS now, so the pin moved here with
    // them. Without it, an asset edit that moves an anchor is found by a
    // 20-minute gate on the box instead of by a 0.1 s `cargo test`.

    /// The text a `body.theme-<mode>` selector opens, up to its closing brace.
    /// (The assets are generated, one flat declaration block per mode, no
    /// nested rules and no `}` inside a comment — verified for all three.)
    fn mode_block<'a>(css: &'a str, mode: &str) -> &'a str {
        let sel = format!("body.theme-{mode} {{");
        let start = css
            .find(&sel)
            .unwrap_or_else(|| panic!("no `{sel}` block in the asset"))
            + sel.len();
        let rest = &css[start..];
        let end = rest
            .find('}')
            .unwrap_or_else(|| panic!("`{sel}` block is never closed"));
        &rest[..end]
    }

    /// Every `--bg-base` anchor a gate phase asserts in PIXELS is the value
    /// its asset declares, in the right mode block.
    #[test]
    fn the_anchor_values_the_gate_phases_read_are_the_ones_the_assets_declare() {
        // (asset, dark --bg-base, light --bg-base) = smoke.sh's
        // PAL_D_BASE/PAL_L_BASE (phase_palette), SL_D_BASE/SL_L_BASE
        // (phase_obspal), WA_D_BASE/WA_L_BASE (phase_wasp).
        let anchors = [
            ("1984", "#0d0f31", "#e4e5f5"),
            ("Slate", "#1c1c1d", "#fffffe"),
            ("Wasp", "#242425", "#c4c4c5"),
        ];
        for (name, dark, light) in anchors {
            let t = BUILTIN_THEMES.iter().find(|t| t.name == name).unwrap_or_else(|| {
                panic!("{name} is not a shipped asset any more, but scripts/smoke.sh still reads its pixels")
            });
            for (mode, want) in [("dark", dark), ("light", light)] {
                let decl = format!("--bg-base: {want};");
                assert!(
                    mode_block(t.css, mode).contains(&decl),
                    "{name} body.theme-{mode} does not declare `{decl}` — that hex is what a gate phase asserts as a PIXEL, so moving it here without moving it there turns a 0.1 s failure into a 20-minute one"
                );
            }
        }
    }

    // ================= ledger item 4 / criterion 3: SEEDING =================
    // The three legs C3 names, each as its own test, plus the two the
    // per-FILE predicate is there for (a partial dir, and an unwritable
    // vault). A reviewer runs: cargo test --manifest-path src-tauri/Cargo.toml
    // builtins:: -- --nocapture

    /// (bytes, mtime) of every file the seeder can write, so "unchanged" is a
    /// comparison and not a claim. Missing files are recorded as `None` —
    /// present in the map, so a file that VANISHED is a diff, not a silence.
    fn snapshot(root: &Path) -> Vec<(PathBuf, Option<(Vec<u8>, std::time::SystemTime)>)> {
        let td = root.join(".obsidian").join("themes");
        let mut out = Vec::new();
        for t in BUILTIN_THEMES {
            for f in ["manifest.json", "theme.css"] {
                let p = td.join(t.name).join(f);
                let v = fs::read(&p).ok().and_then(|b| {
                    fs::metadata(&p).and_then(|m| m.modified()).ok().map(|m| (b, m))
                });
                out.push((p, v));
            }
        }
        out
    }

    fn names(v: &[String]) -> Vec<&str> {
        v.iter().map(|s| s.as_str()).collect()
    }

    /// C3 LEG 1: a vault with no `.obsidian/themes/` gets the built-ins as
    /// REAL theme dirs, and the predicate that lists third-party themes lists
    /// them — the seeded bytes are the shipped bytes, so there is nothing
    /// "built-in" about them once they are down.
    #[test]
    fn seed_writes_the_builtins_when_the_themes_dir_is_absent() {
        let root = tmp_vault("absent");
        assert!(!root.join(".obsidian/themes").exists(), "precondition: no themes dir");
        let rep = seed_builtin_themes(&root);
        let mut want: Vec<&str> = BUILTIN_THEMES.iter().map(|t| t.name).collect();
        want.sort();
        let mut got = names(&rep.wrote);
        got.sort();
        assert_eq!(got, want, "every built-in is written on a vault that has none");
        assert!(rep.kept.is_empty() && rep.failed.is_empty(), "{rep:?}");
        let s = themes_scan(&root);
        assert_eq!(s.listed, want, "the seeded dirs must be LISTED by themefs's predicate");
        assert!(s.excluded.is_empty(), "{:?}", s.excluded);
        // the bytes on disk ARE the shipped asset, byte for byte
        for t in BUILTIN_THEMES {
            let d = root.join(".obsidian/themes").join(t.name);
            assert_eq!(fs::read_to_string(d.join("manifest.json")).unwrap(), t.manifest);
            assert_eq!(fs::read_to_string(d.join("theme.css")).unwrap(), t.css);
        }
    }

    /// C3 LEG 2: a second boot writes NOTHING — same bytes, same mtime, and
    /// the report says `kept`, which is the decision the census publishes.
    /// (mtime equality on a filesystem with coarse timestamps could hide a
    /// rewrite; that is why `seed_never_overwrites_a_user_edit` exists — an
    /// edit is detectable whatever the clock resolution.)
    #[test]
    fn seed_twice_touches_neither_bytes_nor_mtime() {
        let root = tmp_vault("twice");
        seed_builtin_themes(&root);
        let before = snapshot(&root);
        assert!(before.iter().all(|(_, v)| v.is_some()), "first pass left a file unwritten");
        let rep = seed_builtin_themes(&root);
        assert!(rep.wrote.is_empty(), "a second boot wrote {:?}", rep.wrote);
        assert_eq!(rep.kept.len(), BUILTIN_THEMES.len(), "{rep:?}");
        assert_eq!(snapshot(&root), before, "a second boot changed bytes or mtime");
    }

    /// The rule that makes leg 2 worth anything: an existing file is never
    /// opened, so a user's edit to a built-in survives every later boot.
    #[test]
    fn seed_never_overwrites_a_user_edit() {
        let root = tmp_vault("edit");
        seed_builtin_themes(&root);
        let p = root.join(".obsidian/themes/Wasp/theme.css");
        let edited = format!("{}\nbody.theme-dark {{ --text-normal: #ff00ff; }}\n", BUILTIN_THEMES[2].css);
        assert_eq!(BUILTIN_THEMES[2].name, "Wasp", "index/name drift");
        fs::write(&p, &edited).unwrap();
        let rep = seed_builtin_themes(&root);
        assert!(rep.wrote.is_empty(), "the edit was overwritten: {rep:?}");
        assert_eq!(fs::read_to_string(&p).unwrap(), edited, "the user's bytes must survive");
    }

    /// C3 LEG 3: delete ONE and the next boot restores exactly that one. This
    /// is the leg a global "already seeded this vault" flag fails, and the
    /// reason the state is the filesystem.
    #[test]
    fn seed_restores_only_the_theme_that_was_deleted() {
        let root = tmp_vault("deleted");
        seed_builtin_themes(&root);
        let gone = BUILTIN_THEMES[1].name; // Slate
        fs::remove_dir_all(root.join(".obsidian/themes").join(gone)).unwrap();
        let others = snapshot(&root)
            .into_iter()
            .filter(|(p, _)| !p.starts_with(root.join(".obsidian/themes").join(gone)))
            .collect::<Vec<_>>();
        let rep = seed_builtin_themes(&root);
        assert_eq!(names(&rep.wrote), vec![gone], "only the deleted one is restored");
        assert_eq!(rep.kept.len(), BUILTIN_THEMES.len() - 1, "{rep:?}");
        let after = snapshot(&root)
            .into_iter()
            .filter(|(p, _)| !p.starts_with(root.join(".obsidian/themes").join(gone)))
            .collect::<Vec<_>>();
        assert_eq!(after, others, "restoring one theme touched another");
        let s = themes_scan(&root);
        assert!(s.listed.contains(&gone.to_string()), "restored but not listed: {:?}", s);
    }

    /// The predicate is per FILE, not per directory: a half-deleted built-in
    /// (the dir and one file survive) is completed, and the surviving file is
    /// not rewritten. A per-directory check would leave this vault with a
    /// theme `themes_scan` excludes for ever.
    #[test]
    fn seed_completes_a_half_deleted_theme_without_touching_its_sibling() {
        let root = tmp_vault("partial");
        seed_builtin_themes(&root);
        let d = root.join(".obsidian/themes").join(BUILTIN_THEMES[0].name);
        fs::remove_file(d.join("theme.css")).unwrap();
        let m_before = fs::metadata(d.join("manifest.json")).unwrap().modified().unwrap();
        assert!(!themes_scan(&root).excluded.is_empty(), "precondition: the half theme is excluded");
        let rep = seed_builtin_themes(&root);
        assert_eq!(names(&rep.wrote), vec![BUILTIN_THEMES[0].name], "{rep:?}");
        assert_eq!(fs::read_to_string(d.join("theme.css")).unwrap(), BUILTIN_THEMES[0].css);
        assert_eq!(
            fs::metadata(d.join("manifest.json")).unwrap().modified().unwrap(),
            m_before,
            "the sibling file was rewritten"
        );
        assert!(themes_scan(&root).excluded.is_empty(), "still excluded after completion");
    }

    /// A vault we cannot write to must OPEN anyway: the failure is collected
    /// and reported (the census's `f<n>`), never panicked, never silent. The
    /// unwritable case is forced the only portable way — a regular FILE where
    /// the themes directory belongs, which is also a real user's typo.
    #[test]
    fn seed_reports_an_unwritable_vault_instead_of_panicking() {
        let root = tmp_vault("unwritable");
        fs::write(root.join(".obsidian").join("themes"), b"not a directory\n").unwrap();
        let rep = seed_builtin_themes(&root);
        assert!(rep.wrote.is_empty() && rep.kept.is_empty(), "{rep:?}");
        assert_eq!(rep.failed.len(), BUILTIN_THEMES.len(), "every theme must report: {rep:?}");
        for f in &rep.failed {
            assert!(f.contains(':'), "a failure names the theme and the error: {f:?}");
        }
        assert!(rep.line().contains("failed ["), "the console line carries the failures");
        assert_eq!(themes_scan(&root).listed.len(), 0, "nothing was listed, and nothing crashed");
    }
}
