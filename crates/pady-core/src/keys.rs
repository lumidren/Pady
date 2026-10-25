//! Platform-neutral keyboard shortcuts. Executors map [`Key`] to virtual-key
//! codes (Windows) or evdev key codes (Linux).

use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The Windows / Super / Meta key.
    pub super_key: bool,
}

macro_rules! keys {
    ($($variant:ident => [$($name:literal),+]),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Key { $($variant),+ }

        impl Key {
            pub const ALL: &'static [Key] = &[$(Key::$variant),+];

            /// Canonical lowercase name, as written in config files.
            pub fn name(self) -> &'static str {
                match self { $(Key::$variant => keys!(@first $($name),+)),+ }
            }

            pub fn from_name(s: &str) -> Option<Key> {
                let s = s.trim().to_ascii_lowercase();
                match s.as_str() {
                    $($($name)|+ => Some(Key::$variant),)+
                    _ => None,
                }
            }
        }
    };
    (@first $first:literal $(, $rest:literal)*) => { $first };
}

keys! {
    A => ["a"], B => ["b"], C => ["c"], D => ["d"], E => ["e"], F => ["f"], G => ["g"],
    H => ["h"], I => ["i"], J => ["j"], K => ["k"], L => ["l"], M => ["m"], N => ["n"],
    O => ["o"], P => ["p"], Q => ["q"], R => ["r"], S => ["s"], T => ["t"], U => ["u"],
    V => ["v"], W => ["w"], X => ["x"], Y => ["y"], Z => ["z"],
    Num0 => ["0"], Num1 => ["1"], Num2 => ["2"], Num3 => ["3"], Num4 => ["4"],
    Num5 => ["5"], Num6 => ["6"], Num7 => ["7"], Num8 => ["8"], Num9 => ["9"],
    F1 => ["f1"], F2 => ["f2"], F3 => ["f3"], F4 => ["f4"], F5 => ["f5"], F6 => ["f6"],
    F7 => ["f7"], F8 => ["f8"], F9 => ["f9"], F10 => ["f10"], F11 => ["f11"], F12 => ["f12"],
    Enter => ["enter", "return"],
    Escape => ["esc", "escape"],
    Tab => ["tab"],
    Space => ["space"],
    Backspace => ["backspace"],
    Delete => ["delete", "del"],
    Insert => ["insert", "ins"],
    Home => ["home"],
    End => ["end"],
    PageUp => ["pageup", "pgup"],
    PageDown => ["pagedown", "pgdn"],
    Up => ["up"],
    Down => ["down"],
    Left => ["left"],
    Right => ["right"],
    PrintScreen => ["printscreen", "prtsc", "print"],
    Minus => ["minus", "-"],
    Equal => ["equal", "plus", "="],
    Comma => ["comma", ","],
    Period => ["period", "."],
    Slash => ["slash", "/"],
    Backslash => ["backslash", "\\"],
    Semicolon => ["semicolon", ";"],
    Quote => ["quote", "'"],
    BracketLeft => ["bracketleft", "["],
    BracketRight => ["bracketright", "]"],
    Grave => ["grave", "backtick", "`"],
}

/// A shortcut like `ctrl+shift+t`. A combo may be modifiers only
/// (`win` taps the Windows key on its own).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyCombo {
    pub modifiers: Modifiers,
    pub key: Option<Key>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ParseComboError {
    #[error("empty shortcut")]
    Empty,
    #[error("unknown key `{0}`")]
    UnknownKey(String),
    #[error("shortcut has two main keys (`{0}` and `{1}`)")]
    TwoKeys(String, String),
}

impl FromStr for KeyCombo {
    type Err = ParseComboError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut modifiers = Modifiers::default();
        let mut key: Option<Key> = None;
        // Split on '+' but allow a literal "+" key written as "plus".
        let parts: Vec<&str> = s.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            return Err(ParseComboError::Empty);
        }
        for part in parts {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers.ctrl = true,
                "alt" | "option" => modifiers.alt = true,
                "shift" => modifiers.shift = true,
                "win" | "super" | "meta" | "cmd" | "windows" => modifiers.super_key = true,
                other => {
                    let k = Key::from_name(other).ok_or_else(|| ParseComboError::UnknownKey(part.to_string()))?;
                    if let Some(prev) = key {
                        return Err(ParseComboError::TwoKeys(prev.name().into(), k.name().into()));
                    }
                    key = Some(k);
                }
            }
        }
        Ok(KeyCombo { modifiers, key })
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.modifiers;
        let mut parts: Vec<&str> = Vec::new();
        if m.ctrl {
            parts.push("ctrl");
        }
        if m.alt {
            parts.push("alt");
        }
        if m.shift {
            parts.push("shift");
        }
        if m.super_key {
            parts.push("win");
        }
        if let Some(k) = self.key {
            parts.push(k.name());
        }
        f.write_str(&parts.join("+"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_shortcuts() {
        let c: KeyCombo = "Ctrl+Shift+T".parse().unwrap();
        assert!(c.modifiers.ctrl && c.modifiers.shift && !c.modifiers.alt);
        assert_eq!(c.key, Some(Key::T));

        let c: KeyCombo = "super + left".parse().unwrap();
        assert!(c.modifiers.super_key);
        assert_eq!(c.key, Some(Key::Left));

        let c: KeyCombo = "win".parse().unwrap();
        assert!(c.modifiers.super_key);
        assert_eq!(c.key, None);
    }

    #[test]
    fn display_is_canonical() {
        let c: KeyCombo = "shift+CONTROL+esc".parse().unwrap();
        assert_eq!(c.to_string(), "ctrl+shift+esc");
    }

    #[test]
    fn errors_are_specific() {
        assert_eq!("".parse::<KeyCombo>(), Err(ParseComboError::Empty));
        assert_eq!("ctrl+banana".parse::<KeyCombo>(), Err(ParseComboError::UnknownKey("banana".into())));
        assert_eq!("a+b".parse::<KeyCombo>(), Err(ParseComboError::TwoKeys("a".into(), "b".into())));
    }

    #[test]
    fn every_key_name_round_trips() {
        for &k in Key::ALL {
            assert_eq!(Key::from_name(k.name()), Some(k));
        }
    }

    /// Shortcuts come from the config file and the recorder: random strings
    /// parse or fail cleanly, and whatever parses prints back the same.
    #[test]
    fn fuzzed_shortcuts_never_panic() {
        let alphabet: Vec<char> = "ctrlaltshiftwinsuperf0123456789+-=,./;[]'\\` é🙂".chars().collect();
        let words = ["ctrl", "alt", "shift", "win", "a", "f12", "f13", "esc", "plus", "-", "+", "", "banana"];
        let mut rng = crate::Rng::new(0x0BAD_C0FF_EE15_600D);
        for i in 0..30_000 {
            let s = if i % 2 == 0 {
                let len = rng.below(14) as usize;
                rng.text(&alphabet, len)
            } else {
                (0..=rng.below(4)).map(|_| words[rng.below(words.len() as u64) as usize]).collect::<Vec<_>>().join("+")
            };
            if let Ok(c) = s.parse::<KeyCombo>() {
                assert_eq!(c.to_string().parse::<KeyCombo>(), Ok(c), "{s:?}");
            }
        }
    }
}
