//! Keeping the log file honest and free of secrets.
//!
//! `pady.log` is what people attach to bug reports, so two things hold for
//! every line written to it:
//!
//! * **One event, one line.** Text that comes from outside (a device's
//!   product name, a program's name, a broken config's contents) may hold
//!   line breaks or terminal escape codes. [`log_line`] writes those as
//!   visible escapes, so nothing can start a made-up log line or recolour a
//!   terminal.
//! * **No secrets.** A failed action is logged by what it is, not by all of
//!   its text: see [`crate::Action::log_label`].

use std::borrow::Cow;

/// Characters that change how following text is laid out or read: line and
/// paragraph separators, and the bidirectional overrides that can make a
/// line read differently from what it says.
fn is_deceptive(c: char) -> bool {
    matches!(c, '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// `text` as one log line: control characters (line breaks, escape codes)
/// and layout-changing characters become visible escapes such as `\n` and
/// `\u{1b}`. Ordinary text comes back untouched.
pub fn log_line(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|c| c.is_control() || is_deceptive(c)) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() || is_deceptive(c) => out.extend(c.escape_unicode()),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// The program a shell command starts, without its arguments:
/// `curl -H "Authorization: …" https://…` → `curl`.
pub(crate) fn command_name(command: &str) -> &str {
    let command = command.trim_start();
    match command.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next().unwrap_or(""),
        None => command.split_whitespace().next().unwrap_or(""),
    }
}

/// Where a link points, without what may be private in it (the path, the
/// query, a user name or password): `https://hooks.example.com/key/abc?x=1`
/// → `https://hooks.example.com`. App links keep only their scheme
/// (`spotify:`), local paths only their file name.
pub(crate) fn link_site(url: &str) -> String {
    let url = url.trim();
    if let Some((scheme, rest)) = url.split_once("://") {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let host = authority.rsplit('@').next().unwrap_or("");
        return format!("{scheme}://{host}");
    }
    match url.split_once(':') {
        // `C:\page.html`: a drive letter, not a scheme.
        Some((scheme, _)) if scheme.len() > 1 && !scheme.contains(['/', '\\']) => format!("{scheme}:"),
        _ => url.rsplit(['/', '\\']).next().unwrap_or("").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_text_is_untouched() {
        for text in ["", "touchpad `ELAN0504` 105×67 mm", "Öffnen · 開く · فتح", "tab-free, break-free"] {
            assert!(matches!(log_line(text), Cow::Borrowed(t) if t == text), "{text:?}");
        }
    }

    #[test]
    fn nothing_can_start_a_new_line_or_send_escape_codes() {
        let hostile = "Touchpad\n[2026-01-01T00:00:00Z INFO  pady] update installed\r\x1b[2J\u{2028}x\u{202E}txt.exe\0";
        let line = log_line(hostile);
        assert!(!line.chars().any(|c| c.is_control() || is_deceptive(c)), "{line:?}");
        assert!(
            line.starts_with("Touchpad\\n[2026-01-01T00:00:00Z INFO  pady] update installed\\r\\u{1b}[2J"),
            "{line}"
        );
        assert_eq!(line.lines().count(), 1);
    }

    #[test]
    fn commands_are_named_by_their_program() {
        assert_eq!(command_name("curl -H \"Authorization: Bearer x\" https://a"), "curl");
        assert_eq!(command_name("  \"C:\\Program Files\\App\\app.exe\" --token x"), "C:\\Program Files\\App\\app.exe");
        assert_eq!(command_name(""), "");
        assert_eq!(command_name("shutdown"), "shutdown");
    }

    #[test]
    fn links_are_named_by_their_site() {
        for (url, site) in [
            ("https://hooks.example.com/trigger/x/with/key/abc?user=me#f", "https://hooks.example.com"),
            ("https://user:pass@example.com:8443/a", "https://example.com:8443"),
            ("http://example.com", "http://example.com"),
            ("spotify:track:123", "spotify:"),
            ("mailto:me@example.com", "mailto:"),
            ("ms-settings:display", "ms-settings:"),
            ("C:\\Users\\me\\secret plan.html", "secret plan.html"),
            ("/home/me/page.html", "page.html"),
            ("", ""),
        ] {
            assert_eq!(link_site(url), site, "{url:?}");
        }
    }
}
