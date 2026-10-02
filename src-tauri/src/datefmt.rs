// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* goal insdate — "Templates: Insert current date/time" formats the clock with
   a moment.js format string (stock bundles moment 2.29, en locale). No date
   crate (goal criterion 7): this is a port of moment's FORMAT path only —
   expandFormat (long-date tokens), the formattingTokens tokenizer and the en
   token functions — over a broken-down LOCAL time handed in by the webview
   (the only place that knows the local zone without a tz database).

   Tokenizer, faithful to moment's regex (docs/insdate/recon.md REQ-14/15):
     (\[[^\[]*\])|(\\)?([Hh]mm(ss)?|Mo|MM?M?M?|Do|DDDo|DD?D?D?|ddd?d?|do?|
       w[o|w]?|W[o|W]?|Qo?|N{1,5}|YYYYYY|YYYYY|YYYY|YY|y{2,4}|yo?|
       gg(ggg?)?|GG(GGG?)?|e|E|a|A|hh?|HH?|kk?|mm?|ss?|S{1,9}|x|X|zz?|ZZ?|.)
   Note the character class [o|w]: "w|" is ONE token that is not a format
   function, so it prints literally — that is why stock's "w|ww" survey shows
   "w|40" while a lone "w" is the locale week. Unknown tokens print with
   backslashes removed; a [..] prints its contents; a line terminator is
   matched by nothing and so is dropped (JS '.' semantics). */

/// Local wall-clock time + the instant, as the webview's Date sees it.
#[derive(Clone, Copy, Debug, serde::Deserialize)]
pub struct Tm {
    pub y: i64,
    /// 1..=12
    pub mo: u32,
    pub d: u32,
    pub h: u32,
    pub mi: u32,
    pub s: u32,
    pub ms: u32,
    /// minutes EAST of UTC (= -Date.getTimezoneOffset())
    pub off: i32,
    /// Date.now()
    pub epoch_ms: i64,
}

pub const DEFAULT_DATE: &str = "YYYY-MM-DD";
pub const DEFAULT_TIME: &str = "HH:mm";

/// Stock: a missing or "" key falls back to the default, per key (REQ-3).
pub fn render(fmt: Option<&str>, default: &str, t: &Tm) -> String {
    match fmt {
        Some(f) if !f.is_empty() => format(f, t),
        _ => format(default, t),
    }
}

const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October",
    "November", "December",
];
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const DAYS_MIN: [&str; 7] = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];

fn is_lt(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// moment's zeroFill(number, targetLength, forceSign)
fn zf(n: i64, len: usize, force_sign: bool) -> String {
    let a = n.unsigned_abs().to_string();
    let sign = if n >= 0 { if force_sign { "+" } else { "" } } else { "-" };
    format!("{sign}{}{a}", "0".repeat(len.saturating_sub(a.len())))
}

/// en ordinal
fn ord(n: i64) -> String {
    let b = n % 10;
    let suf = if (n % 100) / 10 == 1 {
        "th"
    } else {
        match b {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    };
    format!("{n}{suf}")
}

fn leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// days since 1970-01-01, proleptic Gregorian (Howard Hinnant's days_from_civil)
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// 0 = Sunday
fn weekday(y: i64, m: u32, d: u32) -> i64 {
    (days_from_civil(y, m, d) + 4).rem_euclid(7)
}

fn day_of_year(y: i64, m: u32, d: u32) -> i64 {
    days_from_civil(y, m, d) - days_from_civil(y, 1, 1) + 1
}

/// moment firstWeekOffset(year, dow, doy)
fn first_week_offset(y: i64, dow: i64, doy: i64) -> i64 {
    let fwd = 7 + dow - doy;
    let fwdlw = (7 + weekday(y, 1, fwd as u32) - dow) % 7;
    -fwdlw + fwd - 1
}

fn weeks_in_year(y: i64, dow: i64, doy: i64) -> i64 {
    let days = if leap(y) { 366 } else { 365 };
    (days - first_week_offset(y, dow, doy) + first_week_offset(y + 1, dow, doy)) / 7
}

/// moment weekOfYear -> (week, weekYear)
fn week_of_year(t: &Tm, dow: i64, doy: i64) -> (i64, i64) {
    let off = first_week_offset(t.y, dow, doy);
    let week = (day_of_year(t.y, t.mo, t.d) - off - 1).div_euclid(7) + 1;
    if week < 1 {
        (week + weeks_in_year(t.y - 1, dow, doy), t.y - 1)
    } else if week > weeks_in_year(t.y, dow, doy) {
        (week - weeks_in_year(t.y, dow, doy), t.y + 1)
    } else {
        (week, t.y)
    }
}

/// en: week starts Sunday, the week containing Jan 1 is week 1
fn locale_week(t: &Tm) -> (i64, i64) {
    week_of_year(t, 0, 6)
}
fn iso_week(t: &Tm) -> (i64, i64) {
    week_of_year(t, 1, 4)
}

/// en longDateFormat (+ the derived lowercase forms)
fn long_date(tok: &str) -> Option<&'static str> {
    Some(match tok {
        "LTS" => "h:mm:ss A",
        "LT" => "h:mm A",
        "L" => "MM/DD/YYYY",
        "LL" => "MMMM D, YYYY",
        "LLL" => "MMMM D, YYYY h:mm A",
        "LLLL" => "dddd, MMMM D, YYYY h:mm A",
        "l" => "M/D/YYYY",
        "ll" => "MMM D, YYYY",
        "lll" => "MMM D, YYYY h:mm A",
        "llll" => "ddd, MMM D, YYYY h:mm A",
        _ => return None,
    })
}

/// length of a bracket literal "[...]" starting at i (no '[' inside), if any
fn bracket_at(c: &[char], i: usize) -> Option<usize> {
    if c[i] != '[' {
        return None;
    }
    for (j, &ch) in c.iter().enumerate().skip(i + 1) {
        match ch {
            ']' => return Some(j - i + 1),
            '[' => return None,
            _ => {}
        }
    }
    None
}

fn run_of(c: &[char], i: usize, ch: char, max: usize) -> usize {
    c[i..].iter().take(max).take_while(|&&x| x == ch).count()
}

/// moment localFormattingTokens: LTS|LT|LL?L?L?|l{1,4}
fn long_tok_at(c: &[char], i: usize) -> usize {
    match c[i] {
        'L' if c.get(i + 1) == Some(&'T') => {
            if c.get(i + 2) == Some(&'S') {
                3
            } else {
                2
            }
        }
        'L' => run_of(c, i, 'L', 4),
        'l' => run_of(c, i, 'l', 4),
        _ => 0,
    }
}

/// moment expandFormat: replace long-date tokens until none is left (<= 6 passes)
fn expand(fmt: &str) -> String {
    let mut s = fmt.to_string();
    for _ in 0..6 {
        let c: Vec<char> = s.chars().collect();
        let (mut out, mut i, mut hit) = (String::new(), 0, false);
        while i < c.len() {
            if let Some(n) = bracket_at(&c, i) {
                out.extend(&c[i..i + n]);
                i += n;
                continue;
            }
            if c[i] == '\\' && i + 1 < c.len() && long_tok_at(&c, i + 1) > 0 {
                let n = 1 + long_tok_at(&c, i + 1);
                out.extend(&c[i..i + n]); // escaped: longDateFormat("\\LT") is undefined, kept
                i += n;
                continue;
            }
            let n = long_tok_at(&c, i);
            if n > 0 {
                let tok: String = c[i..i + n].iter().collect();
                out.push_str(long_date(&tok).unwrap_or(&tok));
                hit = true;
                i += n;
                continue;
            }
            out.push(c[i]);
            i += 1;
        }
        s = out;
        if !hit {
            break;
        }
    }
    s
}

/// Length of the formattingTokens alternative (minus the bracket/backslash
/// parts) matching at i, or 0 if nothing matches (a line terminator).
fn tok_at(c: &[char], i: usize) -> usize {
    let at = |k: usize| c.get(i + k).copied();
    let ch = c[i];
    match ch {
        'H' | 'h' if at(1) == Some('m') && at(2) == Some('m') => {
            if at(3) == Some('s') && at(4) == Some('s') {
                5
            } else {
                3
            }
        }
        'M' if at(1) == Some('o') => 2,
        'M' => run_of(c, i, 'M', 4),
        'D' if at(1) == Some('o') => 2,
        'D' if at(1) == Some('D') && at(2) == Some('D') && at(3) == Some('o') => 4,
        'D' => run_of(c, i, 'D', 4),
        'd' if at(1) == Some('d') => run_of(c, i, 'd', 4),
        'd' if at(1) == Some('o') => 2,
        'w' if matches!(at(1), Some('o' | '|' | 'w')) => 2,
        'W' if matches!(at(1), Some('o' | '|' | 'W')) => 2,
        'Q' if at(1) == Some('o') => 2,
        'N' => run_of(c, i, 'N', 5),
        'Y' => match run_of(c, i, 'Y', 6) {
            6 => 6,
            5 => 5,
            4 => 4,
            2 | 3 => 2,
            _ => 1, // '.' — a single Y, which IS a format function
        },
        'y' => match run_of(c, i, 'y', 4) {
            1 if at(1) == Some('o') => 2,
            n => n,
        },
        'g' | 'G' => match run_of(c, i, ch, 5) {
            5 => 5,
            4 => 4,
            2 | 3 => 2,
            _ => 1,
        },
        'h' | 'H' | 'k' | 'm' | 's' | 'z' | 'Z' => run_of(c, i, ch, 2),
        'S' => run_of(c, i, 'S', 9),
        c0 if is_lt(c0) => 0,
        _ => 1,
    }
}

/// moment formatTokenFunctions (en). None = not a format function.
fn token(tok: &str, t: &Tm) -> Option<String> {
    let doy = || day_of_year(t.y, t.mo, t.d);
    let wd = || weekday(t.y, t.mo, t.d);
    let h12 = || -> i64 {
        let h = (t.h % 12) as i64;
        if h == 0 { 12 } else { h }
    };
    let k24 = || if t.h == 0 { 24 } else { t.h as i64 };
    let bc = t.y <= 0;
    let era_year = if bc { 1 - t.y } else { t.y };
    let zone = |sep: &str| {
        let (sign, o) = if t.off < 0 { ('-', -t.off) } else { ('+', t.off) };
        format!("{sign}{}{sep}{}", zf((o / 60) as i64, 2, false), zf((o % 60) as i64, 2, false))
    };
    let ms = t.ms as i64;
    Some(match tok {
        "M" => (t.mo).to_string(),
        "Mo" => ord(t.mo as i64),
        "MM" => zf(t.mo as i64, 2, false),
        "MMM" => MONTHS[(t.mo - 1) as usize][..3].to_string(),
        "MMMM" => MONTHS[(t.mo - 1) as usize].to_string(),
        "D" => t.d.to_string(),
        "Do" => ord(t.d as i64),
        "DD" => zf(t.d as i64, 2, false),
        "DDD" => doy().to_string(),
        "DDDo" => ord(doy()),
        "DDDD" => zf(doy(), 3, false),
        "d" | "e" => wd().to_string(), // en dow = 0: locale weekday == day()
        "do" => ord(wd()),
        "dd" => DAYS_MIN[wd() as usize].to_string(),
        "ddd" => DAYS[wd() as usize][..3].to_string(),
        "dddd" => DAYS[wd() as usize].to_string(),
        "E" => (if wd() == 0 { 7 } else { wd() }).to_string(),
        "w" => locale_week(t).0.to_string(),
        "wo" => ord(locale_week(t).0),
        "ww" => zf(locale_week(t).0, 2, false),
        "W" => iso_week(t).0.to_string(),
        "Wo" => ord(iso_week(t).0),
        "WW" => zf(iso_week(t).0, 2, false),
        "Q" => t.mo.div_ceil(3).to_string(),
        "Qo" => ord(t.mo.div_ceil(3) as i64),
        "N" | "NN" | "NNN" | "NNNNN" => (if bc { "BC" } else { "AD" }).to_string(),
        "NNNN" => (if bc { "Before Christ" } else { "Anno Domini" }).to_string(),
        "y" => zf(era_year, 1, false),
        "yo" => ord(era_year),
        "yy" => zf(era_year, 2, false),
        "yyy" => zf(era_year, 3, false),
        "yyyy" => zf(era_year, 4, false),
        "Y" => {
            if t.y <= 9999 {
                zf(t.y, 4, false)
            } else {
                format!("+{}", t.y)
            }
        }
        "YY" => zf(t.y % 100, 2, false),
        "YYYY" => zf(t.y, 4, false),
        "YYYYY" => zf(t.y, 5, false),
        "YYYYYY" => zf(t.y, 6, true),
        "gg" => zf(locale_week(t).1 % 100, 2, false),
        "gggg" => zf(locale_week(t).1, 4, false),
        "ggggg" => zf(locale_week(t).1, 5, false),
        "GG" => zf(iso_week(t).1 % 100, 2, false),
        "GGGG" => zf(iso_week(t).1, 4, false),
        "GGGGG" => zf(iso_week(t).1, 5, false),
        "a" => (if t.h > 11 { "pm" } else { "am" }).to_string(),
        "A" => (if t.h > 11 { "PM" } else { "AM" }).to_string(),
        "H" => t.h.to_string(),
        "HH" => zf(t.h as i64, 2, false),
        "h" => h12().to_string(),
        "hh" => zf(h12(), 2, false),
        "k" => k24().to_string(),
        "kk" => zf(k24(), 2, false),
        "hmm" => format!("{}{}", h12(), zf(t.mi as i64, 2, false)),
        "hmmss" => format!("{}{}{}", h12(), zf(t.mi as i64, 2, false), zf(t.s as i64, 2, false)),
        "Hmm" => format!("{}{}", t.h, zf(t.mi as i64, 2, false)),
        "Hmmss" => format!("{}{}{}", t.h, zf(t.mi as i64, 2, false), zf(t.s as i64, 2, false)),
        "m" => t.mi.to_string(),
        "mm" => zf(t.mi as i64, 2, false),
        "s" => t.s.to_string(),
        "ss" => zf(t.s as i64, 2, false),
        "S" => (ms / 100).to_string(),
        "SS" => zf(ms / 10, 2, false),
        "SSS" => zf(ms, 3, false),
        s if s.len() >= 4 && s.len() <= 9 && s.bytes().all(|b| b == b'S') => {
            zf(ms * 10i64.pow(s.len() as u32 - 3), s.len(), false)
        }
        "z" | "zz" => String::new(), // local (not UTC) moment: zoneAbbr() is ""
        "Z" => zone(":"),
        "ZZ" => zone(""),
        "X" => t.epoch_ms.div_euclid(1000).to_string(),
        "x" => t.epoch_ms.to_string(),
        _ => return None,
    })
}

/// moment(..).format(fmt), en locale. "" formats to "" (the default is the caller's: `render`).
pub fn format(fmt: &str, t: &Tm) -> String {
    let c: Vec<char> = expand(fmt).chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if let Some(n) = bracket_at(&c, i) {
            out.extend(&c[i + 1..i + n - 1]);
            i += n;
            continue;
        }
        if c[i] == '\\' && i + 1 < c.len() {
            let n = tok_at(&c, i + 1);
            if n > 0 {
                // "\\" + token: never a format function; printed without backslashes
                out.extend(c[i + 1..i + 1 + n].iter().filter(|&&x| x != '\\'));
                i += 1 + n;
                continue;
            }
        }
        let n = tok_at(&c, i);
        if n == 0 {
            i += 1; // line terminator: no alternative matches it, String.match skips it
            continue;
        }
        let tok: String = c[i..i + n].iter().collect();
        match token(&tok, t) {
            Some(v) => out.push_str(&v),
            None => out.extend(tok.chars().filter(|&x| x != '\\')),
        }
        i += n;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a UTC wall-clock time (off = 0), epoch derived from the civil date
    fn utc(y: i64, mo: u32, d: u32, h: u32, mi: u32, s: u32, ms: u32) -> Tm {
        let epoch_ms = days_from_civil(y, mo, d) * 86_400_000
            + (h as i64 * 3600 + mi as i64 * 60 + s as i64) * 1000
            + ms as i64;
        Tm { y, mo, d, h, mi, s, ms, off: 0, epoch_ms }
    }

    /// The survey run on stock 1.13.7 (docs/insdate/stock/formats.log, RUN
    /// survey c01): box clock Fri 2026-10-02 08:19:03.502 UTC. Byte-for-byte.
    #[test]
    fn stock_survey_44_tokens() {
        let t = utc(2026, 10, 2, 8, 19, 3, 502);
        assert_eq!(t.epoch_ms, 1_790_929_143_502, "civil -> epoch agrees with stock's x");
        let f = "YYYY|YY|M|MM|MMM|MMMM|D|DD|Do|DDD|DDDD|d|dd|ddd|dddd|E|e|w|ww|W|WW|Q|H|HH|h|hh|k|kk|m|mm|s|ss|S|SSS|A|a|X|x|Z|ZZ|gggg|GGGG|[lit]|qbfj|LT|L|LL";
        assert_eq!(
            format(f, &t),
            "2026|26|10|10|Oct|October|2|02|2nd|275|275|5|Fr|Fri|Friday|5|5|w|40|W|40|4|8|08|8|08|8|08|19|19|3|03|5|502|AM|am|1790929143|1790929143502|+00:00|+0000|2026|2026|lit|qbfj|8:19 AM|10/02/2026|October 2, 2026"
        );
        // RUN survey c04 timeFormat, 08:19:18
        let t = utc(2026, 10, 2, 8, 19, 18, 0);
        assert_eq!(format("h:mm:ss a [o'clock] qbfj", &t), "8:19:18 am o'clock qbfj");
    }

    /// RUN survey2 (formats.log): the tokens beyond the first 44, stock clock
    /// 08:29:13.484 for the date and 08:29:28.605 (x = 1790929768605) for the time.
    #[test]
    fn stock_survey2_more_tokens_and_escapes() {
        let t = utc(2026, 10, 2, 8, 29, 13, 484);
        let f = "w W Mo DDDo do wo Wo Qo N NN NNN NNNN NNNNN y yo yy yyyy Y YYYYY YYYYYY gg ggggg GG GGGGG hmm hmmss Hmm Hmmss SS SSSS SSSSSSSSS z zz LTS LLL LLLL l ll lll llll ggg";
        assert_eq!(
            format(f, &t),
            "40 40 10th 275th 5th 40th 40th 4th AD AD AD Anno Domini AD 2026 2026th 2026 2026 2026 02026 +002026 26 02026 26 02026 829 82913 829 82913 48 4840 484000000   8:29:13 AM October 2, 2026 8:29 AM Friday, October 2, 2026 8:29 AM 10/2/2026 Oct 2, 2026 Oct 2, 2026 8:29 AM Fri, Oct 2, 2026 8:29 AM 26g"
        );
        let t = utc(2026, 10, 2, 8, 29, 28, 605);
        assert_eq!(t.epoch_ms, 1_790_929_768_605);
        assert_eq!(format("\\Y\\[x] [a[b] w| \\", &t), "Y[1790929768605] [amb w| ");
    }

    /// One assertion per supported token, at a time that makes padding,
    /// 12/24h and ordinals visible (Tue 2026-03-03 13:05:07.009, +05:30).
    #[test]
    fn each_token() {
        let mut t = utc(2026, 3, 3, 13, 5, 7, 9);
        t.off = 330;
        let cases: &[(&str, &str)] = &[
            ("M", "3"), ("Mo", "3rd"), ("MM", "03"), ("MMM", "Mar"), ("MMMM", "March"),
            ("D", "3"), ("Do", "3rd"), ("DD", "03"),
            ("DDD", "62"), ("DDDo", "62nd"), ("DDDD", "062"),
            ("d", "2"), ("do", "2nd"), ("dd", "Tu"), ("ddd", "Tue"), ("dddd", "Tuesday"),
            ("e", "2"), ("E", "2"),
            ("w", "10"), ("wo", "10th"), ("ww", "10"),
            ("W", "10"), ("Wo", "10th"), ("WW", "10"),
            ("Q", "1"), ("Qo", "1st"),
            ("N", "AD"), ("NN", "AD"), ("NNN", "AD"), ("NNNN", "Anno Domini"), ("NNNNN", "AD"),
            ("y", "2026"), ("yo", "2026th"), ("yy", "2026"), ("yyy", "2026"), ("yyyy", "2026"),
            ("Y", "2026"), ("YY", "26"), ("YYYY", "2026"), ("YYYYY", "02026"), ("YYYYYY", "+002026"),
            ("gg", "26"), ("gggg", "2026"), ("ggggg", "02026"),
            ("GG", "26"), ("GGGG", "2026"), ("GGGGG", "02026"),
            ("a", "pm"), ("A", "PM"),
            ("H", "13"), ("HH", "13"), ("h", "1"), ("hh", "01"), ("k", "13"), ("kk", "13"),
            ("hmm", "105"), ("hmmss", "10507"), ("Hmm", "1305"), ("Hmmss", "130507"),
            ("m", "5"), ("mm", "05"), ("s", "7"), ("ss", "07"),
            ("S", "0"), ("SS", "00"), ("SSS", "009"), ("SSSS", "0090"), ("SSSSS", "00900"),
            ("SSSSSS", "009000"), ("SSSSSSS", "0090000"), ("SSSSSSSS", "00900000"), ("SSSSSSSSS", "009000000"),
            ("z", ""), ("zz", ""), ("Z", "+05:30"), ("ZZ", "+0530"),
            ("LTS", "1:05:07 PM"), ("LT", "1:05 PM"), ("L", "03/03/2026"), ("LL", "March 3, 2026"),
            ("LLL", "March 3, 2026 1:05 PM"), ("LLLL", "Tuesday, March 3, 2026 1:05 PM"),
            ("l", "3/3/2026"), ("ll", "Mar 3, 2026"), ("lll", "Mar 3, 2026 1:05 PM"),
            ("llll", "Tue, Mar 3, 2026 1:05 PM"),
        ];
        for (f, want) in cases {
            assert_eq!(format(f, &t), *want, "token {f}");
        }
        // X / x are the instant, independent of the wall clock fields
        t.epoch_ms = 1_790_929_143_502;
        assert_eq!(format("X", &t), "1790929143");
        assert_eq!(format("x", &t), "1790929143502");
        t.epoch_ms = -1500; // floor, like Math.floor
        assert_eq!(format("X", &t), "-2");
    }

    #[test]
    fn clock_edges_and_negative_zone() {
        let mut t = utc(2026, 10, 2, 0, 0, 0, 0);
        assert_eq!(format("h hh k kk a A H", &t), "12 12 24 24 am AM 0");
        t.h = 12;
        assert_eq!(format("h k a", &t), "12 12 pm");
        t.off = -210;
        assert_eq!(format("Z ZZ", &t), "-03:30 -0330");
        t.off = -60;
        assert_eq!(format("Z", &t), "-01:00");
    }

    #[test]
    fn ordinals() {
        for (n, want) in [(1, "1st"), (2, "2nd"), (3, "3rd"), (4, "4th"), (11, "11th"), (12, "12th"),
            (13, "13th"), (21, "21st"), (22, "22nd"), (23, "23rd"), (101, "101st"), (111, "111th"), (112, "112th")]
        {
            assert_eq!(ord(n), want);
        }
    }

    /// week-year boundaries: 2026 starts on a Thursday (53 ISO weeks)
    #[test]
    fn week_years() {
        // Thu 2026-12-31: ISO W53 of 2026; en week 1 of 2027 (its week holds Jan 1)
        let t = utc(2026, 12, 31, 9, 0, 0, 0);
        assert_eq!(format("W GGGG w gggg", &t), "53 2026 1 2027");
        // Fri 2027-01-01: still ISO W53 of 2026
        let t = utc(2027, 1, 1, 9, 0, 0, 0);
        assert_eq!(format("W GGGG w gggg E e", &t), "53 2026 1 2027 5 5");
        // Mon 2027-01-04: ISO W1 2027
        let t = utc(2027, 1, 4, 9, 0, 0, 0);
        assert_eq!(format("W GGGG", &t), "1 2027");
        // Sun 2026-03-01: ISO weekday 7, locale weekday 0, en week starts that day
        let t = utc(2026, 3, 1, 9, 0, 0, 0);
        assert_eq!(format("E e d W w", &t), "7 0 0 9 10");
        // leap year day-of-year
        let t = utc(2028, 12, 31, 9, 0, 0, 0);
        assert_eq!(format("DDD DDDD", &t), "366 366");
        let t = utc(2026, 1, 1, 9, 0, 0, 0);
        assert_eq!(format("DDDD", &t), "001");
    }

    #[test]
    fn unknown_tokens_pass_through() {
        let t = utc(2026, 10, 2, 8, 19, 3, 502);
        // letters that are no token, punctuation, non-ASCII, and the "w|" / "W|" pseudo-tokens
        assert_eq!(format("qbfj", &t), "qbfj");
        assert_eq!(format("B C F J O P R T U V b c f i j n o p q r t u v", &t),
            "B C F J O P R T U V b c f i j n o p q r t u v");
        assert_eq!(format("-/:.,;()|_ äöü 日付 🙂", &t), "-/:.,;()|_ äöü 日付 🙂");
        assert_eq!(format("w|W|", &t), "w|W|");
        assert_eq!(format("g ggg", &t), "g 26g");
    }

    #[test]
    fn escapes() {
        let t = utc(2026, 10, 2, 8, 19, 3, 502);
        assert_eq!(format("[YYYY]", &t), "YYYY");
        assert_eq!(format("[]", &t), "");
        assert_eq!(format("[today is] dddd", &t), "today is Friday");
        assert_eq!(format("[LT] [l]", &t), "LT l", "long-date tokens inside [] are not expanded");
        assert_eq!(format("\\LT \\l", &t), "LT l");
        assert_eq!(format("\\YYYY", &t), "YYYY", "the backslash takes the whole token");
        assert_eq!(format("[a[b]", &t), "[amb", "an unclosed [ is a literal");
        assert_eq!(format("YYYY[", &t), "2026[");
        assert_eq!(format("\\", &t), "", "a trailing backslash prints nothing");
        assert_eq!(format("a\nb", &t), "amb", "a line terminator matches no token and is dropped");
        assert_eq!(format("[x\ny]", &t), "x\ny", "but survives inside []");
    }

    /// Stock (formats.log RUN empty / partial): a missing or "" key formats
    /// with the default — per key. format("") itself is "".
    #[test]
    fn empty_format_uses_default() {
        let t = utc(2026, 10, 2, 8, 20, 21, 0);
        assert_eq!(format("", &t), "");
        assert_eq!(render(Some(""), DEFAULT_DATE, &t), "2026-10-02");
        assert_eq!(render(None, DEFAULT_DATE, &t), "2026-10-02");
        assert_eq!(render(Some(""), DEFAULT_TIME, &t), "08:20");
        assert_eq!(render(None, DEFAULT_TIME, &t), "08:20");
        assert_eq!(render(Some("DD/MM/YYYY"), DEFAULT_DATE, &t), "02/10/2026");
        assert_eq!(render(Some("dddd, MMMM Do YYYY"), DEFAULT_DATE, &t), "Friday, October 2nd 2026");
    }

    #[test]
    fn years_outside_four_digits() {
        let t = utc(5, 6, 7, 9, 0, 0, 0);
        assert_eq!(format("Y YY YYYY YYYYYY y N", &t), "0005 05 0005 +000005 5 AD");
        let t = utc(12345, 6, 7, 9, 0, 0, 0);
        assert_eq!(format("Y YYYY", &t), "+12345 12345");
        let t = utc(0, 6, 7, 9, 0, 0, 0);
        assert_eq!(format("y N NNNN YYYY", &t), "1 BC Before Christ 0000");
    }
}
