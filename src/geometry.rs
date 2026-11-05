//! The dock's resting layout and hit-testing. Pure functions, unit-tested.
//!
//! The Slint UI magnifies icons with a mapping that never moves the point under the pointer,
//! so a click can be resolved against this resting layout.

use crate::config::{sanitize_icon_size, Entry};

/// Space between neighbouring items, in logical pixels.
pub const GAP: f32 = 8.0;
/// Width of a separator's slot.
pub const SEPARATOR_WIDTH: f32 = 6.0;

/// The resting position of one entry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    /// Left edge, measured from the start of the dock.
    pub u: f32,
    /// Width of the slot (icon size, or [`SEPARATOR_WIDTH`] for separators).
    pub width: f32,
    /// True for separator slots.
    pub separator: bool,
}

/// Lays the entries out in a row. Returns every slot and the total width.
///
/// `icon_size` is sanitised first: a non-finite or out-of-range size (which would
/// otherwise push `NaN` into every coordinate and make the whole dock vanish) is
/// replaced with the built-in default.
pub fn layout(entries: &[Entry], icon_size: f32) -> (Vec<Slot>, f32) {
    let icon_size = sanitize_icon_size(icon_size, crate::config::DEFAULT_ICON_SIZE);
    let mut slots = Vec::with_capacity(entries.len());
    let mut u = 0.0_f32;
    for entry in entries {
        let width = if entry.separator {
            SEPARATOR_WIDTH
        } else {
            icon_size
        };
        slots.push(Slot {
            u,
            width,
            separator: entry.separator,
        });
        u += width + GAP;
    }
    let total = if slots.is_empty() { 0.0 } else { u - GAP };
    (slots, total)
}

/// The entry under pointer position `p` (same coordinates as `Slot::u`), if any.
/// Half of the gap on each side belongs to the neighbouring icon, so there are no dead zones.
/// Separator slots are never hit: clicking a divider does nothing.
pub fn hit(slots: &[Slot], p: f32) -> Option<usize> {
    slots.iter().position(|slot| {
        !slot.separator && p >= slot.u - GAP / 2.0 && p < slot.u + slot.width + GAP / 2.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(separator: bool) -> Entry {
        Entry {
            name: "x".to_string(),
            icon: String::new(),
            exec: String::new(),
            matches: None,
            separator,
        }
    }

    fn sample() -> Vec<Entry> {
        vec![entry(false), entry(false), entry(true), entry(false)]
    }

    // ---------------------------------------------------------------- layout

    #[test]
    fn slots_are_spaced_by_the_gap() {
        let (slots, total) = layout(&sample(), 56.0);
        assert_eq!(slots[0].u, 0.0);
        assert_eq!(slots[1].u, 64.0);
        assert_eq!(slots[2].u, 128.0);
        assert_eq!(slots[2].width, SEPARATOR_WIDTH);
        assert_eq!(slots[3].u, 142.0);
        assert_eq!(total, 198.0);
    }

    #[test]
    fn an_empty_dock_lays_out_to_nothing() {
        let (slots, total) = layout(&[], 56.0);
        assert!(slots.is_empty());
        assert_eq!(total, 0.0);
    }

    #[test]
    fn a_single_icon_has_no_gap_around_it() {
        let (slots, total) = layout(&[entry(false)], 56.0);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].u, 0.0);
        assert_eq!(slots[0].width, 56.0);
        assert_eq!(total, 56.0);
    }

    #[test]
    fn separators_use_their_own_width() {
        let (slots, _) = layout(&[entry(true)], 56.0);
        assert_eq!(slots[0].width, SEPARATOR_WIDTH);
        assert!(slots[0].separator);
    }

    #[test]
    fn positions_increase_monotonically() {
        let (slots, _) = layout(&sample(), 64.0);
        for pair in slots.windows(2) {
            assert!(pair[0].u < pair[1].u, "slots must be ordered: {:?}", slots);
        }
    }

    #[test]
    fn a_non_finite_icon_size_falls_back_to_the_default() {
        let (slots, total) = layout(&sample(), f32::NAN);
        let (expected_slots, expected_total) = layout(&sample(), crate::config::DEFAULT_ICON_SIZE);
        assert_eq!(slots, expected_slots);
        assert_eq!(total, expected_total);
    }

    #[test]
    fn an_infinite_icon_size_falls_back_to_the_default() {
        let (slots, _) = layout(&sample(), f32::INFINITY);
        let (expected_slots, _) = layout(&sample(), crate::config::DEFAULT_ICON_SIZE);
        assert_eq!(slots, expected_slots);
    }

    #[test]
    fn an_out_of_range_icon_size_is_clamped() {
        let (slots, _) = layout(&sample(), 4.0);
        let (expected_slots, _) = layout(&sample(), crate::config::MIN_ICON_SIZE);
        assert_eq!(slots, expected_slots);
    }

    #[test]
    fn every_coordinate_is_finite() {
        // Even with hostile input, no NaN may reach the UI.
        let (slots, total) = layout(&sample(), f32::NAN);
        assert!(total.is_finite());
        for slot in &slots {
            assert!(slot.u.is_finite() && slot.width.is_finite());
        }
    }

    // ------------------------------------------------------------------- hit

    #[test]
    fn hit_testing_gives_each_icon_half_the_gap_on_both_sides() {
        let (slots, _) = layout(&sample(), 56.0);
        assert_eq!(hit(&slots, 28.0), Some(0));
        assert_eq!(hit(&slots, 59.9), Some(0));
        assert_eq!(hit(&slots, 60.0), Some(1));
        assert_eq!(hit(&slots, 130.0), None);
        assert_eq!(hit(&slots, 170.0), Some(3));
        assert_eq!(hit(&slots, -10.0), None);
    }

    #[test]
    fn the_left_edge_is_inclusive_and_the_right_edge_exclusive() {
        let (slots, _) = layout(&[entry(false)], 56.0);
        assert_eq!(hit(&slots, -4.0), Some(0)); // u - GAP/2 = -4
        assert_eq!(hit(&slots, -4.001), None);
        assert_eq!(hit(&slots, 59.9), Some(0)); // u + width + GAP/2 = 60
        assert_eq!(hit(&slots, 60.0), None);
    }

    #[test]
    fn the_gap_between_two_icons_has_no_dead_zone() {
        // Icon 0's hit range is [-4, 60) and icon 1's is [60, 124): the boundary
        // point belongs to the later icon, and no position in between misses.
        let (slots, _) = layout(&[entry(false), entry(false)], 56.0);
        assert_eq!(hit(&slots, 59.999), Some(0));
        assert_eq!(hit(&slots, 60.0), Some(1));
    }

    #[test]
    fn a_separator_slot_is_never_hit() {
        let entries = vec![entry(false), entry(true), entry(false)];
        let (slots, _) = layout(&entries, 56.0);
        // The separator rests at u = 64 with width 6.
        assert_eq!(hit(&slots, 67.0), None);
    }

    #[test]
    fn the_ungapped_halves_around_a_separator_are_dead() {
        let entries = vec![entry(false), entry(true), entry(false)];
        let (slots, _) = layout(&entries, 56.0);
        // Half-gaps around the separator (60..64 and 70..74) belong to nobody.
        assert_eq!(hit(&slots, 62.0), None);
        assert_eq!(hit(&slots, 72.0), None);
    }

    #[test]
    fn beyond_the_last_icons_half_gap_nothing_is_hit() {
        let (slots, _) = layout(&sample(), 56.0);
        // The last icon (u=142, w=56) is clickable up to 142 + 56 + 4 = 202.
        assert_eq!(hit(&slots, 199.0), Some(3));
        assert_eq!(hit(&slots, 202.0), None);
        assert_eq!(hit(&slots, 1000.0), None);
    }

    #[test]
    fn an_empty_dock_hits_nothing() {
        assert_eq!(hit(&[], 0.0), None);
        assert_eq!(hit(&[], 100.0), None);
    }

    #[test]
    fn a_negative_position_hits_nothing() {
        let (slots, _) = layout(&sample(), 56.0);
        assert_eq!(hit(&slots, f32::NEG_INFINITY), None);
    }
}
