//! Pady core: everything that does not depend on the operating system.
//!
//! Data flows in one direction:
//!
//! ```text
//! touchpad backend ──TouchFrame──▶ Recognizer ──Gesture──▶ Engine ──Action──▶ Executor
//! ```
//!
//! Backends (in `pady-input`) turn raw HID / evdev reports into [`TouchFrame`]s
//! measured in millimetres, so every threshold here is device independent.
//! Executors (in `pady-actions`) carry out [`Action`]s on the host OS.

pub mod action;
pub mod config;
pub mod engine;
pub mod frame;
pub mod gesture;
pub mod keys;
pub mod recognizer;
pub mod trigger;

pub use action::{Action, MediaKey};
pub use config::{Binding, Config, ConfigError, Profile, Settings, SharedConfig};
pub use engine::{ActionError, Engine, EngineEvent, Executor, Job, Level, LevelKind};
pub use frame::{now_ms, Contact, TouchFrame};
pub use gesture::{Direction, Gesture, PinchDirection};
pub use keys::{Key, KeyCombo, Modifiers};
pub use recognizer::{Recognizer, RecognizerSettings};
pub use trigger::Trigger;
