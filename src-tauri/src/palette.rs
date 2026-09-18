// SPDX-License-Identifier: GPL-3.0-or-later
/*! THE PALETTE AXIS — the second, independent theme axis.

The app already had ONE theme axis, and it is not this one:

    MODE     light | dark     cfg key "theme"    main.rs set_theme / get_theme
    PALETTE  default | 1984 | slate   cfg "palette"  main.rs set_palette / get_palette

They COMPOSE and neither overloads the other. Mode keeps its whole existing
contract: it defaults to the system's prefers-color-scheme until the user picks
one, Ctrl+P "Toggle light/dark mode" flips it, and set_theme refuses anything
that is not "dark" or "light" so a typo can never read back as "no choice".
Palette is a second attribute on the same root element, and light/dark still
selects that palette's own variant — the 1984 scheme ships both, so the two
axes multiply instead of colliding.

WHY A TABLE AND NOT A MATCH ARM. The known names have to agree in four places:
the Rust validator, the Ctrl+P command registry, the settings dropdown, and the
stylesheet. This list is the one of them, and the tests below pin the other
three to it — a palette added here with no CSS block, or a CSS block with no
entry here, is a failing test rather than a user-visible dead menu item.

THE DEFAULT IS NOT A PALETTE FILE. `default` is the ABSENCE of a palette: the
frontend removes the data-palette attribute entirely, so not one palette
selector matches and the pixels are the ones this app always painted. That is
deliberate — a "default palette" expressed as a CSS block would be a second
definition of the default that could drift from the first.
*/

/// (id, the name shown to a human). `default` first: it is the fallback, and
/// `PALETTES[0].0` is what an unknown name resolves to.
pub const PALETTES: &[(&str, &str)] = &[("default", "Default"), ("1984", "1984"), ("slate", "Slate")];

/// the id stored/applied when nothing valid was chosen
pub const DEFAULT_PALETTE: &str = PALETTES[0].0;

/// Is this a palette we ship? The ONLY gate a value passes on its way into
/// ~/.rustidian.json — same shape as set_theme's dark|light check, and for the
/// same reason: a junk value that reaches the file reads back as a real choice
/// on the next boot and pins the user to a palette that does not exist.
pub fn is_known(name: &str) -> bool {
    PALETTES.iter().any(|(id, _)| *id == name)
}

/// a stored/incoming value normalised to something that can be APPLIED.
/// Junk does not become an error the user has to clear — it becomes the default.
pub fn resolve(name: &str) -> &'static str {
    PALETTES
        .iter()
        .find(|(id, _)| *id == name)
        .map(|(id, _)| *id)
        .unwrap_or(DEFAULT_PALETTE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSS: &str = include_str!("../../ui/style.css");
    const MAIN_JS: &str = include_str!("../../ui/main.js");

    #[test]
    fn default_is_the_first_entry_and_the_fallback() {
        assert_eq!(DEFAULT_PALETTE, "default");
        assert_eq!(resolve("default"), "default");
        assert_eq!(resolve("1984"), "1984");
    }

    /* The validation criterion, in both directions: a name we do not ship is
       refused, and refusal resolves to the DEFAULT rather than to nothing. */
    #[test]
    fn an_unknown_palette_is_refused_and_falls_back_to_the_default() {
        for junk in ["", " ", "1985", "1984 ", "Default", "nineteen-eighty-four", "../../etc/passwd", "dark", "light"] {
            assert!(!is_known(junk), "{junk:?} must not be a known palette");
            assert_eq!(resolve(junk), DEFAULT_PALETTE, "{junk:?} must fall back to the default");
        }
    }

    /* MODE IS A DIFFERENT AXIS. "dark" and "light" are the values of `theme`;
       if either ever became a palette id the two axes would be overloaded onto
       one field, which is the single mistake this feature is built to avoid. */
    #[test]
    fn mode_values_are_not_palette_ids() {
        assert!(!is_known("dark") && !is_known("light"));
    }

    /// every non-default palette has BOTH css variants; the default has NEITHER
    #[test]
    fn every_palette_has_its_css_and_the_default_has_none() {
        for (id, _) in PALETTES {
            let dark = format!(":root[data-palette=\"{id}\"] {{");
            let light = format!(":root[data-palette=\"{id}\"][data-theme=\"light\"] {{");
            if *id == DEFAULT_PALETTE {
                assert!(!CSS.contains(&dark), "the default palette must be the ABSENCE of a css block, but {dark} exists");
                assert!(!CSS.contains(&light), "the default palette must be the ABSENCE of a css block, but {light} exists");
            } else {
                assert!(CSS.contains(&dark), "palette {id:?} has no dark css block ({dark})");
                assert!(CSS.contains(&light), "palette {id:?} has no light css block ({light})");
            }
        }
    }

    /// no css block claims a palette the Rust side would refuse
    #[test]
    fn no_css_block_names_a_palette_that_is_not_in_the_table() {
        for (i, _) in CSS.match_indices("[data-palette=\"") {
            let rest = &CSS[i + "[data-palette=\"".len()..];
            let name = &rest[..rest.find('"').expect("unterminated data-palette selector")];
            assert!(is_known(name), "ui/style.css styles palette {name:?}, which is not in PALETTES");
        }
    }

    /* THE CASCADE BUG THIS EXISTS TO CATCH.
       :root[data-theme="light"] and :root[data-palette="<id>"] have the SAME
       specificity (0,2,0) and the palette block comes LATER in the file, so on
       light+<id> the palette's DARK value wins unless the (0,3,0) block
       re-declares it. A token declared in one variant and not the other is
       therefore a token that paints dark ink on light paper — invisible in
       review, obvious to a user. Each variant must be TOTAL. */
    #[test]
    fn the_two_variants_of_a_palette_declare_the_identical_token_set() {
        for (id, _) in PALETTES.iter().filter(|(id, _)| *id != DEFAULT_PALETTE) {
            let dark = tokens_of(&format!(":root[data-palette=\"{id}\"] {{"));
            let light = tokens_of(&format!(":root[data-palette=\"{id}\"][data-theme=\"light\"] {{"));
            assert!(dark.len() > 20, "palette {id:?} dark variant declares only {} tokens", dark.len());
            let only_dark: Vec<&String> = dark.iter().filter(|t| !light.contains(t)).collect();
            let only_light: Vec<&String> = light.iter().filter(|t| !dark.contains(t)).collect();
            assert!(
                only_dark.is_empty() && only_light.is_empty(),
                "palette {id:?}: variants are not total — dark-only {only_dark:?}, light-only {only_light:?}"
            );
        }
    }

    /// the two anchor colours the smoke phase asserts from PIXELS, pinned here
    /// so a stylesheet edit that moves them fails in 0.1s instead of 20 minutes
    #[test]
    fn the_1984_backgrounds_are_the_upstream_values() {
        assert_eq!(decl(":root[data-palette=\"1984\"] {", "--bg-base").as_deref(), Some("#0d0f31"));
        assert_eq!(
            decl(":root[data-palette=\"1984\"][data-theme=\"light\"] {", "--bg-base").as_deref(),
            Some("#e4e5f5")
        );
    }

    // ---- `slate`: the palette whose whole claim is that its numbers are traceable ----

    const SLATE_DARK: &str = ":root[data-palette=\"slate\"] {";
    const SLATE_LIGHT: &str = ":root[data-palette=\"slate\"][data-theme=\"light\"] {";
    const SOURCE_MD: &str = include_str!("../../docs/palette-slate/SOURCE.md");

    /// the three bands the `obspal` smoke phase reads out of PIXELS, pinned to
    /// docs/palette-slate/SOURCE.md's table so a stylesheet edit that moves one
    /// of them fails in 0.1s instead of 20 minutes of X11 — and so the phase's
    /// expected values have exactly one definition anyone can find.
    #[test]
    fn the_slate_bands_are_the_values_source_md_publishes() {
        for (sel, base, sidebar, ribbon) in [
            (SLATE_DARK, "#1c1c1d", "#282829", "#2e2e2f"),
            (SLATE_LIGHT, "#fffffe", "#f6f6f7", "#efefee"),
        ] {
            assert_eq!(decl(sel, "--bg-base").as_deref(), Some(base));
            assert_eq!(decl(sel, "--bg-sidebar").as_deref(), Some(sidebar));
            assert_eq!(decl(sel, "--bg-ribbon").as_deref(), Some(ribbon));
        }
    }

    /* THE CHECK THAT MUST NOT GIVE THE SAME ANSWER TWICE. A dark and a light
       assertion that would both pass on ONE palette block are one assertion
       wearing two hats. These three bands are the ones the smoke phase
       measures, so if they ever coincide the pixel evidence stops meaning
       "both variants exist". */
    #[test]
    fn the_two_slate_variants_are_not_the_same_window() {
        for token in ["--bg-base", "--bg-sidebar", "--bg-ribbon"] {
            assert_ne!(
                decl(SLATE_DARK, token),
                decl(SLATE_LIGHT, token),
                "{token} is identical in both slate variants — the light assertion would pass on the dark block"
            );
        }
        // and the ribbon must not collide with the sidebar, or R8's deviation
        // (the reason the light variant is distinguishable at all) is gone
        assert_ne!(decl(SLATE_LIGHT, "--bg-ribbon"), decl(SLATE_LIGHT, "--bg-sidebar"));
    }

    /* PROVENANCE IS THE POINT, AND IT IS MACHINE-CHECKED.
       Every literal in both slate blocks carries a comment naming the upstream
       variable it came from. An undocumented hex here is indistinguishable
       from a correct one in a screenshot, which is exactly why this is a test
       and not a review convention. */
    #[test]
    fn every_slate_literal_names_its_upstream_source() {
        for sel in [SLATE_DARK, SLATE_LIGHT] {
            for line in block(sel).lines().map(str::trim).filter(|l| l.starts_with("--")) {
                assert!(
                    line.contains("(upstream)"),
                    "no provenance comment on {sel} line {line:?}"
                );
            }
        }
    }

    /* THE TABLE IN SOURCE.md IS THE PALETTE, OR IT IS FICTION.
       docs/palette-slate/SOURCE.md accounts for every token with the value it
       says we ship; this reads that table back and compares it to the
       stylesheet in BOTH directions, so neither a doc that drifted nor a hex
       that was "fixed" in the CSS alone can survive. */
    #[test]
    fn source_md_accounts_for_every_slate_token_with_the_value_we_ship() {
        let rows: Vec<(String, String, String)> = SOURCE_MD
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split('|').collect();
                // | n | token | L upstream | L shipped | D upstream | D shipped | src |
                if f.len() < 8 {
                    return None;
                }
                f[1].trim().parse::<u32>().ok()?;
                let cell = |i: usize| f[i].trim().trim_matches('`').trim().to_string();
                let tok = cell(2);
                // columns 4 and 6 are the SHIPPED values — the ones the
                // stylesheet must declare. 3 and 5 are their upstream
                // originals, which check-offsets.sh relates to them.
                tok.starts_with("--").then(|| (tok, cell(4), cell(6)))
            })
            .collect();

        let dark = tokens_of(SLATE_DARK);
        assert_eq!(
            rows.len(),
            dark.len(),
            "SOURCE.md documents {} tokens but the slate block declares {}",
            rows.len(),
            dark.len()
        );
        // the criterion the goal states in so many words: a palette must be
        // TOTAL, and 1984 is the definition of the complete token set
        assert_eq!(dark.len(), tokens_of(":root[data-palette=\"1984\"] {").len());

        for (token, light_value, dark_value) in &rows {
            assert_eq!(
                decl(SLATE_LIGHT, token).as_deref(),
                Some(light_value.as_str()),
                "SOURCE.md's LIGHT value for {token} is not what ui/style.css ships"
            );
            assert_eq!(
                decl(SLATE_DARK, token).as_deref(),
                Some(dark_value.as_str()),
                "SOURCE.md's DARK value for {token} is not what ui/style.css ships"
            );
        }
        for token in tokens_of(SLATE_LIGHT).iter().chain(dark.iter()) {
            assert!(
                rows.iter().any(|(t, _, _)| t == token),
                "{token} is shipped in a slate block but SOURCE.md accounts for no such token"
            );
        }
    }

    /* THE WATERMARK, RE-DERIVED IN THE TEST SUITE.
       docs/palette-slate/SOURCE.md records two numbers per token per mode: the
       upstream value, and the value we ship — which is the upstream one with
       its LAST NIBBLE moved exactly one step (f down, everything else up), so
       the shift stays inside one channel and inside the ±2 pixel tolerance.
       docs/palette-slate/check-offsets.sh is the runnable form of this rule for
       a reviewer; this is the same rule inside `cargo test`, so a hex edited in
       either file alone cannot reach a green gate. Literals with no nibble to
       move (rgba(), hsl()) must be recorded identical. */
    #[test]
    fn every_shipped_slate_value_is_its_upstream_with_the_last_nibble_moved_one_step() {
        fn step(c: char) -> char {
            match c {
                'f' => 'e',
                c => char::from_digit(c.to_digit(16).unwrap() + 1, 16).unwrap(),
            }
        }
        fn watermark(upstream: &str) -> String {
            let is_hex = upstream.starts_with('#')
                && (upstream.len() == 7 || upstream.len() == 9)
                && upstream[1..].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
            if !is_hex {
                return upstream.to_string(); // rgba()/hsl(): nothing to move
            }
            let (head, last) = upstream.split_at(upstream.len() - 1);
            format!("{head}{}", step(last.chars().next().unwrap()))
        }

        let mut checked = 0;
        for line in SOURCE_MD.lines() {
            let f: Vec<&str> = line.split('|').collect();
            if f.len() < 8 {
                continue;
            }
            if f[1].trim().parse::<u32>().is_err() {
                continue;
            }
            let cell = |i: usize| f[i].trim().trim_matches('`').trim().to_string();
            let token = cell(2);
            if !token.starts_with("--") {
                continue;
            }
            for (mode, up, shipped) in [("light", cell(3), cell(4)), ("dark", cell(5), cell(6))] {
                assert_eq!(
                    watermark(&up),
                    shipped,
                    "{token} {mode}: shipped {shipped} is not upstream {up} with the last nibble moved one step"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 72, "expected 36 tokens (35 colours + the --graph-bg alias) x 2 modes of watermark pairs in SOURCE.md");
    }

    /* LICENSING: the upstream repo ships no LICENSE, so nothing of it may be
       vendored. Colour VALUES are facts and travel; FILES do not. Attribution
       is carried in the stylesheet next to the values it belongs to. */
    #[test]
    fn the_palette_is_attributed_and_no_upstream_file_is_vendored() {
        assert!(
            CSS.contains("github.com/covertbert/iterm2-1984"),
            "the 1984 palette must name its source in ui/style.css"
        );
        assert!(!CSS.contains(".itermcolors\""), "no upstream file may be referenced as an asset");
    }

    /// the frontend registry must offer exactly the palettes Rust would accept
    #[test]
    fn the_frontend_registry_lists_the_same_palettes() {
        for (id, label) in PALETTES {
            assert!(
                MAIN_JS.contains(&format!("[\"{id}\", \"{label}\"]")),
                "ui/main.js PALETTES is missing [\"{id}\", \"{label}\"]"
            );
        }
    }

    // ---- helpers: a deliberately small css reader, enough for one rule block ----
    fn block(selector: &str) -> &'static str {
        let start = CSS.find(selector).unwrap_or_else(|| panic!("no such rule: {selector}"));
        let rest = &CSS[start + selector.len()..];
        let end = rest.find("\n}").unwrap_or_else(|| panic!("unterminated rule: {selector}"));
        &rest[..end]
    }

    fn tokens_of(selector: &str) -> Vec<String> {
        block(selector)
            .lines()
            .filter_map(|l| {
                let l = l.trim();
                let name = l.strip_prefix("--")?;
                Some(format!("--{}", name.split(':').next()?.trim()))
            })
            .collect()
    }

    fn decl(selector: &str, token: &str) -> Option<String> {
        block(selector).lines().find_map(|l| {
            let l = l.trim();
            let (name, val) = l.split_once(':')?;
            (name.trim() == token).then(|| val.split(';').next().unwrap_or("").trim().to_string())
        })
    }
}
