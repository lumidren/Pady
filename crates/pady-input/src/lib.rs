//! Touchpad input backends.
//!
//! [`start`] finds the touchpad(s) and streams [`TouchFrame`]s, in
//! millimetres, into the given channel from a background thread.
//!
//! The protocol logic lives in OS-independent modules so it is unit tested
//! on every platform:
//! * [`hid`]: HID digitizer reports (Windows precision touchpads)
//! * [`mt`]: the Linux multitouch slot protocol (type B)

pub mod hid;
pub mod mt;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

use crossbeam_channel::Sender;
use pady_core::TouchFrame;
use serde::Serialize;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DeviceInfo {
    pub name: String,
    pub width_mm: f32,
    pub height_mm: f32,
    pub max_contacts: u8,
}

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("no precision touchpad was found")]
    NoTouchpad,
    #[error("permission denied reading touchpad: {0}")]
    Permission(String),
    #[error("touchpad input is not supported on this operating system")]
    Unsupported,
    #[error("{0}")]
    Os(String),
}

/// Keeps the backend running. Devices may be discovered lazily (Windows
/// learns about a touchpad on its first report), so the list can grow.
#[derive(Clone, Debug, Default)]
pub struct InputHandle {
    devices: Arc<Mutex<Vec<DeviceInfo>>>,
}

impl InputHandle {
    pub(crate) fn new(devices: Arc<Mutex<Vec<DeviceInfo>>>) -> Self {
        InputHandle { devices }
    }

    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.devices.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Start reading touchpads, sending frames to `frames`.
pub fn start(frames: Sender<TouchFrame>) -> Result<InputHandle, InputError> {
    #[cfg(windows)]
    return windows::start(frames);
    #[cfg(target_os = "linux")]
    return linux::start(frames);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = frames;
        Err(InputError::Unsupported)
    }
}
