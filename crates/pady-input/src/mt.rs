//! The Linux multitouch slot protocol (type B), turned into [`TouchFrame`]s.
//!
//! The kernel describes each finger in a numbered *slot*: `ABS_MT_SLOT`
//! selects a slot, `ABS_MT_TRACKING_ID` starts (>= 0) or ends (-1) a contact
//! in it, `ABS_MT_POSITION_X/Y` move it, `ABS_MT_TOOL_TYPE` says whether it
//! is a finger or a palm, and `SYN_REPORT` closes a packet. Values are only
//! sent when they change, so the tracker keeps every slot's last state.
//! [`MtTracker`] is fed those values one by one and yields a frame, in
//! millimetres, whenever a packet changed the set of contacts.
//!
//! This module knows nothing about evdev types so it is unit tested on every
//! platform; `linux.rs` only translates event codes into calls here.

use pady_core::{Contact, TouchFrame};

/// One absolute axis as reported by `EVIOCGABS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AxisRange {
    pub min: i32,
    pub max: i32,
    /// Units per millimetre; 0 when the driver does not know.
    pub resolution: i32,
}

/// Assumed pad width when the driver reports no resolution on either axis.
/// Typical laptop pads are 90-120 mm wide, and every threshold in the
/// recognizer is in millimetres, so a plausible guess beats raw units.
pub const FALLBACK_WIDTH_MM: f32 = 100.0;

/// `ABS_MT_TOOL_TYPE` value for a palm (`MT_TOOL_PALM` in the kernel).
pub const MT_TOOL_PALM: i32 = 2;

/// Ids for contacts adopted after a [`MtTracker::reset`], whose real tracking
/// id was lost. Kernel tracking ids fit in 16 bits, so these never collide.
const ADOPTED_ID_BASE: u32 = 0x4000_0000;
/// Ids for contacts synthesized from `BTN_TOOL_*TAP` (see [`MtTracker`]).
const SYNTHETIC_ID_BASE: u32 = 0x8000_0000;
/// Horizontal spacing of synthesized contacts around the real centroid.
const SYNTHETIC_SPACING_MM: f32 = 8.0;

/// Linear mapping from device units on one axis to millimetres.
#[derive(Clone, Copy, Debug)]
struct Scale {
    min: i32,
    units_per_mm: f32,
    extent_mm: f32,
}

impl Scale {
    fn new(range: AxisRange, units_per_mm: f32) -> Self {
        Scale { min: range.min, units_per_mm, extent_mm: span(range) / units_per_mm }
    }

    fn mm(self, value: i32) -> f32 {
        let mm = (value as f64 - self.min as f64) as f32 / self.units_per_mm;
        // Some drivers report slightly outside their advertised range.
        mm.clamp(0.0, self.extent_mm)
    }
}

fn span(range: AxisRange) -> f32 {
    (range.max as i64 - range.min as i64).max(1) as f32
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum SlotState {
    /// No finger (the slot's tracking id is -1).
    Free,
    Active(u32),
    /// State unknown: at start-up or after a buffer overrun. The slot becomes
    /// active if position updates arrive for it, because the kernel never
    /// sends positions for an empty slot.
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    state: SlotState,
    /// Last reported position and tool. Kept across contacts because the
    /// kernel does not repeat a value that did not change, even for a new
    /// finger.
    x: Option<i32>,
    y: Option<i32>,
    palm: bool,
}

/// Rebuilds the set of touching fingers from multitouch type B events.
///
/// Output frames are in millimetres from the top-left corner (y grows
/// downward) and [`Contact::id`] is the kernel tracking id, stable while the
/// finger stays down.
///
/// Contacts the driver marks as palms ([`MT_TOOL_PALM`], e.g. touches the
/// firmware flags as not confident) are left out, so a resting hand does not
/// add a finger or block a gesture. A finger that turns into a palm looks
/// like a lift; one that turns back reappears with its tracking id.
///
/// # Pads with fewer slots than fingers
///
/// Many touchpads track only two slots (or, with `INPUT_PROP_SEMI_MT`, the
/// two corners of a bounding box) yet report the real finger count through
/// `BTN_TOOL_TRIPLETAP` / `QUADTAP` / `QUINTTAP`. When that count exceeds the
/// active slots, the missing fingers are synthesized: copies of the real
/// contacts' centroid, spread [`SYNTHETIC_SPACING_MM`] apart horizontally,
/// with ids that stay the same while the count does. They therefore move
/// exactly with the real fingers, which is what swipe, drag and tap
/// detection look at. Synthesis needs at least one real contact to anchor it,
/// and palms count as reported slots: drivers differ on whether the tool
/// keys include them, and a palm must never come back as a made-up finger.
#[derive(Clone, Debug)]
pub struct MtTracker {
    slots: Vec<Slot>,
    current: Option<usize>,
    x: Scale,
    y: Scale,
    /// Bit `n - 1` is set while the tool key for `n` fingers is held.
    tool_mask: u8,
    next_adopted: u32,
    last: Vec<Contact>,
}

impl MtTracker {
    /// `slots` is `ABS_MT_SLOT`'s maximum + 1.
    ///
    /// If an axis has no resolution, the other axis's resolution is assumed
    /// (square units). If neither has one, the pad is assumed to be
    /// [`FALLBACK_WIDTH_MM`] wide with the aspect ratio of the two ranges.
    pub fn new(slots: usize, x: AxisRange, y: AxisRange) -> Self {
        let (rx, ry) = match (x.resolution > 0, y.resolution > 0) {
            (true, true) => (x.resolution as f32, y.resolution as f32),
            (true, false) => (x.resolution as f32, x.resolution as f32),
            (false, true) => (y.resolution as f32, y.resolution as f32),
            (false, false) => {
                let r = span(x) / FALLBACK_WIDTH_MM;
                (r, r)
            }
        };
        MtTracker {
            slots: vec![Slot::default(); slots.max(1)],
            current: Some(0),
            x: Scale::new(x, rx),
            y: Scale::new(y, ry),
            tool_mask: 0,
            next_adopted: 0,
            last: Vec::new(),
        }
    }

    pub fn width_mm(&self) -> f32 {
        self.x.extent_mm
    }

    pub fn height_mm(&self) -> f32 {
        self.y.extent_mm
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// `ABS_MT_SLOT`. Events for a slot beyond the device's range are ignored
    /// until a valid slot is selected again.
    pub fn slot(&mut self, index: i32) {
        self.current = usize::try_from(index).ok().filter(|&i| i < self.slots.len());
    }

    /// `ABS_MT_TRACKING_ID`: a negative id lifts the finger in the current
    /// slot; a new id (even without a lift in between) starts a new contact.
    pub fn tracking_id(&mut self, id: i32) {
        if let Some(slot) = self.current_slot() {
            slot.state = match u32::try_from(id) {
                Ok(id) => SlotState::Active(id),
                Err(_) => SlotState::Free,
            };
        }
    }

    /// `ABS_MT_POSITION_X`.
    pub fn position_x(&mut self, value: i32) {
        if let Some(slot) = self.current_slot() {
            slot.x = Some(value);
        }
        self.adopt_current();
    }

    /// `ABS_MT_POSITION_Y`.
    pub fn position_y(&mut self, value: i32) {
        if let Some(slot) = self.current_slot() {
            slot.y = Some(value);
        }
        self.adopt_current();
    }

    /// `ABS_MT_TOOL_TYPE`: contacts in a slot whose tool is [`MT_TOOL_PALM`]
    /// are left out of frames until the slot reports another tool.
    pub fn tool_type(&mut self, tool: i32) {
        if let Some(slot) = self.current_slot() {
            slot.palm = tool == MT_TOOL_PALM;
        }
    }

    /// `BTN_TOOL_FINGER` (1) .. `BTN_TOOL_QUINTTAP` (5) pressed or released.
    /// The kernel may press the new key before releasing the old one within a
    /// packet, so the largest held key wins.
    pub fn tool_key(&mut self, fingers: u8, pressed: bool) {
        if !(1..=8).contains(&fingers) {
            return;
        }
        let bit = 1u8 << (fingers - 1);
        if pressed {
            self.tool_mask |= bit;
        } else {
            self.tool_mask &= !bit;
        }
    }

    /// Set the tool finger count directly (0 when no tool key is held).
    pub fn tool_fingers(&mut self, fingers: u8) {
        self.tool_mask = 0;
        self.tool_key(fingers, true);
    }

    /// Fingers the device says are down according to its `BTN_TOOL_*` keys.
    pub fn tool_count(&self) -> u8 {
        (u8::BITS - self.tool_mask.leading_zeros()) as u8
    }

    /// Forget per-slot state after `SYN_DROPPED`: events were lost, so any
    /// slot may have lifted or landed. Every slot becomes unknown (re-adopted
    /// when it reports movement, with its last known position and tool) and
    /// the tool keys are cleared; the caller should then restore what it can
    /// query from the device (current slot, its tracking id, tool and
    /// position, held tool keys). A gesture in progress is cut short by the
    /// resulting lift rather than misread.
    pub fn reset(&mut self) {
        for slot in &mut self.slots {
            slot.state = SlotState::Unknown;
        }
        self.tool_mask = 0;
    }

    /// `SYN_REPORT`: returns a frame if the contacts differ from the last
    /// frame returned, including the change to no contacts at all.
    pub fn sync(&mut self, time_ms: u64) -> Option<TouchFrame> {
        let contacts = self.contacts();
        if contacts == self.last {
            return None;
        }
        self.last.clone_from(&contacts);
        Some(TouchFrame { time_ms, contacts, width_mm: self.width_mm(), height_mm: self.height_mm() })
    }

    fn current_slot(&mut self) -> Option<&mut Slot> {
        self.current.and_then(|i| self.slots.get_mut(i))
    }

    fn adopt_current(&mut self) {
        let id = ADOPTED_ID_BASE | (self.next_adopted & !SYNTHETIC_ID_BASE & !ADOPTED_ID_BASE);
        if let Some(slot) = self.current_slot() {
            if slot.state == SlotState::Unknown && slot.x.is_some() && slot.y.is_some() {
                slot.state = SlotState::Active(id);
                self.next_adopted = self.next_adopted.wrapping_add(1);
            }
        }
    }

    fn contacts(&self) -> Vec<Contact> {
        let mut contacts = Vec::new();
        // Touches the slots account for, palms included.
        let mut touching = 0;
        for slot in &self.slots {
            if let (SlotState::Active(id), Some(x), Some(y)) = (slot.state, slot.x, slot.y) {
                touching += 1;
                if !slot.palm {
                    contacts.push(Contact { id, x: self.x.mm(x), y: self.y.mm(y) });
                }
            }
        }

        let real = contacts.len();
        let missing = (self.tool_count() as usize).saturating_sub(touching);
        if real > 0 && missing > 0 {
            let cx = contacts.iter().map(|c| c.x).sum::<f32>() / real as f32;
            let cy = contacts.iter().map(|c| c.y).sum::<f32>() / real as f32;
            for k in 0..missing {
                // Alternate right and left of the centroid: +s, -s, +2s, -2s...
                let step = (k / 2 + 1) as f32 * SYNTHETIC_SPACING_MM;
                let dx = if k % 2 == 0 { step } else { -step };
                contacts.push(Contact {
                    id: SYNTHETIC_ID_BASE + k as u32,
                    x: (cx + dx).clamp(0.0, self.width_mm()),
                    y: cy,
                });
            }
        }
        contacts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 100 x 50 mm pad at 10 units/mm, 5 slots.
    fn tracker() -> MtTracker {
        let x = AxisRange { min: 0, max: 1000, resolution: 10 };
        let y = AxisRange { min: 0, max: 500, resolution: 10 };
        MtTracker::new(5, x, y)
    }

    fn land(t: &mut MtTracker, slot: i32, id: i32, x: i32, y: i32) {
        t.slot(slot);
        t.tracking_id(id);
        t.position_x(x);
        t.position_y(y);
    }

    fn lift(t: &mut MtTracker, slot: i32) {
        t.slot(slot);
        t.tracking_id(-1);
    }

    fn ids(f: &TouchFrame) -> Vec<u32> {
        f.contacts.iter().map(|c| c.id).collect()
    }

    #[test]
    fn single_finger_in_millimetres() {
        let mut t = tracker();
        assert_eq!(t.width_mm(), 100.0);
        assert_eq!(t.height_mm(), 50.0);
        land(&mut t, 0, 7, 250, 100);
        let f = t.sync(10).unwrap();
        assert_eq!(f.time_ms, 10);
        assert_eq!(f.contacts, vec![Contact { id: 7, x: 25.0, y: 10.0 }]);
        assert_eq!((f.width_mm, f.height_mm), (100.0, 50.0));

        t.position_x(300);
        let f = t.sync(20).unwrap();
        assert_eq!(f.contacts, vec![Contact { id: 7, x: 30.0, y: 10.0 }]);

        lift(&mut t, 0);
        let f = t.sync(30).unwrap();
        assert!(f.contacts.is_empty(), "the lift must produce an empty frame");
        assert!(t.sync(40).is_none());
    }

    #[test]
    fn offset_ranges_are_measured_from_the_minimum() {
        let x = AxisRange { min: 1000, max: 5000, resolution: 40 };
        let y = AxisRange { min: -200, max: 2800, resolution: 40 };
        let mut t = MtTracker::new(2, x, y);
        assert_eq!((t.width_mm(), t.height_mm()), (100.0, 75.0));
        land(&mut t, 0, 1, 1000, -200);
        assert_eq!(t.sync(0).unwrap().contacts[0], Contact { id: 1, x: 0.0, y: 0.0 });
        // Out-of-range reports are clamped onto the pad.
        t.position_x(6000);
        assert_eq!(t.sync(1).unwrap().contacts[0].x, 100.0);
    }

    #[test]
    fn three_fingers_land_move_and_lift() {
        let mut t = tracker();
        land(&mut t, 0, 10, 100, 100);
        land(&mut t, 1, 11, 200, 100);
        land(&mut t, 2, 12, 300, 100);
        let f = t.sync(0).unwrap();
        assert_eq!(f.finger_count(), 3);
        assert_eq!(ids(&f), vec![10, 11, 12]);

        // All three move down 5 mm in one packet.
        for s in 0..3 {
            t.slot(s);
            t.position_y(150);
        }
        let f = t.sync(8).unwrap();
        assert!(f.contacts.iter().all(|c| c.y == 15.0));
        assert_eq!(ids(&f), vec![10, 11, 12]);

        // Fingers leave one packet at a time.
        lift(&mut t, 1);
        assert_eq!(ids(&t.sync(16).unwrap()), vec![10, 12]);
        lift(&mut t, 0);
        lift(&mut t, 2);
        assert!(t.sync(24).unwrap().contacts.is_empty());
    }

    #[test]
    fn no_frame_when_nothing_changed() {
        let mut t = tracker();
        assert!(t.sync(0).is_none(), "empty to empty is not a change");
        land(&mut t, 0, 1, 100, 100);
        assert!(t.sync(1).is_some());
        t.position_x(100); // same value
        assert!(t.sync(2).is_none());
    }

    #[test]
    fn slot_reuse_keeps_unrepeated_coordinates() {
        let mut t = tracker();
        land(&mut t, 0, 1, 100, 200);
        t.sync(0);
        lift(&mut t, 0);
        assert!(t.sync(1).unwrap().contacts.is_empty());
        // A new finger lands in the same slot at the same y: the kernel only
        // sends what changed.
        t.slot(0);
        t.tracking_id(2);
        t.position_x(400);
        let f = t.sync(2).unwrap();
        assert_eq!(f.contacts, vec![Contact { id: 2, x: 40.0, y: 20.0 }]);
    }

    #[test]
    fn tracking_id_change_in_same_slot_is_a_new_contact() {
        let mut t = tracker();
        land(&mut t, 3, 5, 100, 100);
        assert_eq!(ids(&t.sync(0).unwrap()), vec![5]);
        t.slot(3);
        t.tracking_id(6);
        assert_eq!(ids(&t.sync(1).unwrap()), vec![6]);
    }

    #[test]
    fn slot_selection_persists_and_bad_slots_are_ignored() {
        let mut t = tracker();
        land(&mut t, 1, 1, 100, 100);
        t.sync(0);
        // No ABS_MT_SLOT: events go to the last selected slot.
        t.position_x(200);
        assert_eq!(t.sync(1).unwrap().contacts[0].x, 20.0);
        t.slot(9);
        t.tracking_id(4);
        t.position_x(500);
        t.slot(-1);
        t.tracking_id(-1);
        assert!(t.sync(2).is_none());
    }

    #[test]
    fn resolution_zero_assumes_100mm_wide_and_keeps_aspect() {
        let x = AxisRange { min: 0, max: 3000, resolution: 0 };
        let y = AxisRange { min: 0, max: 2000, resolution: 0 };
        let mut t = MtTracker::new(5, x, y);
        assert_eq!(t.width_mm(), FALLBACK_WIDTH_MM);
        assert!((t.height_mm() - 66.666_67).abs() < 1e-3);
        land(&mut t, 0, 1, 1500, 1000);
        let c = t.sync(0).unwrap().contacts[0];
        assert_eq!(c.x, 50.0);
        assert!((c.y - 33.333_33).abs() < 1e-3);
    }

    #[test]
    fn one_missing_resolution_borrows_the_other() {
        let x = AxisRange { min: 0, max: 1200, resolution: 12 };
        let y = AxisRange { min: 0, max: 600, resolution: 0 };
        let t = MtTracker::new(2, x, y);
        assert_eq!((t.width_mm(), t.height_mm()), (100.0, 50.0));
    }

    #[test]
    fn semi_mt_synthesizes_missing_fingers() {
        let mut t = MtTracker::new(
            2,
            AxisRange { min: 0, max: 1000, resolution: 10 },
            AxisRange { min: 0, max: 500, resolution: 10 },
        );
        t.tool_key(1, true);
        land(&mut t, 0, 1, 400, 200);
        assert_eq!(t.sync(0).unwrap().finger_count(), 1);

        // Second and third fingers land; only one more slot exists.
        t.tool_key(3, true);
        t.tool_key(1, false);
        land(&mut t, 1, 2, 600, 200);
        let f = t.sync(1).unwrap();
        assert_eq!(f.finger_count(), 3);
        let synth = f.contacts[2];
        assert!(synth.id >= SYNTHETIC_ID_BASE);
        assert_eq!((synth.x, synth.y), (58.0, 20.0)); // centroid (50, 20) + 8 mm

        // Real fingers move 10 mm up: the synthetic one follows, same id.
        t.slot(0);
        t.position_y(100);
        t.slot(1);
        t.position_y(100);
        let g = t.sync(2).unwrap();
        assert_eq!(g.contacts[2], Contact { id: synth.id, x: 58.0, y: 10.0 });

        // Five fingers: synthetic ones alternate around the centroid.
        t.tool_key(5, true);
        t.tool_key(3, false);
        let g = t.sync(3).unwrap();
        let xs: Vec<f32> = g.contacts[2..].iter().map(|c| c.x).collect();
        assert_eq!(xs, vec![58.0, 42.0, 66.0]);

        // Back to two fingers: synthesis stops.
        t.tool_fingers(2);
        assert_eq!(t.sync(4).unwrap().finger_count(), 2);
        // Tool keys alone (no real contact) do not create fingers.
        lift(&mut t, 0);
        lift(&mut t, 1);
        t.tool_fingers(3);
        assert!(t.sync(5).unwrap().contacts.is_empty());
    }

    #[test]
    fn tool_count_is_the_largest_held_key() {
        let mut t = tracker();
        assert_eq!(t.tool_count(), 0);
        t.tool_key(2, true);
        t.tool_key(3, true);
        assert_eq!(t.tool_count(), 3);
        t.tool_key(3, false);
        assert_eq!(t.tool_count(), 2);
        t.tool_fingers(0);
        assert_eq!(t.tool_count(), 0);
        t.tool_key(0, true);
        t.tool_key(9, true);
        assert_eq!(t.tool_count(), 0);
    }

    #[test]
    fn more_slots_than_tool_count_trusts_slots() {
        let mut t = tracker();
        t.tool_fingers(1);
        land(&mut t, 0, 1, 100, 100);
        land(&mut t, 1, 2, 200, 100);
        land(&mut t, 2, 3, 300, 100);
        assert_eq!(t.sync(0).unwrap().finger_count(), 3);
    }

    #[test]
    fn reset_lifts_everything_then_readopts_moving_slots() {
        let mut t = tracker();
        t.tool_fingers(2);
        land(&mut t, 0, 1, 100, 100);
        land(&mut t, 1, 2, 200, 100);
        assert_eq!(t.sync(0).unwrap().finger_count(), 2);

        t.reset();
        assert_eq!(t.tool_count(), 0);
        let f = t.sync(1).unwrap();
        assert!(f.contacts.is_empty(), "a reset reports a lift");

        // Slot 1 is still down and moves; it is adopted with a fresh id and
        // its old y, since only x was resent.
        t.slot(1);
        t.position_x(250);
        let f = t.sync(2).unwrap();
        assert_eq!(f.finger_count(), 1);
        let c = f.contacts[0];
        assert!((ADOPTED_ID_BASE..SYNTHETIC_ID_BASE).contains(&c.id));
        assert_eq!((c.x, c.y), (25.0, 10.0));
        // ...and keeps that id while it moves.
        t.position_y(150);
        assert_eq!(t.sync(3).unwrap().contacts[0].id, c.id);

        // A slot whose lift was lost stays gone; a real tracking id wins.
        t.slot(0);
        t.tracking_id(9);
        let f = t.sync(4).unwrap();
        assert_eq!(f.contacts[0].id, 9);
        assert_eq!(f.contacts[0].x, 10.0);
    }

    #[test]
    fn fingers_down_at_startup_are_adopted_once_positioned() {
        let mut t = tracker();
        t.slot(2);
        t.position_x(100);
        assert!(t.sync(0).is_none(), "needs both coordinates");
        t.position_y(100);
        assert_eq!(t.sync(1).unwrap().finger_count(), 1);
        t.tracking_id(-1);
        assert!(t.sync(2).unwrap().contacts.is_empty());
    }

    #[test]
    fn palms_are_left_out() {
        let mut t = tracker();
        // A palm lands on its own: nothing to report.
        t.slot(0);
        t.tool_type(MT_TOOL_PALM);
        t.tracking_id(1);
        t.position_x(100);
        t.position_y(400);
        assert!(t.sync(0).is_none());

        // Three fingers join it.
        land(&mut t, 1, 2, 300, 100);
        land(&mut t, 2, 3, 400, 100);
        land(&mut t, 3, 4, 500, 100);
        assert_eq!(ids(&t.sync(1).unwrap()), vec![2, 3, 4]);

        // The palm moving changes nothing.
        t.slot(0);
        t.position_x(150);
        assert!(t.sync(2).is_none());
    }

    #[test]
    fn finger_turning_into_a_palm_lifts_and_can_return() {
        let mut t = tracker();
        land(&mut t, 0, 1, 100, 100);
        land(&mut t, 1, 2, 200, 100);
        t.sync(0);
        t.slot(1);
        t.tool_type(MT_TOOL_PALM);
        assert_eq!(ids(&t.sync(1).unwrap()), vec![1]);
        t.tool_type(0); // MT_TOOL_FINGER
        assert_eq!(ids(&t.sync(2).unwrap()), vec![1, 2]);
    }

    #[test]
    fn palm_tool_type_persists_for_the_next_contact_in_the_slot() {
        let mut t = tracker();
        t.slot(0);
        t.tool_type(MT_TOOL_PALM);
        t.tracking_id(1);
        t.position_x(100);
        t.position_y(100);
        t.tracking_id(-1);
        // Another palm in the same slot: the kernel does not resend the tool.
        t.tracking_id(2);
        assert!(t.sync(0).is_none());
        // A finger: the tool changes, so it is sent.
        t.tracking_id(3);
        t.tool_type(0);
        assert_eq!(ids(&t.sync(1).unwrap()), vec![3]);
    }

    #[test]
    fn palms_are_never_replaced_by_synthetic_fingers() {
        let mut t = tracker();
        // Two touches reported by the tool keys: one finger, one palm.
        t.tool_fingers(2);
        land(&mut t, 0, 1, 100, 100);
        t.slot(1);
        t.tool_type(MT_TOOL_PALM);
        t.tracking_id(2);
        t.position_x(500);
        t.position_y(400);
        assert_eq!(ids(&t.sync(0).unwrap()), vec![1]);
        // A third touch the slots cannot show is still synthesized.
        t.tool_fingers(3);
        let f = t.sync(1).unwrap();
        assert_eq!(f.finger_count(), 2);
        assert!(f.contacts[1].id >= SYNTHETIC_ID_BASE);
    }
}
