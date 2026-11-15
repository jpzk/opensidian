// SPDX-License-Identifier: GPL-3.0-or-later
/*! Issue #119 — the Rust half of Properties.

 * `.obsidian/types.json` (`{"types": {name: type}}`, Obsidian's file and shape): read
   as a lowercase-name map, written by MERGE — every other name and every other
   top-level key of the file is kept; a file that is not a JSON object is REFUSED,
   never replaced (the app.json rule of main.rs set_app_bool_in).
 * the vault's property census for the value / key autocomplete (and the Properties
   view): every top-level frontmatter key of every indexed note with the number of
   notes holding it and its distinct scalar values, read off the in-memory index.
   This is a read-only summary: nothing here writes a note. The note edits are the
   UI's (ui/props-model.js), one entry's lines at a time.
 * "Properties in document" (app.json "propertiesInDocument": visible | hidden |
   source, absent = visible): the value parsing; main.rs reads and writes the key. */

use std::collections::BTreeMap;
use std::path::Path;

pub const TYPES_REL: &str = ".obsidian/types.json";
/// the types a user can assign (stock's Property type menu)
pub const TYPES: &[&str] = &["text", "multitext", "number", "checkbox", "date", "datetime", "tags", "aliases"];
pub const MODES: &[&str] = &["visible", "hidden", "source"];
/// distinct values kept per key for the suggester
const MAX_VALUES: usize = 200;

/// app.json "propertiesInDocument" -> the mode; anything unknown is the default
pub fn mode_of(v: Option<&serde_json::Value>) -> &'static str {
    let s = v.and_then(|x| x.as_str()).unwrap_or("");
    MODES.iter().copied().find(|m| *m == s).unwrap_or("visible")
}

/// lowercase name -> assigned type (an unparsable file reads as no assignment)
pub fn types_in(root: &Path) -> BTreeMap<String, String> {
    let cur = std::fs::read_to_string(root.join(TYPES_REL)).unwrap_or_default();
    let mut out = BTreeMap::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&cur) {
        if let Some(t) = v.get("types").and_then(|t| t.as_object()) {
            for (k, ty) in t {
                if let Some(s) = ty.as_str() {
                    out.insert(k.to_lowercase(), s.to_string());
                }
            }
        }
    }
    out
}

/// the new body of types.json with `name` assigned `ty` (None = the assignment goes).
/// The existing spelling of the name is replaced case-insensitively; everything else
/// is kept. Err = the file is not a JSON object (refused, never overwritten).
pub fn types_merged(cur: &str, name: &str, ty: Option<&str>) -> Result<String, String> {
    if let Some(t) = ty {
        if !TYPES.contains(&t) {
            return Err(format!("unknown property type {t:?}"));
        }
    }
    let name = name.trim();
    if name.is_empty() {
        return Err("empty property name".into());
    }
    let mut v = if cur.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str::<serde_json::Value>(cur).map_err(|e| format!("types.json is not JSON ({e}) — refusing to overwrite it"))?
    };
    let o = v.as_object_mut().ok_or("types.json is not a JSON object — refusing to overwrite it")?;
    let types = o.entry("types").or_insert_with(|| serde_json::json!({}));
    let t = types.as_object_mut().ok_or("types.json \"types\" is not an object — refusing to overwrite it")?;
    let lc = name.to_lowercase();
    t.retain(|k, _| k.to_lowercase() != lc);
    if let Some(ty) = ty {
        t.insert(name.to_string(), serde_json::Value::String(ty.into()));
    }
    serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
}

#[derive(serde::Serialize, Debug, Default, Clone, PartialEq)]
pub struct PropStat {
    /// the first spelling met (notes in name order)
    pub name: String,
    /// notes holding the key
    pub count: usize,
    /// distinct scalar values (list items count one by one), first-seen order
    pub values: Vec<String>,
}

/// the frontmatter yaml lines of a note (index.rs rule: "---" .. "---" | "...")
fn fm_lines(s: &str) -> Option<Vec<&str>> {
    let mut it = s.split('\n');
    if it.next()?.trim_end() != "---" {
        return None;
    }
    let mut out = Vec::new();
    for l in it {
        let t = l.trim_end_matches('\r');
        if matches!(t.trim_end(), "---" | "...") {
            return Some(out);
        }
        out.push(t);
    }
    None
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\''))) {
        return s[1..s.len() - 1].to_string();
    }
    s.to_string()
}

/// one note's (key, values) pairs: top-level `key:` lines, inline scalars, flow lists,
/// block lists. Nested maps / block scalars contribute the key only.
pub fn note_props(content: &str) -> Vec<(String, Vec<String>)> {
    let Some(lines) = fm_lines(content) else { return Vec::new() };
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for l in lines {
        if l.trim().is_empty() || l.trim_start().starts_with('#') {
            continue;
        }
        if l.starts_with(' ') || l.starts_with('\t') || l.starts_with('-') {
            if let (Some(last), Some(item)) = (out.last_mut(), l.trim_start().strip_prefix("- ")) {
                let it = unquote(item);
                if !it.is_empty() && !it.starts_with(['[', '{', '|', '>', '&', '*']) {
                    last.1.push(it);
                }
            }
            continue;
        }
        let Some(c) = l.find(':') else { continue };
        let key = unquote(&l[..c]);
        if key.is_empty() {
            continue;
        }
        let rest = l[c + 1..].trim();
        let mut vals = Vec::new();
        if let Some(inner) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            for x in inner.split(',') {
                let x = unquote(x);
                if !x.is_empty() {
                    vals.push(x);
                }
            }
        } else if !rest.is_empty() && !rest.starts_with(['|', '>', '&', '*', '{', '#']) {
            vals.push(unquote(rest));
        }
        out.push((key, vals));
    }
    out
}

/// the vault census: lowercase key -> stat, over every note
pub fn census<'a>(contents: impl Iterator<Item = &'a str>) -> Vec<PropStat> {
    let mut m: BTreeMap<String, PropStat> = BTreeMap::new();
    for c in contents {
        let mut seen = std::collections::HashSet::new();
        for (k, vals) in note_props(c) {
            let lc = k.to_lowercase();
            let st = m.entry(lc.clone()).or_insert_with(|| PropStat { name: k.clone(), ..Default::default() });
            if seen.insert(lc) {
                st.count += 1;
            }
            for v in vals {
                if st.values.len() < MAX_VALUES && !st.values.contains(&v) {
                    st.values.push(v);
                }
            }
        }
    }
    m.into_values().collect()
}

/// stock's property search: `[key]` (the note has the key) / `[key:value]` (one of its
/// values contains `value`, case-insensitive). -> (lowercase key, lowercase value). Only a
/// query that is exactly one bracket term qualifies; anything else is a normal search.
pub fn prop_query(q: &str) -> Option<(String, Option<String>)> {
    let inner = q.trim().strip_prefix('[')?.strip_suffix(']')?;
    if inner.contains(['[', ']']) {
        return None;
    }
    let (k, v) = match inner.find(':') {
        Some(i) => (&inner[..i], Some(&inner[i + 1..])),
        None => (inner, None),
    };
    let k = unquote(k).to_lowercase();
    if k.is_empty() {
        return None;
    }
    let v = v.map(|x| unquote(x).to_lowercase()).filter(|x| !x.is_empty());
    Some((k, v))
}

/// the 0-based file line of `key`'s entry when the note matches the property query
pub fn prop_hit(content: &str, key: &str, val: Option<&str>) -> Option<usize> {
    let (_, vals) = note_props(content).into_iter().find(|(k, _)| k.to_lowercase() == key)?;
    if let Some(v) = val {
        if !vals.iter().any(|x| x.to_lowercase().contains(v)) {
            return None;
        }
    }
    let lines = fm_lines(content)?;
    let at = lines.iter().position(|l| {
        !l.starts_with([' ', '\t', '-', '#']) && l.find(':').is_some_and(|c| unquote(&l[..c]).to_lowercase() == key)
    })?;
    Some(at + 1)
}

/// the notes (name order of the index) whose frontmatter holds `key`, any spelling
pub fn notes_with<'a>(docs: impl Iterator<Item = (&'a str, &'a str)>, key: &str) -> Vec<String> {
    let lc = key.to_lowercase();
    docs.filter(|(_, c)| note_props(c).iter().any(|(k, _)| k.to_lowercase() == lc)).map(|(n, _)| n.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_defaults_to_visible() {
        assert_eq!(mode_of(None), "visible");
        assert_eq!(mode_of(Some(&serde_json::json!("hidden"))), "hidden");
        assert_eq!(mode_of(Some(&serde_json::json!("source"))), "source");
        assert_eq!(mode_of(Some(&serde_json::json!("weird"))), "visible");
        assert_eq!(mode_of(Some(&serde_json::json!(true))), "visible");
    }

    #[test]
    fn types_merge_keeps_everything_else_and_refuses_garbage() {
        let cur = "{\"types\":{\"Status\":\"text\",\"due\":\"date\"},\"other\":1}";
        let out = types_merged(cur, "status", Some("multitext")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["other"], 1, "a key we do not own is kept");
        assert_eq!(v["types"]["due"], "date");
        assert_eq!(v["types"]["status"], "multitext");
        assert!(v["types"].get("Status").is_none(), "the old spelling is replaced, not duplicated");
        let out = types_merged(&out, "due", None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v["types"].get("due").is_none());
        assert!(types_merged("not json", "x", Some("text")).is_err());
        assert!(types_merged("[1]", "x", Some("text")).is_err());
        assert!(types_merged("{\"types\":3}", "x", Some("text")).is_err());
        assert!(types_merged("{}", "x", Some("bogus")).is_err());
        assert!(types_merged("{}", "  ", Some("text")).is_err());
        assert_eq!(types_merged("", "a", Some("number")).unwrap(), "{\n  \"types\": {\n    \"a\": \"number\"\n  }\n}");
    }

    #[test]
    fn types_in_reads_lowercase() {
        let d = std::env::temp_dir().join(format!("props-types-{}", std::process::id()));
        std::fs::create_dir_all(d.join(".obsidian")).unwrap();
        std::fs::write(d.join(TYPES_REL), "{\"types\":{\"Due\":\"date\",\"n\":7}}").unwrap();
        let t = types_in(&d);
        assert_eq!(t.get("due").map(String::as_str), Some("date"));
        assert!(t.get("n").is_none(), "a non-string type is not an assignment");
        std::fs::write(d.join(TYPES_REL), "garbage").unwrap();
        assert!(types_in(&d).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn census_counts_notes_and_values() {
        let a = "---\ntitle: A\nstatus: open\ntags:\n  - x\n  - \"y\"\nnested:\n  k: 1\n---\nbody";
        let b = "---\r\nStatus: 'done'\r\nlist: [p, q]\r\n# c\r\n---\r\n";
        let c = "no frontmatter\nstatus: nope";
        let s = census([a, b, c].into_iter());
        let get = |k: &str| s.iter().find(|p| p.name.to_lowercase() == k).cloned().unwrap();
        assert_eq!(get("status").count, 2);
        assert_eq!(get("status").name, "status");
        assert_eq!(get("status").values, vec!["open", "done"]);
        assert_eq!(get("tags").values, vec!["x", "y"]);
        assert_eq!(get("list").values, vec!["p", "q"]);
        assert_eq!(get("nested").values, Vec::<String>::new(), "a nested map's inner lines are not values");
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn property_search_query_and_hits() {
        assert_eq!(prop_query("[status]"), Some(("status".into(), None)));
        assert_eq!(prop_query(" [Status:Done] "), Some(("status".into(), Some("done".into()))));
        assert_eq!(prop_query("[status:]"), Some(("status".into(), None)));
        assert_eq!(prop_query("[\"my key\":x]"), Some(("my key".into(), Some("x".into()))));
        assert_eq!(prop_query("status"), None);
        assert_eq!(prop_query("[]"), None);
        assert_eq!(prop_query("[a] b"), None);
        assert_eq!(prop_query("[[link]]"), None);
        let n = "---\ntitle: x\n# c\nStatus: Done\ntags:\n  - a\n---\nbody status: no\n";
        assert_eq!(prop_hit(n, "status", None), Some(3));
        assert_eq!(prop_hit(n, "status", Some("don")), Some(3));
        assert_eq!(prop_hit(n, "status", Some("open")), None);
        assert_eq!(prop_hit(n, "tags", Some("a")), Some(4));
        assert_eq!(prop_hit(n, "body status", None), None);
        assert_eq!(prop_hit("no frontmatter\nstatus: x\n", "status", None), None);
        let docs = [("A", n), ("B", "---\nother: 1\n---\n"), ("C", "---\nSTATUS: open\n---\n")];
        assert_eq!(notes_with(docs.iter().copied(), "Status"), vec!["A", "C"]);
    }
}
