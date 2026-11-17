//! #220 UI translations (Project Fluent). ONE copy of every string table:
//! locales/<lang>/main.ftl at the repo root is embedded here (include_str!) and
//! handed to the webview by `i18n_load` (ui/i18n.js formats it with the vendored
//! @fluent/bundle), while Rust-side text (native dialogs) is formatted by the
//! fluent-bundle crate from the same bytes. No file is read at runtime and
//! nothing is fetched: a language is exactly the .ftl compiled into the binary.
//!
//! English is the source language and the fallback for every missing message.
//! Placeables are NOT wrapped in Unicode isolation marks (use_isolating off on
//! both sides) so the English UI stays byte-identical to the pre-i18n strings.
// tr()/ui_lang() get their first native callers with the consent dialog extraction
#![cfg_attr(not(test), allow(dead_code))]
use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};

/// (code, native name, ftl source) in the order the Language list shows them.
/// The native names are what each speaker calls the language; they are never
/// translated (a Japanese UI still lists "Español").
pub const LANGS: &[(&str, &str, &str)] = &[
    ("en", "English", include_str!("../../locales/en/main.ftl")),
    ("zh-CN", "中文", include_str!("../../locales/zh-CN/main.ftl")),
    ("ko", "한국어", include_str!("../../locales/ko/main.ftl")),
    ("ru", "Русский", include_str!("../../locales/ru/main.ftl")),
    ("es", "Español", include_str!("../../locales/es/main.ftl")),
    ("ja", "日本語", include_str!("../../locales/ja/main.ftl")),
];

/// the persisted value meaning "follow the system"; also the default
pub const AUTO: &str = "auto";

/// the .ftl source of a supported language code, None for anything else
pub fn ftl(lang: &str) -> Option<&'static str> {
    LANGS.iter().find(|l| l.0 == lang).map(|l| l.2)
}

/// is `s` a value the Language setting may hold ("auto" or a LANGS code)
pub fn valid_setting(s: &str) -> bool {
    s == AUTO || ftl(s).is_some()
}

/// map one locale tag (POSIX "ja_JP.UTF-8", BCP 47 "zh-Hans-CN", "ru") to a
/// supported code. Every Chinese tag maps to zh-CN, the one Chinese table.
pub fn match_tag(tag: &str) -> Option<&'static str> {
    let t = tag.split(['.', '@']).next().unwrap_or("").replace('_', "-");
    let primary = t.split('-').next().unwrap_or("").to_ascii_lowercase();
    match primary.as_str() {
        "en" => Some("en"),
        "zh" => Some("zh-CN"),
        "ko" => Some("ko"),
        "ru" => Some("ru"),
        "es" => Some("es"),
        "ja" => Some("ja"),
        _ => None,
    }
}

/// the system language from the POSIX locale variables, in their precedence
/// order LC_ALL > LC_MESSAGES > LANG (the first one set and non-empty decides,
/// like setlocale(LC_MESSAGES)); unsupported or C/POSIX -> English.
pub fn system_lang_from(get: impl Fn(&str) -> Option<String>) -> &'static str {
    for k in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(v) = get(k).filter(|v| !v.is_empty()) {
            return match_tag(&v).unwrap_or("en");
        }
    }
    "en"
}

pub fn system_lang() -> &'static str {
    system_lang_from(|k| std::env::var(k).ok())
}

/// the language a setting value resolves to on this system
pub fn resolve(setting: &str) -> &'static str {
    if setting == AUTO {
        return system_lang();
    }
    LANGS.iter().find(|l| l.0 == setting).map(|l| l.0).unwrap_or("en")
}

fn bundle(lang: &str) -> Option<FluentBundle<FluentResource>> {
    let src = ftl(lang)?;
    let res = FluentResource::try_new(src.to_string()).ok()?;
    let id = lang.parse().ok()?;
    let mut b = FluentBundle::new(vec![id]);
    b.set_use_isolating(false);
    b.add_resource(res).ok()?;
    Some(b)
}

fn format_in(lang: &str, id: &str, args: &[(&str, &str)]) -> Option<String> {
    let b = bundle(lang)?;
    let pattern = b.get_message(id)?.value()?;
    let mut fa = FluentArgs::new();
    for (k, v) in args {
        fa.set(*k, FluentValue::from(*v));
    }
    let mut errs = vec![];
    let s = b.format_pattern(pattern, Some(&fa), &mut errs);
    errs.is_empty().then(|| s.into_owned())
}

/// Rust-side text: message `id` in `lang`, else in English, else the id itself
/// (never a blank label). Native dialogs are rare, so the bundle is built per
/// call instead of held in a global.
pub fn tr(lang: &str, id: &str, args: &[(&str, &str)]) -> String {
    format_in(lang, id, args)
        .or_else(|| format_in("en", id, args))
        .unwrap_or_else(|| id.to_string())
}

/// the id fragment ui/i18n.js `slug()` derives from an English literal (the
/// settings table, command ids, menu titles): "..." -> " dots ", ASCII
/// lowercase, every run of anything but [a-z0-9] -> "-", no leading/trailing "-"
pub fn slug(s: &str) -> String {
    let s = s.replace("...", " dots ").to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// message and term ids of one .ftl source, sorted; Err on any parse error.
/// Fluent entries start in column 0 with their identifier (continuation lines
/// are indented, comments start with #), so the ids are read off those lines
/// once the whole source has parsed cleanly.
pub fn keys(src: &str) -> Result<Vec<String>, String> {
    FluentResource::try_new(src.to_string()).map_err(|(_, e)| format!("{e:?}"))?;
    let mut out: Vec<String> = src
        .lines()
        .filter_map(|l| {
            let (id, _) = l.split_once('=')?;
            let id = id.trim_end();
            let body = id.strip_prefix('-').unwrap_or(id);
            let ok = body.chars().next()?.is_ascii_alphabetic()
                && body.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            ok.then(|| id.to_string())
        })
        .collect();
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_locale_parses_and_has_the_english_key_set() {
        let en = keys(ftl("en").unwrap()).expect("en parses");
        assert!(!en.is_empty());
        let mut dup = en.clone();
        dup.dedup();
        assert_eq!(dup.len(), en.len(), "en has a duplicate id");
        for (code, _, src) in LANGS {
            let k = keys(src).unwrap_or_else(|e| panic!("{code}/main.ftl does not parse: {e}"));
            let missing: Vec<_> = en.iter().filter(|x| !k.contains(x)).collect();
            let extra: Vec<_> = k.iter().filter(|x| !en.contains(x)).collect();
            assert!(missing.is_empty() && extra.is_empty(), "{code}: missing {missing:?} extra {extra:?}");
        }
    }

    #[test]
    fn every_message_formats_without_errors_in_every_language() {
        // placeholders a translation uses must exist in English (args are
        // supplied from the English message's variables)
        for (code, _, _) in LANGS {
            let b = bundle(code).unwrap_or_else(|| panic!("{code} bundle"));
            for id in keys(ftl("en").unwrap()).unwrap() {
                if id.starts_with('-') {
                    continue;
                }
                let m = b.get_message(&id).unwrap_or_else(|| panic!("{code}: no {id}"));
                if let Some(p) = m.value() {
                    let mut fa = FluentArgs::new();
                    for v in ["v", "name", "count", "n", "path", "program", "plugin", "vault", "file", "folder", "version", "key", "url", "error", "lang"] {
                        fa.set(v, FluentValue::from("x"));
                    }
                    let mut errs = vec![];
                    let _ = b.format_pattern(p, Some(&fa), &mut errs);
                    assert!(errs.is_empty(), "{code} {id}: {errs:?}");
                }
            }
        }
    }

    /* #220 item 5: English stays byte-identical. Every surface that was extracted
       derives its id from English data (ui/main.js sRowId / cmdRow, ui/i18n.js m());
       these pin each derived en message to the literal it replaced, so an edit to
       the table, the registry or a menu call site that is not mirrored in en/main.ftl
       fails here instead of silently painting a stale string. */
    fn en(id: &str, args: &[(&str, &str)]) -> Option<String> {
        format_in("en", id, args)
    }
    /// "Version 0.9.6" -> ("Version", Some("0.9.6")), like main.js sVerArg
    fn split_version(label: &str) -> (&str, Option<&str>) {
        match label.rsplit_once(' ') {
            Some((head, v)) if v.starts_with(|c: char| c.is_ascii_digit()) && v.chars().all(|c| c.is_ascii_digit() || c == '.') => (head, Some(v)),
            _ => (label, None),
        }
    }

    #[test]
    fn slug_matches_the_webview_rule() {
        assert_eq!(slug("Open in new tab"), "open-in-new-tab");
        assert_eq!(slug("Rename..."), "rename-dots");
        assert_eq!(slug("Sort by column (A to Z)"), "sort-by-column-a-to-z");
        assert_eq!(slug("(vault root)"), "vault-root");
        assert_eq!(slug("app:open-settings"), "app-open-settings");
        assert_eq!(slug("Read-only"), "read-only");
    }

    #[test]
    fn settings_strings_are_extracted() {
        let m = crate::settings::model();
        let mut bad = vec![];
        let mut chk = |id: String, args: &[(&str, &str)], want: &str| {
            if en(&id, args).as_deref() != Some(want) {
                bad.push(format!("{id} != {want:?} (got {:?})", en(&id, args)));
            }
        };
        for n in &m.nav {
            chk(format!("snav-{}", n.id), &[], n.entry);
            chk(format!("snavh-{}", slug(n.group)), &[], n.group);
        }
        let mut painted = 0;
        for r in &m.rows {
            if r.tab == "hotkeys" || r.tab == "keychain" {
                continue; // built by code (renderHk / ui/keychain.js), not from the table
            }
            painted += 1;
            let (head, v) = split_version(r.label);
            let id = format!("set-{}-{}", r.tab, slug(head));
            let args: Vec<(&str, &str)> = v.map(|v| ("v", v)).into_iter().collect();
            chk(id.clone(), &args, r.label);
            if !matches!(r.desc, "" | "-" | "(external link row)") {
                chk(format!("{id}-desc"), &[], r.desc);
            }
            if let Some(s) = r.section {
                chk(format!("ssec-{}", slug(s)), &[], s);
            }
        }
        assert!(painted > 50, "only {painted} painted rows");
        assert!(bad.is_empty(), "{} settings strings not extracted:\n{}", bad.len(), bad.join("\n"));
    }

    #[test]
    fn commands_are_extracted() {
        let js = include_str!("../../ui/main.js");
        let start = js.find("\nconst CMDS = [\n").expect("CMDS registry");
        let end = start + js[start..].find("\n].map(").expect("CMDS end");
        let mut n = 0;
        let mut bad = vec![];
        for l in js[start..end].lines() {
            let l = l.trim_start();
            let Some(rest) = l.strip_prefix("[\"") else { continue };
            let Some((id, rest)) = rest.split_once('"') else { continue };
            let rest = rest.trim_start_matches(|c: char| c == ',' || c == ' ');
            let Some(rest) = rest.strip_prefix('"') else { continue };
            let Some((name, _)) = rest.split_once('"') else { continue };
            n += 1;
            let k = format!("cmd-{}", slug(id));
            if en(&k, &[]).as_deref() != Some(name) {
                bad.push(format!("{k} != {name:?}"));
            }
        }
        for t in 1..=8 {
            let k = format!("cmd-workspace-goto-tab-{t}");
            if en(&k, &[]) != Some(format!("Go to tab #{t}")) {
                bad.push(k);
            }
        }
        assert!(n > 50, "only {n} registry rows found");
        assert!(bad.is_empty(), "commands not extracted: {bad:?}");
    }

    #[test]
    fn menu_titles_are_extracted() {
        let files = [
            include_str!("../../ui/main.js"),
            include_str!("../../ui/edmenu.js"),
            include_str!("../../ui/props.js"),
            include_str!("../../ui/plugins/canvas-menus.js"),
            include_str!("../../ui/plugins/canvas-edges.js"),
        ];
        let mut n = 0;
        let mut bad = vec![];
        for src in files {
            for part in src.split("I18N.m(\"").skip(1) {
                let Some((lit, tail)) = part.split_once('"') else { continue };
                assert!(tail.starts_with(')'), "I18N.m takes one plain literal, got {lit:?}{}", &tail[..tail.len().min(20)]);
                n += 1;
                let k = format!("menu-{}", slug(lit));
                if en(&k, &[]).as_deref() != Some(lit) {
                    bad.push(format!("{k} != {lit:?}"));
                }
            }
        }
        assert!(n > 150, "only {n} menu call sites found");
        assert!(bad.is_empty(), "menu titles not extracted: {bad:?}");
    }

    #[test]
    fn native_names_match_the_issue_list() {
        let names: Vec<_> = LANGS.iter().map(|l| l.1).collect();
        assert_eq!(names, ["English", "中文", "한국어", "Русский", "Español", "日本語"]);
    }

    #[test]
    fn system_lang_follows_posix_precedence() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|p| p.0 == k).map(|p| p.1.to_string())
        };
        assert_eq!(system_lang_from(env(&[("LANG", "ja_JP.UTF-8")])), "ja");
        assert_eq!(system_lang_from(env(&[("LANG", "ja_JP.UTF-8"), ("LC_MESSAGES", "ru_RU.UTF-8")])), "ru");
        assert_eq!(system_lang_from(env(&[("LANG", "ja_JP.UTF-8"), ("LC_ALL", "es_ES.UTF-8")])), "es");
        assert_eq!(system_lang_from(env(&[("LC_ALL", ""), ("LANG", "ko_KR.UTF-8")])), "ko");
        assert_eq!(system_lang_from(env(&[("LANG", "zh_CN.UTF-8")])), "zh-CN");
        assert_eq!(system_lang_from(env(&[("LANG", "C.UTF-8")])), "en");
        assert_eq!(system_lang_from(env(&[("LANG", "de_DE.UTF-8")])), "en");
        assert_eq!(system_lang_from(env(&[])), "en");
        assert_eq!(match_tag("zh-Hans-CN"), Some("zh-CN"));
        assert_eq!(match_tag("sr@latin"), None);
    }

    #[test]
    fn missing_message_falls_back_to_english_then_the_id() {
        assert_eq!(tr("ja", "no-such-message-220", &[]), "no-such-message-220");
        assert_eq!(tr("xx", "language-auto", &[]), tr("en", "language-auto", &[]));
        assert_eq!(resolve("ja"), "ja");
        assert_eq!(resolve("xx"), "en");
        assert!(valid_setting("auto") && valid_setting("zh-CN") && !valid_setting("de"));
    }
}
