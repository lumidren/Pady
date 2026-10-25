use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    pub const ALL: [Direction; 4] = [Direction::Up, Direction::Down, Direction::Left, Direction::Right];

    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Up => "up",
            Direction::Down => "down",
            Direction::Left => "left",
            Direction::Right => "right",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "up" => Some(Direction::Up),
            "down" => Some(Direction::Down),
            "left" => Some(Direction::Left),
            "right" => Some(Direction::Right),
            _ => None,
        }
    }

    /// Direction of the dominant axis of a movement (screen coordinates, y down).
    pub fn from_delta(dx: f32, dy: f32) -> Self {
        if dx.abs() >= dy.abs() {
            if dx >= 0.0 {
                Direction::Right
            } else {
                Direction::Left
            }
        } else if dy >= 0.0 {
            Direction::Down
        } else {
            Direction::Up
        }
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the recognizer saw. Discrete gestures map to [`crate::Trigger`]s;
/// drags are a start / move / end stream that the engine turns into a held
/// mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Gesture {
    Tap { fingers: u8 },
    Hold { fingers: u8 },
    /// `step` is 0 for the first trigger of a swipe and counts up as the
    /// fingers keep travelling (used for "scrub" bindings like volume).
    Swipe { fingers: u8, direction: Direction, step: u32 },
    DragStart { fingers: u8 },
    /// Movement in millimetres since the previous drag event.
    DragMove { dx: f32, dy: f32 },
    DragEnd,
}
