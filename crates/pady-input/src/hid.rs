//! OS-independent parts of reading a HID precision touchpad.
//!
//! A Windows Precision Touchpad reports each finger in its own HID *link
//! collection* with X/Y, a contact id and a tip switch. Reports come in one
//! of two shapes:
//!
//! * **parallel**: one report holds every finger;
//! * **hybrid**: the device has fewer finger slots than fingers it can
//!   sense, so a frame is split across several reports. The first carries
//!   the total *contact count*, the rest carry a count of 0.
//!
//! [`FrameAssembler`] stitches either shape back into whole frames.

/// Value range of one axis as described by the HID report descriptor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisCaps {
    pub logical_min: i32,
    pub logical_max: i32,
    pub physical_min: i32,
    pub physical_max: i32,
    /// HID unit code (low nibble = system: 1 SI linear [cm], 3 English linear [inch]).
    pub units: u32,
    /// HID unit exponent, a 4-bit two's-complement code (0xE = -2).
    pub unit_exp: u32,
}

impl AxisCaps {
    /// Physical length of the axis in millimetres, if the descriptor says.
    pub fn length_mm(&self) -> Option<f32> {
        if self.physical_max <= self.physical_min {
            return None;
        }
        let span = span(self.physical_min, self.physical_max);
        let exp = decode_exponent(self.unit_exp);
        let mm_per_unit = match self.units & 0xF {
            1 => 10.0, // centimetre
            3 => 25.4, // inch
            _ => return None,
        };
        let mm = span * 10f32.powi(exp) * mm_per_unit;
        // Reject nonsense (bad descriptors exist): touchpads are 2–40 cm wide.
        (5.0..=400.0).contains(&mm).then_some(mm)
    }

    /// Map a raw logical value to millimetres along an axis of `length_mm`.
    pub fn to_mm(&self, value: i32, length_mm: f32) -> f32 {
        let offset = (i64::from(value) - i64::from(self.logical_min)) as f32;
        (offset / span(self.logical_min, self.logical_max)).clamp(0.0, 1.0) * length_mm
    }
}

/// Width of a descriptor range, at least 1. Computed in 64 bits: devices
/// may describe ranges as wide as `i32::MIN..=i32::MAX`.
fn span(min: i32, max: i32) -> f32 {
    (i64::from(max) - i64::from(min)).max(1) as f32
}

/// HID stores the unit exponent as a signed nibble: 0..7 → 0..7, 8..F → -8..-1.
fn decode_exponent(code: u32) -> i32 {
    let n = (code & 0xF) as i32;
    if n >= 8 {
        n - 16
    } else {
        n
    }
}

/// Physical pad size, falling back to a typical 100 mm wide pad with the
/// logical aspect ratio when the descriptor has no physical units.
pub fn pad_size_mm(x: &AxisCaps, y: &AxisCaps) -> (f32, f32) {
    match (x.length_mm(), y.length_mm()) {
        (Some(w), Some(h)) => (w, h),
        _ => {
            let aspect = span(y.logical_min, y.logical_max) / span(x.logical_min, x.logical_max);
            // A nonsense descriptor still gets a pad of plausible proportions.
            (100.0, (100.0 * aspect).clamp(20.0, 200.0))
        }
    }
}

/// One finger slot as read from a single report.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlotReading {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    /// Finger is touching the surface.
    pub tip: bool,
    /// The device thinks this is a finger, not a palm (true if unsupported).
    pub confident: bool,
}

/// Most contacts a frame may claim (see [`FrameAssembler::push`]).
const MAX_CONTACTS: usize = 32;

#[derive(Debug, Default)]
pub struct FrameAssembler {
    expected: usize,
    seen: usize,
    touching: Vec<SlotReading>,
}

impl FrameAssembler {
    /// Feed one report. `contact_count` is the report's Contact Count usage
    /// (0 for hybrid continuation reports); `slots` are its finger slots in
    /// order. Returns the touching, confident fingers once a frame is whole.
    pub fn push(&mut self, contact_count: u32, slots: &[SlotReading]) -> Option<Vec<SlotReading>> {
        if contact_count > 0 {
            // Real pads report at most 5-10 contacts; a bogus count must not
            // keep a frame open (and growing) forever.
            self.expected = (contact_count as usize).min(MAX_CONTACTS);
            self.seen = 0;
            self.touching.clear();
        } else if self.expected == 0 {
            return None; // continuation report with no frame in progress
        }
        let take = (self.expected - self.seen).min(slots.len());
        for slot in &slots[..take] {
            if slot.tip && slot.confident {
                self.touching.push(*slot);
            }
        }
        self.seen += take;
        if self.seen >= self.expected {
            self.expected = 0;
            Some(std::mem::take(&mut self.touching))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis(lmax: i32, pmax: i32, units: u32, exp: u32) -> AxisCaps {
        AxisCaps { logical_min: 0, logical_max: lmax, physical_min: 0, physical_max: pmax, units, unit_exp: exp }
    }

    fn slot(id: u32, tip: bool) -> SlotReading {
        SlotReading { id, x: id as i32 * 100, y: 50, tip, confident: true }
    }

    #[test]
    fn decodes_centimetres_and_inches() {
        // 1050 × 10^-2 cm = 10.5 cm = 105 mm
        assert_eq!(axis(4000, 1050, 0x11, 0xE).length_mm(), Some(105.0));
        // 400 × 10^-2 in = 4 in = 101.6 mm
        let mm = axis(4000, 400, 0x13, 0xE).length_mm().unwrap();
        assert!((mm - 101.6).abs() < 0.01);
    }

    #[test]
    fn missing_or_absurd_units_fall_back() {
        assert_eq!(axis(4000, 0, 0x11, 0xE).length_mm(), None);
        assert_eq!(axis(4000, 1050, 0x00, 0xE).length_mm(), None);
        assert_eq!(axis(4000, 1050, 0x11, 0x0).length_mm(), None, "10.5 m is not a touchpad");
        let (w, h) = pad_size_mm(&axis(3000, 0, 0, 0), &axis(2000, 0, 0, 0));
        assert_eq!((w, h.round()), (100.0, 67.0));
    }

    #[test]
    fn maps_logical_to_mm_and_clamps() {
        let a = axis(1000, 0, 0, 0);
        assert_eq!(a.to_mm(500, 80.0), 40.0);
        assert_eq!(a.to_mm(-20, 80.0), 0.0);
        assert_eq!(a.to_mm(5000, 80.0), 80.0);
    }

    #[test]
    fn parallel_report_is_one_frame() {
        let mut fa = FrameAssembler::default();
        let frame = fa.push(3, &[slot(1, true), slot(2, true), slot(3, true), slot(0, false), slot(0, false)]).unwrap();
        assert_eq!(frame.iter().map(|s| s.id).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn hybrid_reports_are_stitched() {
        let mut fa = FrameAssembler::default();
        // Device with 2 slots reporting 4 fingers over 2 reports.
        assert_eq!(fa.push(4, &[slot(1, true), slot(2, true)]), None);
        let frame = fa.push(0, &[slot(3, true), slot(4, true)]).unwrap();
        assert_eq!(frame.len(), 4);
        // Stray continuation is ignored.
        assert_eq!(fa.push(0, &[slot(9, true)]), None);
    }

    #[test]
    fn lift_report_yields_empty_frame() {
        let mut fa = FrameAssembler::default();
        assert_eq!(fa.push(1, &[slot(1, false), slot(0, false)]), Some(vec![]));
    }

    #[test]
    fn palms_are_dropped() {
        let mut fa = FrameAssembler::default();
        let palm = SlotReading { confident: false, ..slot(2, true) };
        assert_eq!(fa.push(2, &[slot(1, true), palm]).unwrap().len(), 1);
    }

    fn random_axis(rng: &mut crate::Rng) -> AxisCaps {
        AxisCaps {
            logical_min: rng.i32(),
            logical_max: rng.i32(),
            physical_min: rng.i32(),
            physical_max: rng.i32(),
            units: rng.next() as u32,
            unit_exp: rng.next() as u32,
        }
    }

    /// Descriptors and reports come from the device: no values may panic,
    /// sizes stay finite and positive, and frames stay bounded.
    #[test]
    fn fuzzed_descriptors_and_reports_never_panic() {
        let mut rng = crate::Rng::new(0x9E37_79B9_7F4A_7C15);
        for _ in 0..20_000 {
            let (x, y) = (random_axis(&mut rng), random_axis(&mut rng));
            let (w, h) = pad_size_mm(&x, &y);
            assert!(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0, "{x:?} {y:?} gave {w}×{h}");
            let mm = x.to_mm(rng.i32(), w);
            assert!((0.0..=w).contains(&mm), "{mm} outside 0..={w}");
        }
        let mut fa = FrameAssembler::default();
        for _ in 0..50_000 {
            let slots: Vec<SlotReading> = (0..rng.below(12))
                .map(|_| SlotReading {
                    id: rng.next() as u32,
                    x: rng.i32(),
                    y: rng.i32(),
                    tip: rng.below(2) == 0,
                    confident: rng.below(4) > 0,
                })
                .collect();
            let count = [0, 0, 0, 1, 2, 3, 5, 10, 1000, u32::MAX][rng.below(10) as usize];
            if let Some(touching) = fa.push(count, &slots) {
                assert!(touching.len() <= MAX_CONTACTS);
            }
            assert!(fa.touching.len() <= MAX_CONTACTS, "a bogus contact count kept a frame growing");
        }
    }
}
