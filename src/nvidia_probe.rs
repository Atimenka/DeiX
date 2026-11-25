//! Read-only PCI resource description for future NVIDIA bring-up.
//!
//! This does not enable decoding, map registers, power on a GPU, or submit
//! commands. BAR size cannot be obtained without destructive BAR probing.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryBar {
    pub base: u64,
    pub is_64_bit: bool,
    pub prefetchable: bool,
}

/// Decode BAR0 from already-read configuration space. Reject I/O, reserved
/// encodings, unassigned addresses and the all-ones device-absent response.
pub fn decode_bar0(low: u32, high: u32) -> Option<MemoryBar> {
    if low == u32::MAX || low & 1 != 0 { return None; }
    let is_64_bit = match (low >> 1) & 3 {
        0 => false,
        2 => true,
        _ => return None,
    };
    if is_64_bit && high == u32::MAX { return None; }
    let base = (low as u64 & !0xf) | if is_64_bit { (high as u64) << 32 } else { 0 };
    if base == 0 { return None; }
    Some(MemoryBar { base, is_64_bit, prefetchable: low & 8 != 0 })
}

/// Known GT 520M PCI IDs only identify product families, not verified silicon
/// revisions. Use BOOT0 later only after safe mapping and power management.
pub fn is_gt_520m(device_id: u16) -> bool {
    matches!(device_id, 0x0ded | 0x0df7 | 0x1050 | 0x1052)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bar32_and_bar64() {
        assert_eq!(decode_bar0(0xf600_0000, 0xffff_ffff), Some(MemoryBar {
            base: 0xf600_0000, is_64_bit: false, prefetchable: false,
        }));
        assert_eq!(decode_bar0(0x2000_000c, 0x12), Some(MemoryBar {
            base: 0x12_2000_0000, is_64_bit: true, prefetchable: true,
        }));
    }
    #[test]
    fn reject_unsafe_or_unassigned() {
        for (low, high) in [(u32::MAX, 0), (1, 0), (2, 0), (6, 0), (0, 0), (4, u32::MAX)] {
            assert_eq!(decode_bar0(low, high), None);
        }
        assert!(decode_bar0(4, 1).is_some());
    }
    #[test]
    fn matches_known_ids_only() {
        for id in [0x0ded, 0x0df7, 0x1050, 0x1052] { assert!(is_gt_520m(id)); }
        for id in [0x1051, 0, 0xffff] { assert!(!is_gt_520m(id)); }
    }
}
