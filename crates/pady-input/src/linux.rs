//! Linux backend: reads touchpads through evdev (`/dev/input/event*`).
//!
//! Devices are never grabbed, so the desktop keeps pointer motion, clicks
//! and scrolling; Pady only listens alongside it. Each touchpad gets a reader
//! thread that feeds raw events into an [`MtTracker`] and forwards its frames.
//!
//! Hotplug: a watcher thread rescans `/dev/input` every [`RESCAN_INTERVAL`]
//! for touchpads plugged in later (USB, Bluetooth), and a reader whose device
//! goes away removes itself from the device list. Everything winds down once
//! a frame cannot be sent because the receiver is gone.

use crate::mt::{AxisRange, MtTracker};
use crate::{DeviceInfo, InputError, InputHandle};
use crossbeam_channel::Sender;
use evdev::raw_stream::RawDevice;
use evdev::{AbsoluteAxisType, InputEvent, InputEventKind, Key, PropType, Synchronization};
use pady_core::{now_ms, TouchFrame};
use std::collections::HashMap;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

const PERMISSION_HINT: &str =
    "add your user to the 'input' group: sudo usermod -aG input $USER, then log out and back in";

const RESCAN_INTERVAL: Duration = Duration::from_secs(3);

/// `ENODEV`: the device was unplugged (the same on every Linux architecture).
const ENODEV: i32 = 19;

/// Tool keys and the finger count each one announces.
const TOOL_KEYS: [(Key, u8); 5] = [
    (Key::BTN_TOOL_FINGER, 1),
    (Key::BTN_TOOL_DOUBLETAP, 2),
    (Key::BTN_TOOL_TRIPLETAP, 3),
    (Key::BTN_TOOL_QUADTAP, 4),
    (Key::BTN_TOOL_QUINTTAP, 5),
];

/// State shared by the watcher and the reader threads.
struct Backend {
    /// What [`InputHandle::devices`] reports.
    devices: Arc<Mutex<Vec<DeviceInfo>>>,
    /// Device nodes that have a running reader.
    reading: Mutex<Vec<PathBuf>>,
    /// Set when a frame could not be sent: the receiver is gone, so readers
    /// stop at their next event and the watcher at its next rescan.
    stopped: AtomicBool,
}

/// Identifies what is behind a device node name, which the kernel reuses: a
/// node recreated for another device gets a new inode, and one whose
/// permissions udev changed gets a new ctime.
type NodeStamp = (u64, i64, i64);

#[derive(Default)]
struct ScanResult {
    started: usize,
    denied: usize,
}

pub(crate) fn start(frames: Sender<TouchFrame>) -> Result<InputHandle, InputError> {
    let backend =
        Arc::new(Backend { devices: Arc::default(), reading: Mutex::default(), stopped: AtomicBool::new(false) });
    let mut checked = HashMap::new();
    let found =
        scan(&backend, &frames, &mut checked).map_err(|e| InputError::Os(format!("cannot list /dev/input: {e}")))?;
    if found.started == 0 {
        return Err(if found.denied > 0 {
            log::warn!("{} input device(s) could not be opened (permission denied)", found.denied);
            InputError::Permission(PERMISSION_HINT.into())
        } else {
            InputError::NoTouchpad
        });
    }

    let devices = backend.devices.clone();
    let watcher =
        std::thread::Builder::new().name("pady-evdev-watch".into()).spawn(move || watch(&backend, &frames, checked));
    if let Err(e) = watcher {
        log::warn!("cannot start the touchpad hotplug watcher: {e}");
    }
    Ok(InputHandle::new(devices))
}

fn watch(backend: &Arc<Backend>, frames: &Sender<TouchFrame>, mut checked: HashMap<PathBuf, NodeStamp>) {
    loop {
        std::thread::sleep(RESCAN_INTERVAL);
        if backend.stopped.load(Ordering::Relaxed) {
            log::debug!("frame receiver gone, touchpad watcher exiting");
            return;
        }
        if let Err(e) = scan(backend, frames, &mut checked) {
            log::debug!("cannot rescan /dev/input: {e}");
        }
    }
}

/// Start a reader for every touchpad node that has none. Nodes found not to
/// be usable touchpads are remembered in `checked` and only opened again
/// when the node changes, so rescans do not keep opening every input device.
fn scan(
    backend: &Arc<Backend>,
    frames: &Sender<TouchFrame>,
    checked: &mut HashMap<PathBuf, NodeStamp>,
) -> io::Result<ScanResult> {
    // Not `evdev::enumerate()`: it silently skips nodes it cannot open, and
    // a permission error is the most common reason no touchpad shows up.
    let mut nodes: Vec<(PathBuf, NodeStamp)> = std::fs::read_dir("/dev/input")?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("event")))
        .filter_map(|p| {
            let meta = std::fs::metadata(&p).ok()?;
            Some((p, (meta.ino(), meta.ctime(), meta.ctime_nsec())))
        })
        .collect();
    nodes.sort_by_key(|(p, _)| event_number(p));
    checked.retain(|path, _| nodes.iter().any(|(p, _)| p == path));

    let mut result = ScanResult::default();
    for (path, stamp) in nodes {
        if checked.get(&path) == Some(&stamp) || lock(&backend.reading).contains(&path) {
            continue;
        }
        match RawDevice::open(&path) {
            Ok(dev) => {
                if let Some((info, tracker)) = probe(&dev) {
                    // Not remembered in `checked`: if the reader stops while
                    // the node remains, the next scan tries it again.
                    if spawn_reader(backend, frames, path, dev, info, tracker) {
                        result.started += 1;
                    }
                    continue;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => result.denied += 1,
            Err(e) => log::debug!("skipping {}: {e}", path.display()),
        }
        checked.insert(path, stamp);
    }
    Ok(result)
}

fn event_number(path: &Path) -> u32 {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("event"))
        .and_then(|n| n.parse().ok())
        .unwrap_or(u32::MAX)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
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
            // Axes the device lacks read as 0, i.e. a finger for the tool type.
            let value = |axis: AbsoluteAxisType| abs[axis.0 as usize].value;
            tracker.slot(value(AbsoluteAxisType::ABS_MT_SLOT));
            tracker.tool_type(value(AbsoluteAxisType::ABS_MT_TOOL_TYPE));
            tracker.tracking_id(value(AbsoluteAxisType::ABS_MT_TRACKING_ID));
            tracker.position_x(value(AbsoluteAxisType::ABS_MT_POSITION_X));
            tracker.position_y(value(AbsoluteAxisType::ABS_MT_POSITION_Y));
        }
        Err(e) => log::debug!("cannot read axis state: {e}"),
    }
}

/// A running reader's entry in the device list and in [`Backend::reading`],
/// removed when the reader ends (or never starts).
struct Registration {
    backend: Arc<Backend>,
    path: PathBuf,
    info: DeviceInfo,
}

impl Drop for Registration {
    fn drop(&mut self) {
        lock(&self.backend.reading).retain(|p| *p != self.path);
        let mut devices = lock(&self.backend.devices);
        if let Some(i) = devices.iter().position(|d| *d == self.info) {
            devices.remove(i);
        }
    }
}

fn spawn_reader(
    backend: &Arc<Backend>,
    frames: &Sender<TouchFrame>,
    path: PathBuf,
    dev: RawDevice,
    info: DeviceInfo,
    tracker: MtTracker,
) -> bool {
    log::info!(
        "touchpad {} at {}: {:.0}x{:.0} mm, {} slots, up to {} fingers",
        info.name,
        path.display(),
        info.width_mm,
        info.height_mm,
        tracker.slot_count(),
        info.max_contacts
    );
    lock(&backend.reading).push(path.clone());
    lock(&backend.devices).push(info.clone());
    let registration = Registration { backend: backend.clone(), path, info };
    let thread_name = format!("pady-evdev-{}", event_number(&registration.path));
    let frames = frames.clone();
    // If the thread cannot start, the closure is dropped and so is the registration.
    let spawned = std::thread::Builder::new().name(thread_name).spawn(move || {
        read_loop(dev, tracker, &frames, &registration);
    });
    match spawned {
        Ok(_) => true,
        Err(e) => {
            log::warn!("cannot start a touchpad reader thread: {e}");
            false
        }
    }
}

fn read_loop(mut dev: RawDevice, mut tracker: MtTracker, frames: &Sender<TouchFrame>, reg: &Registration) {
    let name = reg.info.name.as_str();
    let stopped = &reg.backend.stopped;
    let mut events: Vec<InputEvent> = Vec::with_capacity(64);
    // After SYN_DROPPED everything up to the next SYN_REPORT is unreliable.
    let mut dropping = false;
    loop {
        events.clear();
        match dev.fetch_events() {
            Ok(batch) => events.extend(batch),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.raw_os_error() == Some(ENODEV) => {
                log::info!("touchpad {name} at {} was removed", reg.path.display());
                return;
            }
            Err(e) => {
                log::warn!("{name}: stopped reading: {e}");
                return;
            }
        }
        if stopped.load(Ordering::Relaxed) {
            return;
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
                            stopped.store(true, Ordering::Relaxed);
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
                    AbsoluteAxisType::ABS_MT_TOOL_TYPE => tracker.tool_type(ev.value()),
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

    #[test]
    fn registration_removes_its_entry_when_dropped() {
        let backend =
            Arc::new(Backend { devices: Arc::default(), reading: Mutex::default(), stopped: AtomicBool::new(false) });
        let info = |name: &str| DeviceInfo { name: name.into(), width_mm: 100.0, height_mm: 60.0, max_contacts: 5 };
        // Two identical pads and another one.
        let mut regs = Vec::new();
        for (n, name) in ["pad", "pad", "other"].into_iter().enumerate() {
            let path = PathBuf::from(format!("/dev/input/event{n}"));
            lock(&backend.reading).push(path.clone());
            lock(&backend.devices).push(info(name));
            regs.push(Registration { backend: backend.clone(), path, info: info(name) });
        }
        drop(regs.remove(0));
        assert_eq!(*lock(&backend.devices), vec![info("pad"), info("other")]);
        assert_eq!(
            *lock(&backend.reading),
            vec![PathBuf::from("/dev/input/event1"), PathBuf::from("/dev/input/event2")]
        );
        regs.clear();
        assert!(lock(&backend.devices).is_empty());
        assert!(lock(&backend.reading).is_empty());
    }
}
