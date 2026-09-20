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

// The seeding step (ledger item 4) is this module's first non-test consumer;
// until it lands the tests below are the only caller. An allow beats inventing
// a use for the compiler's benefit — and it is scoped to this file.
#![allow(dead_code)]

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
pub const BUILTIN_THEMES: &[BuiltinTheme] = &[
    builtin!("1984", "1984"),
    builtin!("Slate", "Slate"),
    builtin!("Wasp", "Wasp"),
];

/// Is `name` one of ours? (Used by the seeding step in item 4 and by the
/// settings pane to mark a row; it is NOT a privileged load path — after
/// seeding a built-in is an ordinary file on disk with no second code path.)
pub fn is_builtin(name: &str) -> bool {
    BUILTIN_THEMES.iter().any(|t| t.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themefs::{sanitize_css, themes_scan};
    use std::fs;
    use std::path::{Path, PathBuf};

    const STYLE_CSS: &str = include_str!("../../ui/style.css");

    fn tmp_vault(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("rustidian-builtins-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        root
    }

    /// write every shipped asset into `root`'s theme directory, the way the
    /// seeding step will (item 4). Nothing clever: a directory and two files.
    fn write_assets(root: &Path) {
        for t in BUILTIN_THEMES {
            let d = root.join(".obsidian").join("themes").join(t.name);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("manifest.json"), t.manifest).unwrap();
            fs::write(d.join("theme.css"), t.css).unwrap();
        }
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

    // ---- the migration itself: no colour was lost on the way out ----

    /// Every colour VALUE declared in a `:root[data-palette=...]` block of
    /// ui/style.css, as written (a `var(...)` reference is not a value).
    fn palette_block_values() -> Vec<String> {
        let mut out = Vec::new();
        let mut inside = false;
        for line in STYLE_CSS.lines() {
            if line.starts_with(":root[data-palette=") {
                inside = true;
                continue;
            }
            if inside && line.starts_with('}') {
                inside = false;
                continue;
            }
            if !inside || !line.starts_with("  --") {
                continue;
            }
            let Some((_, rest)) = line.split_once(':') else { continue };
            let Some((val, _)) = rest.split_once(';') else { continue };
            let val = val.trim();
            if val.starts_with("var(") || val.is_empty() {
                continue;
            }
            out.push(val.to_string());
        }
        out.sort();
        out.dedup();
        out
    }

    /// The migration criterion of ledger item 3: every colour the palette
    /// blocks carry appears in EXACTLY ONE asset. Not zero (it was dropped on
    /// the way out) and not two (two themes would share a literal, which for
    /// these three palettes would mean the extraction crossed a block
    /// boundary).
    ///
    /// THIS TEST DIES WITH THE BLOCKS. Item 5 deletes the per-palette blocks
    /// from ui/style.css; on that commit `palette_block_values()` returns
    /// nothing, the floor below fires, and the test must be REMOVED in the
    /// same commit — it is a migration check, and a migration check that
    /// passes vacuously after the migration is worse than no check.
    #[test]
    fn every_palette_colour_is_carried_by_exactly_one_asset() {
        let vals = palette_block_values();
        assert!(
            vals.len() >= 100,
            "non-vacuity floor: ui/style.css declared {} palette values, expected >= 100 \
             (if the palette blocks are GONE, delete this test with them — see the doc comment)",
            vals.len()
        );
        for v in &vals {
            let hits: Vec<&str> = BUILTIN_THEMES
                .iter()
                .filter(|t| t.css.contains(v.as_str()))
                .map(|t| t.name)
                .collect();
            assert_eq!(hits.len(), 1, "{v:?} appears in {hits:?}, want exactly one asset");
        }
    }

    /// The other direction, so the assets cannot quietly INVENT a colour: every
    /// hex literal in an asset is a hex literal ui/style.css declares. (Values
    /// mapped onto stock names are copies, so they are covered by this too.)
    #[test]
    fn an_asset_invents_no_colour_of_its_own() {
        for t in BUILTIN_THEMES {
            for line in t.css.lines() {
                let line = line.trim();
                if !line.starts_with("--") {
                    continue;
                }
                let Some((_, rest)) = line.split_once(':') else { continue };
                let Some((val, _)) = rest.split_once(';') else { continue };
                let val = val.trim();
                if val.starts_with("var(") {
                    continue;
                }
                assert!(
                    STYLE_CSS.contains(val),
                    "{}: {val:?} is in no palette block of ui/style.css",
                    t.name
                );
            }
        }
    }

    /// Until item 8 deletes `palette::PALETTES`, the two lists must agree:
    /// every palette except `default` ships as an asset under its MENU name.
    /// A palette added to the table with no asset would be a dead menu row.
    #[test]
    fn every_palette_but_the_default_ships_as_an_asset() {
        for (id, label) in crate::palette::PALETTES {
            if *id == crate::palette::DEFAULT_PALETTE {
                continue; // the default is the absence of a theme (module doc)
            }
            assert!(is_builtin(label), "palette {id:?} ({label:?}) ships no asset");
        }
        assert_eq!(
            BUILTIN_THEMES.len(),
            crate::palette::PALETTES.len() - 1,
            "an asset with no palette row (or the reverse)"
        );
    }
}
