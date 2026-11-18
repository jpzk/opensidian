// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Minimal diff-match-patch for the markdown three-way merge (spec §4.7).
//!
//! Written here instead of pulling a crate (deps.md: no diff crate in the
//! lock). Same model as Google's diff-match-patch: a diff base→theirs is
//! turned into context patches, which are applied one by one to "ours".
//! Differences, all on the side of caution (a failed patch becomes a
//! conflict file, never a garbled note):
//!
//! - diff = Myers O(ND) in line mode, then char-level refinement of each
//!   replaced block; a diff over the D cap is refused (→ conflict file).
//! - patch context starts at 4 chars and grows by 4 until the pattern is
//!   unique in the text it was made from (cap 64 per side); hunk contexts
//!   are taken from the progressively patched text, as in DMP.
//! - apply is EXACT match only (no bitap fuzz): the hunk's old text
//!   (context + deleted chars) must occur in ours; with several
//!   occurrences the nearest to the expected location wins only if it is
//!   strictly nearer than the next one and within [`MATCH_DISTANCE`].

use std::collections::HashMap;

/// Context chars added on each side of a change, initial value.
pub const MARGIN: usize = 4;
/// Context growth cap per side.
pub const MAX_CONTEXT: usize = 64;
/// Max distance (chars) of an ambiguous match from its expected location.
pub const MATCH_DISTANCE: usize = 1000;
/// Max edit distance D the Myers search explores per level.
pub const MAX_D: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Equal,
    Delete,
    Insert,
}

/// A diff run over chars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub op: Op,
    pub text: Vec<char>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// Start of `old` in the text the hunk was made from (with every
    /// earlier hunk applied).
    pub loc: usize,
    /// Context + deleted chars + context.
    pub old: Vec<char>,
    /// Context + inserted chars + context.
    pub new: Vec<char>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmpError {
    /// The diff needs more than [`MAX_D`] edits at one level.
    TooComplex,
    /// Hunk `n` did not match exactly / unambiguously.
    PatchFailed(usize),
}

/// Myers diff; `None` if D would exceed `max_d`. Returns runs of
/// (op, len) in a/b order.
fn myers<T: Eq>(a: &[T], b: &[T], max_d: usize) -> Option<Vec<(Op, usize)>> {
    let n = a.len() as isize;
    let m = b.len() as isize;
    let dlim = (a.len() + b.len()).min(max_d);
    let off = dlim as isize + 1;
    let mut v = vec![0isize; 2 * dlim + 3];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;
    'outer: for d in 0..=dlim as isize {
        // keep only diagonals -d..=d (all the backtrack reads): O(D²) total
        trace.push(v[(off - d) as usize..=(off + d) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let i = (k + off) as usize;
            let mut x = if k == -d || (k != d && v[i - 1] < v[i + 1]) { v[i + 1] } else { v[i - 1] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[i] = x;
            if x >= n && y >= m {
                found = Some(d);
                break 'outer;
            }
            k += 2;
        }
    }
    let dend = found?;
    let mut ops: Vec<Op> = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..=dend).rev() {
        let tv = &trace[d as usize];
        let at = |kk: isize| tv[(kk + d) as usize];
        let k = x - y;
        let (prev_x, prev_y) = if d == 0 {
            (0, 0)
        } else {
            let prev_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) { k + 1 } else { k - 1 };
            let px = at(prev_k);
            (px, px - prev_k)
        };
        while x > prev_x && y > prev_y {
            ops.push(Op::Equal);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if x == prev_x {
                ops.push(Op::Insert);
                y -= 1;
            } else {
                ops.push(Op::Delete);
                x -= 1;
            }
        }
    }
    ops.reverse();
    let mut runs: Vec<(Op, usize)> = Vec::new();
    for o in ops {
        match runs.last_mut() {
            Some((p, l)) if *p == o => *l += 1,
            _ => runs.push((o, 1)),
        }
    }
    Some(runs)
}

fn push(out: &mut Vec<Run>, op: Op, text: &[char]) {
    if text.is_empty() {
        return;
    }
    match out.last_mut() {
        Some(r) if r.op == op => r.text.extend_from_slice(text),
        _ => out.push(Run { op, text: text.to_vec() }),
    }
}

/// Char diff with common prefix/suffix trimmed first.
fn char_diff(a: &[char], b: &[char], out: &mut Vec<Run>) -> Result<(), DmpError> {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let (a2, b2) = (&a[pre..], &b[pre..]);
    let suf = a2.iter().rev().zip(b2.iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a2[..a2.len() - suf], &b2[..b2.len() - suf]);
    push(out, Op::Equal, &a[..pre]);
    let runs = myers(am, bm, MAX_D).ok_or(DmpError::TooComplex)?;
    let (mut i, mut j) = (0, 0);
    for (op, l) in runs {
        match op {
            Op::Equal => {
                push(out, Op::Equal, &am[i..i + l]);
                i += l;
                j += l;
            }
            Op::Delete => {
                push(out, Op::Delete, &am[i..i + l]);
                i += l;
            }
            Op::Insert => {
                push(out, Op::Insert, &bm[j..j + l]);
                j += l;
            }
        }
    }
    push(out, Op::Equal, &a2[a2.len() - suf..]);
    Ok(())
}

fn lines(t: &[char]) -> Vec<&[char]> {
    let mut v = Vec::new();
    let mut s = 0;
    for (i, c) in t.iter().enumerate() {
        if *c == '\n' {
            v.push(&t[s..=i]);
            s = i + 1;
        }
    }
    if s < t.len() {
        v.push(&t[s..]);
    }
    v
}

fn intern<'a>(ids: &mut HashMap<&'a [char], u32>, ls: &[&'a [char]]) -> Vec<u32> {
    let mut v = Vec::with_capacity(ls.len());
    for l in ls {
        let n = ids.len() as u32;
        v.push(*ids.entry(*l).or_insert(n));
    }
    v
}

/// Diff `a` → `b`: line-mode Myers, then char refinement of replaced
/// blocks. Equal runs reproduce `a`, Equal + Insert reproduce `b`.
pub fn diff(a: &[char], b: &[char]) -> Result<Vec<Run>, DmpError> {
    let la = lines(a);
    let lb = lines(b);
    let mut ids: HashMap<&[char], u32> = HashMap::new();
    let ia = intern(&mut ids, &la);
    let ib = intern(&mut ids, &lb);
    let runs = myers(&ia, &ib, MAX_D).ok_or(DmpError::TooComplex)?;
    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let (mut del, mut ins): (Vec<char>, Vec<char>) = (Vec::new(), Vec::new());
    let flush = |del: &mut Vec<char>, ins: &mut Vec<char>, out: &mut Vec<Run>| -> Result<(), DmpError> {
        if !del.is_empty() && !ins.is_empty() {
            char_diff(del, ins, out)?;
        } else {
            push(out, Op::Delete, del);
            push(out, Op::Insert, ins);
        }
        del.clear();
        ins.clear();
        Ok(())
    };
    for (op, l) in runs {
        match op {
            Op::Equal => {
                flush(&mut del, &mut ins, &mut out)?;
                for x in &la[i..i + l] {
                    push(&mut out, Op::Equal, x);
                }
                i += l;
                j += l;
            }
            Op::Delete => {
                for x in &la[i..i + l] {
                    del.extend_from_slice(x);
                }
                i += l;
            }
            Op::Insert => {
                for x in &lb[j..j + l] {
                    ins.extend_from_slice(x);
                }
                j += l;
            }
        }
    }
    flush(&mut del, &mut ins, &mut out)?;
    Ok(out)
}

/// Occurrences of `pat` in `t` (at most `cap`).
fn find_all(t: &[char], pat: &[char], cap: usize) -> Vec<usize> {
    let mut v = Vec::new();
    if pat.is_empty() || pat.len() > t.len() {
        return v;
    }
    for (i, w) in t.windows(pat.len()).enumerate() {
        if w[0] == pat[0] && w == pat {
            v.push(i);
            if v.len() >= cap {
                break;
            }
        }
    }
    v
}

/// Change regions of a diff in `a` coordinates: (start, end, inserted).
fn regions(runs: &[Run]) -> Vec<(usize, usize, Vec<char>)> {
    let mut out: Vec<(usize, usize, Vec<char>)> = Vec::new();
    let mut pos = 0usize;
    let mut cur: Option<(usize, usize, Vec<char>)> = None;
    for r in runs {
        match r.op {
            Op::Equal => {
                // DMP: changes closer than 2 * MARGIN share one patch
                if r.text.len() < 2 * MARGIN {
                    if let Some(c) = cur.as_mut() {
                        c.1 += r.text.len();
                        c.2.extend_from_slice(&r.text);
                        pos += r.text.len();
                        continue;
                    }
                }
                if let Some(c) = cur.take() {
                    out.push(c);
                }
                pos += r.text.len();
            }
            Op::Delete => {
                let c = cur.get_or_insert((pos, pos, Vec::new()));
                c.1 += r.text.len();
                pos += r.text.len();
            }
            Op::Insert => {
                cur.get_or_insert((pos, pos, Vec::new())).2.extend_from_slice(&r.text);
            }
        }
    }
    if let Some(c) = cur {
        out.push(c);
    }
    // a trailing short Equal swallowed into a region would be a no-op
    // tail; it is harmless (deleted and re-inserted identically)
    out
}

/// Context patches turning `a` into `b` (given `runs` = diff(a, b)).
pub fn make_patches(a: &[char], runs: &[Run]) -> Vec<Hunk> {
    let mut t = a.to_vec();
    let mut delta: isize = 0;
    let mut out = Vec::new();
    for (cs, ce, ins) in regions(runs) {
        let s = (cs as isize + delta) as usize;
        let e = (ce as isize + delta) as usize;
        let mut m = MARGIN;
        let (pre, post) = loop {
            let pre = m.min(s);
            let post = m.min(t.len() - e);
            let pat = &t[s - pre..e + post];
            let whole = pre == s && post == t.len() - e;
            if whole || m >= MAX_CONTEXT || find_all(&t, pat, 2).len() == 1 {
                break (pre, post);
            }
            m += MARGIN;
        };
        let old = t[s - pre..e + post].to_vec();
        let mut new = t[s - pre..s].to_vec();
        new.extend_from_slice(&ins);
        new.extend_from_slice(&t[e..e + post]);
        out.push(Hunk { loc: s - pre, old, new });
        t.splice(s..e, ins.iter().copied());
        delta += ins.len() as isize - (ce - cs) as isize;
    }
    out
}

/// Apply hunks in order to `ours`. Exact, unambiguous matches only.
pub fn apply_patches(ours: &[char], hunks: &[Hunk]) -> Result<Vec<char>, DmpError> {
    let mut t = ours.to_vec();
    let mut delta: isize = 0;
    for (n, h) in hunks.iter().enumerate() {
        if h.old.is_empty() {
            return Err(DmpError::PatchFailed(n));
        }
        let expected = (h.loc as isize + delta).max(0) as usize;
        let mut occ = find_all(&t, &h.old, usize::MAX);
        occ.sort_by_key(|&p| p.abs_diff(expected));
        let pos = match occ.as_slice() {
            [] => return Err(DmpError::PatchFailed(n)),
            [p] => *p,
            [p, q, ..] => {
                if p.abs_diff(expected) < q.abs_diff(expected) && p.abs_diff(expected) <= MATCH_DISTANCE {
                    *p
                } else {
                    return Err(DmpError::PatchFailed(n));
                }
            }
        };
        t.splice(pos..pos + h.old.len(), h.new.iter().copied());
        delta = pos as isize - h.loc as isize;
    }
    Ok(t)
}

/// Three-way text merge: apply diff(base, theirs) onto `ours`.
pub fn merge3(base: &str, ours: &str, theirs: &str) -> Result<String, DmpError> {
    let b: Vec<char> = base.chars().collect();
    let o: Vec<char> = ours.chars().collect();
    let t: Vec<char> = theirs.chars().collect();
    let runs = diff(&b, &t)?;
    let hunks = make_patches(&b, &runs);
    Ok(apply_patches(&o, &hunks)?.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn rebuild(runs: &[Run]) -> (String, String) {
        let mut a = String::new();
        let mut b = String::new();
        for r in runs {
            let s: String = r.text.iter().collect();
            match r.op {
                Op::Equal => {
                    a += &s;
                    b += &s;
                }
                Op::Delete => a += &s,
                Op::Insert => b += &s,
            }
        }
        (a, b)
    }

    #[test]
    fn diff_reproduces_both_sides() {
        let cases = [
            ("", ""),
            ("", "abc"),
            ("abc", ""),
            ("abc", "abc"),
            ("a\nb\nc\n", "a\nB\nc\n"),
            ("one two three\nfour\n", "one 2 three\nfour\nfive"),
            ("héllo wörld ✓\n", "hello wörld ✗\n"),
            ("x\ny\nz", "z\ny\nx"),
        ];
        for (a, b) in cases {
            let r = diff(&c(a), &c(b)).unwrap();
            assert_eq!(rebuild(&r), (a.to_string(), b.to_string()), "{a:?} -> {b:?}");
        }
    }

    #[test]
    fn patches_apply_to_their_own_base() {
        let a = "# Title\n\nalpha beta gamma\n\ndelta\nepsilon zeta\n";
        let b = "# Title!\n\nalpha BETA gamma\n\ndelta\nepsilon zeta eta\n";
        let r = diff(&c(a), &c(b)).unwrap();
        let h = make_patches(&c(a), &r);
        assert!(h.len() >= 2);
        let got: String = apply_patches(&c(a), &h).unwrap().into_iter().collect();
        assert_eq!(got, b);
    }

    #[test]
    fn disjoint_edits_merge() {
        let base = "line one\nline two\nline three\nline four\nline five\n";
        let ours = "line ONE\nline two\nline three\nline four\nline five\n";
        let theirs = "line one\nline two\nline three\nline four\nline FIVE\n";
        assert_eq!(
            merge3(base, ours, theirs).unwrap(),
            "line ONE\nline two\nline three\nline four\nline FIVE\n"
        );
        // ours shifted by a long insert at the top: still exact + unique
        let ours2 = format!("{}\n{}", "intro ".repeat(400), base);
        let got = merge3(base, &ours2, theirs).unwrap();
        assert!(got.ends_with("line FIVE\n") && got.starts_with("intro "));
    }

    #[test]
    fn overlapping_edits_fail() {
        let base = "the quick brown fox\n";
        let ours = "the quick red fox\n";
        let theirs = "the quick blue fox\n";
        assert!(matches!(merge3(base, ours, theirs), Err(DmpError::PatchFailed(0))));
    }

    #[test]
    fn ambiguous_far_match_fails() {
        // the hunk's pattern is unique in base but appears twice in ours,
        // both far from the expected location → refuse
        let base = "aaaa XYZ bbbb\n";
        let theirs = "aaaa XQZ bbbb\n";
        let pad = "-".repeat(3000);
        let ours = format!("{pad}\naaaa XYZ bbbb\n{pad}\naaaa XYZ bbbb\n");
        assert!(merge3(base, &ours, theirs).is_err());
    }

    #[test]
    fn myers_cap_is_enforced() {
        let a: Vec<u32> = (0..10).collect();
        let b: Vec<u32> = (100..110).collect();
        assert!(myers(&a, &b, 5).is_none());
        assert!(myers(&a, &b, 20).is_some());
    }
}
