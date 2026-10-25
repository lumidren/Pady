use crate::gesture::{Direction, Gesture};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// A gesture slot that can be bound to an action.
///
/// Written in config files as compact strings: `tap:3`, `hold:4`,
/// `swipe:3:up`, `drag:3`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Trigger {
    Tap(u8),
    Hold(u8),
    Swipe(u8, Direction),
    Drag(u8),
}

impl Trigger {
    pub const MIN_FINGERS: u8 = 3;
    pub const MAX_FINGERS: u8 = 5;

    pub fn fingers(self) -> u8 {
        match self {
            Trigger::Tap(n) | Trigger::Hold(n) | Trigger::Swipe(n, _) | Trigger::Drag(n) => n,
        }
    }

    /// The trigger a discrete gesture fires, if any. Drag movement is a
    /// stream rather than a trigger, so only `DragStart` maps here.
    pub fn from_gesture(g: &Gesture) -> Option<Trigger> {
        match *g {
            Gesture::Tap { fingers } => Some(Trigger::Tap(fingers)),
            Gesture::Hold { fingers } => Some(Trigger::Hold(fingers)),
            Gesture::Swipe { fingers, direction, .. } => Some(Trigger::Swipe(fingers, direction)),
            Gesture::DragStart { fingers } => Some(Trigger::Drag(fingers)),
            Gesture::DragMove { .. } | Gesture::DragEnd => None,
        }
    }

    /// Every bindable slot for one finger count, in display order.
    pub fn all_for(fingers: u8) -> Vec<Trigger> {
        let mut v = vec![Trigger::Tap(fingers), Trigger::Hold(fingers)];
        v.extend(Direction::ALL.iter().map(|&d| Trigger::Swipe(fingers, d)));
        v.push(Trigger::Drag(fingers));
        v
    }

    pub fn label(self) -> String {
        match self {
            Trigger::Tap(n) => format!("{n}-finger tap"),
            Trigger::Hold(n) => format!("{n}-finger hold"),
            Trigger::Swipe(n, d) => format!("{n}-finger swipe {d}"),
            Trigger::Drag(n) => format!("{n}-finger drag"),
        }
    }
}

impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Trigger::Tap(n) => write!(f, "tap:{n}"),
            Trigger::Hold(n) => write!(f, "hold:{n}"),
            Trigger::Swipe(n, d) => write!(f, "swipe:{n}:{d}"),
            Trigger::Drag(n) => write!(f, "drag:{n}"),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("invalid trigger `{0}` (expected e.g. tap:3, hold:4, swipe:3:up, drag:3)")]
pub struct ParseTriggerError(pub String);

impl FromStr for Trigger {
    type Err = ParseTriggerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseTriggerError(s.to_string());
        let parts: Vec<&str> = s.trim().split(':').map(str::trim).collect();
        let fingers: u8 = parts.get(1).and_then(|n| n.parse().ok()).ok_or_else(err)?;
        if !(Trigger::MIN_FINGERS..=Trigger::MAX_FINGERS).contains(&fingers) {
            return Err(err());
        }
        let kind = parts[0].to_ascii_lowercase();
        match (kind.as_str(), parts.len()) {
            ("tap", 2) => Ok(Trigger::Tap(fingers)),
            ("hold", 2) => Ok(Trigger::Hold(fingers)),
            ("drag", 2) => Ok(Trigger::Drag(fingers)),
            ("swipe", 3) => Direction::parse(parts[2]).map(|d| Trigger::Swipe(fingers, d)).ok_or_else(err),
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
    }

    #[test]
    fn rejects_bad_input() {
        for bad in ["", "tap", "tap:2", "tap:6", "swipe:3", "swipe:3:north", "pinch:3", "tap:3:up"] {
            assert!(bad.parse::<Trigger>().is_err(), "{bad} should not parse");
        }
    }
}
