// SPDX-License-Identifier: GPL-3.0-or-later
//! #37 C3-7 (T3): the Live Preview table menu — Row / Column / Sort / Align —
//! as pure text transforms on a GFM pipe table's source.
//!
//! The UI (ui/edmenu.js) hands over the table's source text plus the cell the
//! right-click hit (row 0 = the header, 1.. = body rows; the delimiter row is
//! not a row) and gets the whole table back, RE-PADDED the way stock 1.13.7
//! writes it (goal recon37 logs/stock-actions.txt):
//!   * every column is as wide as its widest cell, at least 3;
//!   * cells are padded by their column's alignment (left / center / right;
//!     none = left), the delimiter is `:----` / `:-:` / `----:` / `-----`;
//!   * every row is `| a | b |`, leading pipe, trailing pipe, one space inside.
//! A `\|` stays inside its cell, a `|` inside `code` is not a separator.

const MIN_W: usize = 3;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Al {
    None,
    Left,
    Center,
    Right,
}

struct Table {
    rows: Vec<Vec<String>>, // rows[0] = header
    al: Vec<Al>,
}

/// one table line -> its trimmed cell texts (stock's split: escaped and code pipes stay)
fn cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut code = false;
    let mut it = t.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\\' => {
                cur.push(c);
                if let Some(n) = it.next() {
                    cur.push(n);
                }
            }
            '`' => {
                code = !code;
                cur.push(c);
            }
            '|' if !code => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn parse(src: &str) -> Result<Table, String> {
    let lines: Vec<&str> = src.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 2 {
        return Err("not a table: no delimiter row".into());
    }
    let hdr = cells(lines[0]);
    let dl = cells(lines[1]);
    if hdr.is_empty() || hdr.len() != dl.len() {
        return Err("not a table: header and delimiter differ".into());
    }
    let mut al = Vec::new();
    for d in &dl {
        if d.is_empty() || !d.trim_matches(':').chars().all(|c| c == '-') || !d.contains('-') {
            return Err(format!("not a table: bad delimiter cell {d:?}"));
        }
        let (l, r) = (d.starts_with(':'), d.ends_with(':'));
        al.push(match (l, r) {
            (true, true) => Al::Center,
            (false, true) => Al::Right,
            (true, false) => Al::Left,
            _ => Al::None,
        });
    }
    let n = hdr.len();
    let mut rows = vec![hdr];
    for l in &lines[2..] {
        let mut c = cells(l);
        c.resize(n, String::new()); // GFM: missing cells are empty, extra ones are dropped
        rows.push(c);
    }
    Ok(Table { rows, al })
}

fn width(s: &str) -> usize {
    s.chars().count()
}

fn pad(s: &str, w: usize, a: Al) -> String {
    let k = w.saturating_sub(width(s));
    match a {
        Al::Right => format!("{}{}", " ".repeat(k), s),
        Al::Center => format!("{}{}{}", " ".repeat(k / 2), s, " ".repeat(k - k / 2)),
        _ => format!("{}{}", s, " ".repeat(k)),
    }
}

fn delim(w: usize, a: Al) -> String {
    match a {
        Al::Left => format!(":{}", "-".repeat(w - 1)),
        Al::Right => format!("{}:", "-".repeat(w - 1)),
        Al::Center => format!(":{}:", "-".repeat(w - 2)),
        Al::None => "-".repeat(w),
    }
}

fn render(t: &Table) -> String {
    let n = t.al.len();
    let w: Vec<usize> = (0..n)
        .map(|c| t.rows.iter().map(|r| width(&r[c])).max().unwrap_or(0).max(MIN_W))
        .collect();
    let line = |r: &Vec<String>| {
        let v: Vec<String> = (0..n).map(|c| pad(&r[c], w[c], t.al[c])).collect();
        format!("| {} |", v.join(" | "))
    };
    let mut out = vec![line(&t.rows[0])];
    let d: Vec<String> = (0..n).map(|c| delim(w[c], t.al[c])).collect();
    out.push(format!("| {} |", d.join(" | ")));
    for r in &t.rows[1..] {
        out.push(line(r));
    }
    out.join("\n")
}

/// sort key: case-insensitive; cells that are both numbers compare as numbers
fn cmp_cells(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

/// apply `op` at (row, col) to the table `src`; returns the re-padded table.
/// ops: row-above row-below row-up row-down row-dup row-del
///      col-left col-right col-move-left col-move-right col-dup col-del
///      align-left align-center align-right sort-asc sort-desc format
pub fn apply(src: &str, op: &str, row: usize, col: usize) -> Result<String, String> {
    let mut t = parse(src)?;
    let (nr, nc) = (t.rows.len(), t.al.len());
    if row >= nr || col >= nc {
        return Err(format!("cell {row},{col} outside a {nr}x{nc} table"));
    }
    let empty_row = vec![String::new(); nc];
    match op {
        "format" => {}
        "row-above" => t.rows.insert(row, empty_row),
        "row-below" => t.rows.insert(row + 1, empty_row),
        "row-up" if row > 0 => t.rows.swap(row, row - 1),
        "row-down" if row + 1 < nr => t.rows.swap(row, row + 1),
        "row-dup" => {
            let r = t.rows[row].clone();
            t.rows.insert(row + 1, r)
        }
        "row-del" if nr > 1 => {
            t.rows.remove(row); // the header goes: the first body row becomes the header
        }
        "col-left" | "col-right" => {
            let at = if op == "col-left" { col } else { col + 1 };
            for r in t.rows.iter_mut() {
                r.insert(at, String::new());
            }
            t.al.insert(at, Al::None);
        }
        "col-move-left" if col > 0 => {
            for r in t.rows.iter_mut() {
                r.swap(col, col - 1);
            }
            t.al.swap(col, col - 1);
        }
        "col-move-right" if col + 1 < nc => {
            for r in t.rows.iter_mut() {
                r.swap(col, col + 1);
            }
            t.al.swap(col, col + 1);
        }
        "col-dup" => {
            for r in t.rows.iter_mut() {
                let c = r[col].clone();
                r.insert(col + 1, c);
            }
            let a = t.al[col];
            t.al.insert(col + 1, a);
        }
        "col-del" if nc > 1 => {
            for r in t.rows.iter_mut() {
                r.remove(col);
            }
            t.al.remove(col);
        }
        "align-left" => t.al[col] = Al::Left,
        "align-center" => t.al[col] = Al::Center,
        "align-right" => t.al[col] = Al::Right,
        "sort-asc" | "sort-desc" => {
            let body = &mut t.rows[1..]; // the header never moves; a stable sort keeps equal rows in order
            if op == "sort-asc" {
                body.sort_by(|a, b| cmp_cells(&a[col], &b[col]));
            } else {
                body.sort_by(|a, b| cmp_cells(&b[col], &a[col]));
            }
        }
        _ => return Err(format!("table op {op:?} does not apply at {row},{col} of a {nr}x{nc} table")),
    }
    Ok(render(&t))
}

#[tauri::command]
pub fn table_op(src: String, op: String, row: usize, col: usize) -> Result<String, String> {
    if src.len() > 1 << 20 {
        return Err("table too large".into());
    }
    apply(&src, &op, row, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    // the fixture stock 1.13.7 acted on (recon37 logs/stock-actions.txt, Insert>Table body)
    const T: &str = "| Name | Qty | Note |\n| :--- | :-: | ---: |\n| apple | 3 | red |\n| pear | 5 | green |";

    #[test]
    fn row_add_below_on_apple_matches_stock() {
        // stock-actions.txt "Row>Add row below on apple"
        let want = "| Name  | Qty |  Note |\n| :---- | :-: | ----: |\n| apple |  3  |   red |\n|       |     |       |\n| pear  |  5  | green |";
        assert_eq!(apply(T, "row-below", 1, 0).unwrap(), want);
    }

    #[test]
    fn column_align_right_on_name_matches_stock() {
        // stock-actions.txt "Column>Align right on Name col"
        let want = "|  Name | Qty |  Note |\n| ----: | :-: | ----: |\n| apple |  3  |   red |\n|  pear |  5  | green |";
        assert_eq!(apply(T, "align-right", 1, 0).unwrap(), want);
    }

    #[test]
    fn sort_z_to_a_on_name_matches_stock() {
        // stock-actions.txt "Sort Z to A on Name col"
        let want = "| Name  | Qty |  Note |\n| :---- | :-: | ----: |\n| pear  |  5  | green |\n| apple |  3  |   red |";
        assert_eq!(apply(T, "sort-desc", 1, 0).unwrap(), want);
        assert_eq!(apply(&apply(T, "sort-desc", 0, 0).unwrap(), "sort-asc", 2, 0).unwrap(), apply(T, "format", 0, 0).unwrap());
    }

    #[test]
    fn empty_table_pads_to_three() {
        // stock Insert>Table output is already in the padded form: format is a fixpoint
        let e = "|     |     |\n| --- | --- |\n|     |     |";
        assert_eq!(apply(e, "format", 0, 0).unwrap(), e);
    }

    #[test]
    fn rows_move_duplicate_delete() {
        let f = |op, r| apply(T, op, r, 0).unwrap();
        assert!(f("row-down", 1).contains("| pear  |  5  | green |\n| apple |  3  |   red |"));
        assert!(f("row-up", 2).contains("| pear  |  5  | green |\n| apple |  3  |   red |"));
        assert_eq!(f("row-dup", 2).matches("| pear  |  5  | green |").count(), 2);
        assert!(!f("row-del", 1).contains("apple"));
        assert!(f("row-above", 0).starts_with("|       |     |       |\n| :---- |"));
        assert!(apply(T, "row-down", 2, 0).is_err()); // the last row cannot move down
        assert!(apply(T, "row-up", 0, 0).is_err()); // nor the header up
    }

    #[test]
    fn columns_add_move_duplicate_delete() {
        let f = |op, c| apply(T, op, 1, c).unwrap();
        assert!(f("col-left", 0).starts_with("|     | Name  |"));
        assert!(f("col-right", 2).starts_with("| Name  | Qty |  Note |     |\n| :---- | :-: | ----: | --- |"));
        assert!(f("col-move-right", 0).starts_with("| Qty | Name  |  Note |\n| :-: | :---- | ----: |"));
        assert!(f("col-move-left", 1).starts_with("| Qty | Name  |  Note |"));
        assert!(f("col-dup", 1).starts_with("| Name  | Qty | Qty |  Note |\n| :---- | :-: | :-: | ----: |"));
        assert_eq!(f("col-del", 1), "| Name  |  Note |\n| :---- | ----: |\n| apple |   red |\n| pear  | green |");
        assert_eq!(f("align-center", 0).lines().nth(1), Some("| :---: | :-: | ----: |"));
        assert_eq!(f("align-left", 2).lines().nth(3), Some("| pear  |  5  | green |"));
        assert!(apply(T, "col-move-left", 1, 0).is_err());
        assert!(apply(T, "col-move-right", 1, 2).is_err());
    }

    #[test]
    fn escaped_and_code_pipes_stay_in_their_cell() {
        let t = "| a | b |\n|---|---|\n| x \\| y | `p|q` |";
        assert_eq!(apply(t, "format", 0, 0).unwrap(), "| a      | b     |\n| ------ | ----- |\n| x \\| y | `p|q` |");
    }

    #[test]
    fn refuses_non_tables_and_bad_cells() {
        assert!(apply("| a |", "format", 0, 0).is_err());
        assert!(apply("| a | b |\n| x | y |", "format", 0, 0).is_err());
        assert!(apply(T, "row-below", 9, 0).is_err());
        assert!(apply(T, "nope", 0, 0).is_err());
    }
}
