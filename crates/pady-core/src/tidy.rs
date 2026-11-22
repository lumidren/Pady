//! Tidying a copied line before it is pasted.
//!
//! Text selected on a web page often carries a space, a tab or a line break
//! from the layout around it, and a form field shows that as a gap before
//! the text. With "Trim spaces when pasting" on, a paste gesture removes it.

/// Blank on screen: spaces, tabs and line breaks of every kind (no-break
/// spaces too), and the invisible zero-width space and byte-order mark that
/// pages sometimes carry.
fn is_blank(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{200B}' | '\u{FEFF}')
}

/// `text` without the blanks around it, or `None` when it is to be left
/// alone: there is nothing to remove, there is nothing but blanks, or it is
/// several lines (the indentation of code and the shape of a paragraph are
/// the writer's business).
pub fn trimmed_line(text: &str) -> Option<&str> {
    let line = text.trim_matches(is_blank);
    let several_lines = line.contains(['\n', '\r', '\u{85}', '\u{2028}', '\u{2029}']);
    if line.is_empty() || line.len() == text.len() || several_lines {
        return None;
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::trimmed_line;

    #[test]
    fn blanks_around_one_line_go() {
        for (copied, pasted) in [
            ("   Critical-DIA", "Critical-DIA"),
            ("\tCritical-DIA", "Critical-DIA"),
            ("\r\n  Critical-DIA \r\n", "Critical-DIA"),
            ("Critical-DIA ", "Critical-DIA"),
            ("\u{A0}Critical-DIA\u{A0}", "Critical-DIA"),
            ("\u{200B}\u{FEFF} two words\t", "two words"),
            (" مرحبا ", "مرحبا"),
        ] {
            assert_eq!(trimmed_line(copied), Some(pasted), "{copied:?}");
        }
    }

    #[test]
    fn blanks_inside_the_line_stay() {
        assert_eq!(trimmed_line(" a  b\tc "), Some("a  b\tc"));
    }

    #[test]
    fn what_needs_no_change_is_left_alone() {
        for copied in ["Critical-DIA", "", "   ", "\r\n", "two words"] {
            assert_eq!(trimmed_line(copied), None, "{copied:?}");
        }
    }

    #[test]
    fn several_lines_are_left_alone() {
        for copied in ["    if x {\n        y();\n    }\n", " first\r\nsecond ", " a\u{2028}b "] {
            assert_eq!(trimmed_line(copied), None, "{copied:?}");
        }
    }
}
