use crate::gesture::{Direction, Gesture, PinchDirection};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// A gesture slot that can be bound to an action.
///
/// Written in config files as compact strings: `tap:3`, `doubletap:3`,
/// `tripletap:3`, `hold:4`, `swipe:3:up`, `pinch:4:in`, `drag:3`, and
/// `shape:1` for a recorded two-finger shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Trigger {
    Tap(u8),
    DoubleTap(u8),
    TripleTap(u8),
    Hold(u8),
    Swipe(u8, Direction),
    Pinch(u8, PinchDirection),
    Drag(u8),
    /// A recorded two-finger shape, by id (1..=[`crate::shapes::MAX_SHAPES`]).
    Shape(u8),
}

impl Trigger {
    pub const MIN_FINGERS: u8 = 3;
    pub const MAX_FINGERS: u8 = 5;

    pub fn fingers(self) -> u8 {
        match self {
            Trigger::Tap(n)
            | Trigger::DoubleTap(n)
            | Trigger::TripleTap(n)
            | Trigger::Hold(n)
            | Trigger::Swipe(n, _)
            | Trigger::Pinch(n, _)
            | Trigger::Drag(n) => n,
            Trigger::Shape(_) => 2,
        }
    }

    /// How many taps in a row this trigger needs (0 for anything but taps).
    pub fn tap_count(self) -> u8 {
        match self {
            Trigger::Tap(_) => 1,
            Trigger::DoubleTap(_) => 2,
            Trigger::TripleTap(_) => 3,
            _ => 0,
        }
    }

    /// Swipes and pinches can keep firing while the fingers keep moving.
    pub fn can_repeat(self) -> bool {
        matches!(self, Trigger::Swipe(..) | Trigger::Pinch(..))
    }

    /// The trigger a discrete gesture fires, if any. Drag movement is a
    /// stream rather than a trigger, so only `DragStart` maps here.
    pub fn from_gesture(g: &Gesture) -> Option<Trigger> {
        match *g {
            Gesture::Tap { fingers } => Some(Trigger::Tap(fingers)),
            Gesture::DoubleTap { fingers } => Some(Trigger::DoubleTap(fingers)),
            Gesture::TripleTap { fingers } => Some(Trigger::TripleTap(fingers)),
            Gesture::Hold { fingers } => Some(Trigger::Hold(fingers)),
            Gesture::Swipe { fingers, direction, .. } => Some(Trigger::Swipe(fingers, direction)),
            Gesture::Pinch { fingers, direction, .. } => Some(Trigger::Pinch(fingers, direction)),
            Gesture::DragStart { fingers } => Some(Trigger::Drag(fingers)),
            Gesture::Shape { id } => Some(Trigger::Shape(id)),
            Gesture::DragMove { .. } | Gesture::DragEnd => None,
        }
    }

    /// Every bindable slot for one finger count, in display order.
    pub fn all_for(fingers: u8) -> Vec<Trigger> {
        let mut v = vec![
            Trigger::Tap(fingers),
            Trigger::DoubleTap(fingers),
            Trigger::TripleTap(fingers),
            Trigger::Hold(fingers),
        ];
        v.extend(Direction::ALL.iter().map(|&d| Trigger::Swipe(fingers, d)));
        v.extend(PinchDirection::ALL.iter().map(|&d| Trigger::Pinch(fingers, d)));
        v.push(Trigger::Drag(fingers));
        v
    }

    pub fn label(self) -> String {
        match self {
            Trigger::Tap(n) => format!("{n}-finger tap"),
            Trigger::DoubleTap(n) => format!("{n}-finger double tap"),
            Trigger::TripleTap(n) => format!("{n}-finger triple tap"),
            Trigger::Hold(n) => format!("{n}-finger hold"),
            Trigger::Swipe(n, d) => format!("{n}-finger swipe {d}"),
            Trigger::Pinch(n, d) => format!("{n}-finger pinch {d}"),
            Trigger::Drag(n) => format!("{n}-finger drag"),
            Trigger::Shape(id) => format!("2-finger shape {id}"),
        }
    }
}

impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Trigger::Tap(n) => write!(f, "tap:{n}"),
            Trigger::DoubleTap(n) => write!(f, "doubletap:{n}"),
            Trigger::TripleTap(n) => write!(f, "tripletap:{n}"),
            Trigger::Hold(n) => write!(f, "hold:{n}"),
            Trigger::Swipe(n, d) => write!(f, "swipe:{n}:{d}"),
            Trigger::Pinch(n, d) => write!(f, "pinch:{n}:{d}"),
            Trigger::Drag(n) => write!(f, "drag:{n}"),
            Trigger::Shape(id) => write!(f, "shape:{id}"),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("invalid trigger `{0}` (expected e.g. tap:3, doubletap:3, hold:4, swipe:3:up, pinch:4:in, drag:3, shape:1)")]
pub struct ParseTriggerError(pub String);

impl FromStr for Trigger {
    type Err = ParseTriggerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseTriggerError(s.to_string());
        let parts: Vec<&str> = s.trim().split(':').map(str::trim).collect();
        let number: u8 = parts.get(1).and_then(|n| n.parse().ok()).ok_or_else(err)?;
        if parts[0].eq_ignore_ascii_case("shape") {
            let ok = parts.len() == 2 && (1..=crate::shapes::MAX_SHAPES).contains(&number);
            return if ok { Ok(Trigger::Shape(number)) } else { Err(err()) };
        }
        let fingers = number;
        if !(Trigger::MIN_FINGERS..=Trigger::MAX_FINGERS).contains(&fingers) {
            return Err(err());
        }
        let kind = parts[0].to_ascii_lowercase();
        match (kind.as_str(), parts.len()) {
            ("tap", 2) => Ok(Trigger::Tap(fingers)),
            ("doubletap", 2) => Ok(Trigger::DoubleTap(fingers)),
            ("tripletap", 2) => Ok(Trigger::TripleTap(fingers)),
            ("hold", 2) => Ok(Trigger::Hold(fingers)),
            ("drag", 2) => Ok(Trigger::Drag(fingers)),
            ("swipe", 3) => Direction::parse(parts[2]).map(|d| Trigger::Swipe(fingers, d)).ok_or_else(err),
            ("pinch", 3) => PinchDirection::parse(parts[2]).map(|d| Trigger::Pinch(fingers, d)).ok_or_else(err),
            _ => Err(err()),
        }
    }
}

impl Serialize for Trigger {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Trigger {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_slot() {
        for n in Trigger::MIN_FINGERS..=Trigger::MAX_FINGERS {
            for t in Trigger::all_for(n) {
                assert_eq!(t.to_string().parse::<Trigger>(), Ok(t));
            }
        }
    }

    #[test]
    fn parsing_is_forgiving_about_case_and_spaces() {
        assert_eq!(" Swipe : 4 : LEFT ".parse(), Ok(Trigger::Swipe(4, Direction::Left)));
        assert_eq!("PINCH:3:Out".parse(), Ok(Trigger::Pinch(3, PinchDirection::Out)));
        assert_eq!(" DoubleTap:4".parse(), Ok(Trigger::DoubleTap(4)));
        assert_eq!(Trigger::TripleTap(3).tap_count(), 3);
        assert_eq!(" Shape:7".parse(), Ok(Trigger::Shape(7)));
        assert_eq!(Trigger::Shape(7).to_string(), "shape:7");
        assert_eq!(Trigger::Shape(7).fingers(), 2);
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "",
            "tap",
            "tap:2",
            "tap:6",
            "swipe:3",
            "swipe:3:north",
            "pinch:3",
            "pinch:3:up",
            "tap:3:up",
            "shape",
            "shape:0",
            "shape:33",
            "shape:1:up",
        ] {
            assert!(bad.parse::<Trigger>().is_err(), "{bad} should not parse");
        }
    }

    #[test]
    fn only_continuous_triggers_repeat() {
        assert!(Trigger::Swipe(3, Direction::Up).can_repeat());
        assert!(Trigger::Pinch(3, PinchDirection::In).can_repeat());
        assert!(!Trigger::Tap(3).can_repeat());
        assert!(!Trigger::DoubleTap(3).can_repeat());
        assert!(!Trigger::Drag(3).can_repeat());
    }

    /// Trigger strings come from the config file: random ones parse or fail
    /// cleanly, and whatever parses prints back to the same trigger.
    #[test]
    fn fuzzed_trigger_strings_never_panic() {
        let alphabet: Vec<char> = "tapdoublrihswpncg:0123456789 -+é🙂\u{0}".chars().collect();
        let names = ["tap", "doubletap", "tripletap", "hold", "swipe", "pinch", "drag", "shape"];
        let tails = ["", ":up", ":left", ":in", ":out", ":sideways", ":up:up"];
        let mut rng = crate::Rng::new(0x1234_5678_9ABC_DEF1);
        for i in 0..30_000 {
            let s = if i % 2 == 0 {
                let len = rng.below(16) as usize;
                rng.text(&alphabet, len)
            } else {
                let n = rng.next() as i64 % 12;
                format!("{}:{n}{}", names[rng.below(8) as usize], tails[rng.below(7) as usize])
            };
            if let Ok(t) = s.parse::<Trigger>() {
                assert_eq!(t.to_string().parse::<Trigger>().ok(), Some(t), "{s:?}");
            }
        }
    }
}
