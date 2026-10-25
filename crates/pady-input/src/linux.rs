//! Linux backend: reads touchpads through evdev (`/dev/input/event*`).
//!
//! Devices are never grabbed, so the desktop keeps pointer motion, clicks
//! and scrolling; Pady only listens alongside it. Each touchpad gets a reader
//! thread that feeds raw events into an [`MtTracker`] and forwards its frames.

use crate::mt::{AxisRange, MtTracker};
use crate::{DeviceInfo, InputError, InputHandle};
use crossbeam_channel::Sender;
use evdev::raw_stream::RawDevice;
use evdev::{AbsoluteAxisType, InputEvent, InputEventKind, Key, PropType, Synchronization};
use pady_core::{now_ms, TouchFrame};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const PERMISSION_HINT: &str =
    "add your user to the 'input' group: sudo usermod -aG input $USER, then log out and back in";

/// Tool keys and the finger count each one announces.
const TOOL_KEYS: [(Key, u8); 5] = [
    (Key::BTN_TOOL_FINGER, 1),
    (Key::BTN_TOOL_DOUBLETAP, 2),
    (Key::BTN_TOOL_TRIPLETAP, 3),
    (Key::BTN_TOOL_QUADTAP, 4),
    (Key::BTN_TOOL_QUINTTAP, 5),
];

pub(crate) fn start(frames: Sender<TouchFrame>) -> Result<InputHandle, InputError> {
    // Not `evdev::enumerate()`: it silently skips nodes it cannot open, and
    // a permission error is the most common reason no touchpad shows up.
    let mut paths: Vec<PathBuf> = std::fs::read_dir("/dev/input")
        .map_err(|e| InputError::Os(format!("cannot list /dev/input: {e}")))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("event")))
        .collect();
    paths.sort_by_key(|p| event_number(p));

    let devices = Arc::new(Mutex::new(Vec::new()));
    let mut denied = 0usize;
    let mut started = 0usize;
    for path in paths {
        let dev = match RawDevice::open(&path) {
            Ok(dev) => dev,
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                denied += 1;
                continue;
            }
            Err(e) => {
                log::debug!("skipping {}: {e}", path.display());
                continue;
            }
        };
        let Some((info, tracker)) = probe(&dev) else { continue };
        log::info!(
            "touchpad {} at {}: {:.0}x{:.0} mm, {} slots, up to {} fingers",
            info.name,
            path.display(),
            info.width_mm,
            info.height_mm,
            tracker.slot_count(),
            info.max_contacts
        );
        let name = info.name.clone();
        let tx = frames.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("pady-evdev-{}", event_number(&path)))
            .spawn(move || read_loop(dev, tracker, tx, &name));
        match spawned {
            Ok(_) => {
                devices.lock().unwrap_or_else(|e| e.into_inner()).push(info);
                started += 1;
            }
            Err(e) => log::warn!("cannot start reader thread for {}: {e}", path.display()),
        }
    }

    if started > 0 {
        Ok(InputHandle::new(devices))
    } else if denied > 0 {
        log::warn!("{denied} input device(s) could not be opened (permission denied)");
        Err(InputError::Permission(PERMISSION_HINT.into()))
    } else {
        Err(InputError::NoTouchpad)
    }
}

fn event_number(path: &Path) -> u32 {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("event"))
        .and_then(|n| n.parse().ok())
        .unwrap_or(u32::MAX)
}

/// A touchpad is an indirect pointer (touchscreens set `INPUT_PROP_DIRECT`)
/// with multitouch positions, slots and finger tool keys.
fn probe(dev: &RawDevice) -> Option<(DeviceInfo, MtTracker)> {
    let name = dev.name().unwrap_or("touchpad").to_string();
    let props = dev.properties();
    if !props.contains(PropType::POINTER) || props.contains(PropType::DIRECT) {
        return None;
    }
    let abs = dev.supported_absolute_axes()?;
    if !abs.contains(AbsoluteAxisType::ABS_MT_POSITION_X) || !abs.contains(AbsoluteAxisType::ABS_MT_POSITION_Y) {
        return None;
    }
    let keys = dev.supported_keys()?;
    if !keys.contains(Key::BTN_TOOL_FINGER) {
        return None;
    }
    if !abs.contains(AbsoluteAxisType::ABS_MT_SLOT) {
        log::warn!("{name}: legacy multitouch protocol A is not supported");
        return None;
    }
    let state = match dev.get_abs_state() {
        Ok(state) => state,
        Err(e) => {
            log::warn!("{name}: cannot read axis ranges: {e}");
            return None;
        }
    };
    let range = |axis: AbsoluteAxisType| {
        let a = state[axis.0 as usize];
        AxisRange { min: a.minimum, max: a.maximum, resolution: a.resolution }
    };
    let slots = usize::try_from(state[AbsoluteAxisType::ABS_MT_SLOT.0 as usize].maximum).map_or(1, |max| max + 1);
    let mut tracker =
        MtTracker::new(slots, range(AbsoluteAxisType::ABS_MT_POSITION_X), range(AbsoluteAxisType::ABS_MT_POSITION_Y));
    resync(dev, &mut tracker);

    let tool_max = TOOL_KEYS.iter().filter(|(k, _)| keys.contains(*k)).map(|&(_, n)| n).max().unwrap_or(1);
    let info = DeviceInfo {
        name,
        width_mm: tracker.width_mm(),
        height_mm: tracker.height_mm(),
        max_contacts: u8::try_from(slots).unwrap_or(u8::MAX).max(tool_max),
    };
    Some((info, tracker))
}

/// Restore what the kernel lets us query: the held tool keys and the current
/// slot with its contact. Other slots are picked up when they next move (see
/// [`MtTracker::reset`]).
fn resync(dev: &RawDevice, tracker: &mut MtTracker) {
    match dev.get_key_state() {
        Ok(keys) => {
            for &(key, n) in &TOOL_KEYS {
                tracker.tool_key(n, keys.contains(key));
            }
        }
        Err(e) => log::debug!("cannot read key state: {e}"),
    }
    match dev.get_abs_state() {
        Ok(abs) => {
            let value = |axis: AbsoluteAxisType| abs[axis.0 as usize].value;
            tracker.slot(value(AbsoluteAxisType::ABS_MT_SLOT));
            tracker.tracking_id(value(AbsoluteAxisType::ABS_MT_TRACKING_ID));
            tracker.position_x(value(AbsoluteAxisType::ABS_MT_POSITION_X));
            tracker.position_y(value(AbsoluteAxisType::ABS_MT_POSITION_Y));
        }
        Err(e) => log::debug!("cannot read axis state: {e}"),
    }
}

fn read_loop(mut dev: RawDevice, mut tracker: MtTracker, frames: Sender<TouchFrame>, name: &str) {
    let mut events: Vec<InputEvent> = Vec::with_capacity(64);
    // After SYN_DROPPED everything up to the next SYN_REPORT is unreliable.
    let mut dropping = false;
    loop {
        events.clear();
        match dev.fetch_events() {
            Ok(batch) => events.extend(batch),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                log::warn!("{name}: stopped reading: {e}");
                return;
            }
        }
        for ev in &events {
            match ev.kind() {
                InputEventKind::Synchronization(Synchronization::SYN_DROPPED) => {
                    log::debug!("{name}: event buffer overrun, resynchronising");
                    dropping = true;
                }
                InputEventKind::Synchronization(Synchronization::SYN_REPORT) => {
                    if dropping {
                        dropping = false;
                        tracker.reset();
                        resync(&dev, &mut tracker);
                    }
                    if let Some(frame) = tracker.sync(now_ms()) {
                        if frames.send(frame).is_err() {
                            log::debug!("{name}: frame receiver gone, reader exiting");
                            return;
                        }
                    }
                }
                _ if dropping => {}
                InputEventKind::AbsAxis(axis) => match axis {
                    AbsoluteAxisType::ABS_MT_SLOT => tracker.slot(ev.value()),
                    AbsoluteAxisType::ABS_MT_TRACKING_ID => tracker.tracking_id(ev.value()),
                    AbsoluteAxisType::ABS_MT_POSITION_X => tracker.position_x(ev.value()),
                    AbsoluteAxisType::ABS_MT_POSITION_Y => tracker.position_y(ev.value()),
                    _ => {}
                },
                InputEventKind::Key(key) => {
                    if let Some(&(_, n)) = TOOL_KEYS.iter().find(|(k, _)| *k == key) {
                        tracker.tool_key(n, ev.value() != 0);
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_nodes_sort_numerically() {
        let mut v: Vec<PathBuf> =
            ["/dev/input/event10", "/dev/input/event2", "/dev/input/eventX"].iter().map(PathBuf::from).collect();
        v.sort_by_key(|p| event_number(p));
        assert_eq!(v[0], PathBuf::from("/dev/input/event2"));
        assert_eq!(v[2], PathBuf::from("/dev/input/eventX"));
    }

    #[test]
    fn tool_keys_count_one_to_five() {
        let counts: Vec<u8> = TOOL_KEYS.iter().map(|&(_, n)| n).collect();
        assert_eq!(counts, vec![1, 2, 3, 4, 5]);
    }
}
