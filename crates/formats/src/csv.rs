//! Comma-separated values (RFC 4180, leniently): the cells of a table pasted or typed as text,
//! for Insert › Tables › Spreadsheet Table.
//!
//! Quoted cells may hold separators, line breaks and doubled quotes; the separator is the comma,
//! semicolon or tab the first line has most of; `\n`, `\r\n` and `\r` end rows; blank rows are
//! dropped. The text is hostile: it is read once, character by character (never sliced), and at
//! most [`MAX_ROWS`] rows are kept.

/// Most rows kept; the rest of the text is ignored.
pub const MAX_ROWS: usize = 100_000;

/// The rows of `text`, each a list of cells (rows may differ in length).
pub fn parse(text: &str) -> Vec<Vec<String>> {
    let first = text.lines().next().unwrap_or("");
    let sep = [',', ';', '\t'].into_iter().max_by_key(|c| first.matches(*c).count()).unwrap_or(',');
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut q = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if q {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cell.push('"');
                    chars.next();
                } else {
                    q = false;
                }
            } else {
                cell.push(c);
            }
        } else if c == '"' {
            q = true;
        } else if c == sep {
            row.push(std::mem::take(&mut cell));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut cell));
            if row.iter().any(|x| !x.is_empty()) {
                rows.push(std::mem::take(&mut row));
            } else {
                row.clear();
            }
            if rows.len() >= MAX_ROWS {
                return rows;
            }
        } else {
            cell.push(c);
        }
    }
    row.push(cell);
    if row.iter().any(|x| !x.is_empty()) {
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_separators_and_line_ends() {
        let r = parse("Name,City\n\"Smith, Ann\",Paris\r\nBo,\"Oslo \"\"N\"\"\"\n\n");
        assert_eq!(r, vec![vec!["Name", "City"], vec!["Smith, Ann", "Paris"], vec!["Bo", "Oslo \"N\""]]);
        assert_eq!(parse("a;b\n1;2")[1], vec!["1", "2"]);
        assert_eq!(parse("a\tb\rc\td"), vec![vec!["a", "b"], vec!["c", "d"]]);
        // A quoted line break stays in its cell.
        assert_eq!(parse("x,\"one\ntwo\""), vec![vec!["x", "one\ntwo"]]);
    }

    /// Hostile text: an unterminated quote, lone quotes, multibyte text and endless rows never
    /// panic, and the rows are capped.
    #[test]
    fn hostile_text_is_read_without_panicking_and_capped() {
        for t in ["", "\"", "\"\"\"", ",,,\n,,", "\"unterminated, cell\nnext", "é,\"ü\"\"\",😀\r\r\n\u{0}", "\r", "\n\n\n"] {
            let _ = parse(t);
        }
        assert_eq!(parse("\"open"), vec![vec!["open"]]);
        assert!(parse(",,,\n\n").is_empty(), "blank rows are dropped");
        let many = "x\n".repeat(MAX_ROWS + 50);
        assert_eq!(parse(&many).len(), MAX_ROWS);
    }
}
