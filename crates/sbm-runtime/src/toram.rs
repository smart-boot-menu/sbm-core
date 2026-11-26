//! Adaptive RAM-resident root.
//!
//! `MemTotal` on a machine sold as 4 GiB is usually below 4 GiB because the
//! kernel excludes firmware reservations. The auto threshold is therefore
//! 3584 MiB of `MemTotal` (the 4 GiB class), and the copy still requires the
//! image plus 1536 MiB of headroom to fit. A 2 GiB machine stays on the
//! backing device. The initrd script `image/initrd-extra/usr/lib/sbm/sbm-toram.sh`
//! must use the same constants.

use serde::{Deserialize, Serialize};

/// `MemTotal` at or above this value is treated as a 4 GiB-class machine.
pub const TORAM_AUTO_MEMTOTAL_KIB: u64 = 3584 * 1024;

/// RAM that must remain after the EROFS image is copied.
pub const TORAM_HEADROOM_BYTES: u64 = 1536 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToramDecision {
    /// Root bytes were (or should be) copied into RAM. The backing disk may be released.
    Resident,
    /// Root stays on the backing device. Destroying that device must be refused.
    BackingProtected,
}

pub fn decide_toram(mode: &str, mem_total_kib: u64, image_bytes: u64) -> ToramDecision {
    let mode = mode.trim();
    if mode == "never" || image_bytes == 0 {
        return ToramDecision::BackingProtected;
    }
    let mem_bytes = mem_total_kib.saturating_mul(1024);
    let need = image_bytes.saturating_add(TORAM_HEADROOM_BYTES);
    if mem_bytes <= need {
        return ToramDecision::BackingProtected;
    }
    if mode == "always" {
        return ToramDecision::Resident;
    }
    if (mode == "auto" || mode.is_empty()) && mem_total_kib >= TORAM_AUTO_MEMTOTAL_KIB {
        ToramDecision::Resident
    } else {
        ToramDecision::BackingProtected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB_KIB: u64 = 1024;

    #[test]
    fn two_gib_machine_stays_on_the_backing_device() {
        let mem_kib = 1900 * MIB_KIB;
        assert_eq!(
            decide_toram("auto", mem_kib, 800 * 1024 * 1024),
            ToramDecision::BackingProtected
        );
    }

    #[test]
    fn four_gib_class_copies_when_the_image_fits() {
        let mem_kib = 3700 * MIB_KIB;
        assert!(mem_kib >= TORAM_AUTO_MEMTOTAL_KIB);
        assert_eq!(decide_toram("auto", mem_kib, GIB), ToramDecision::Resident);
    }

    #[test]
    fn four_gib_class_refuses_an_image_that_would_not_leave_headroom() {
        let mem_kib = 3700 * MIB_KIB;
        assert_eq!(
            decide_toram("auto", mem_kib, 3 * GIB),
            ToramDecision::BackingProtected
        );
    }

    #[test]
    fn always_still_requires_the_image_to_fit() {
        assert_eq!(
            decide_toram("always", 8 * 1024 * MIB_KIB, GIB),
            ToramDecision::Resident
        );
        assert_eq!(
            decide_toram("always", 1900 * MIB_KIB, GIB),
            ToramDecision::BackingProtected
        );
    }

    #[test]
    fn never_and_unknown_modes_protect_the_backing_device() {
        assert_eq!(
            decide_toram("never", 8 * 1024 * MIB_KIB, GIB),
            ToramDecision::BackingProtected
        );
        assert_eq!(
            decide_toram("yes", 8 * 1024 * MIB_KIB, GIB),
            ToramDecision::BackingProtected
        );
    }

    #[test]
    fn initrd_script_uses_the_same_constants() {
        let src = include_str!("../../../image/initrd-extra/usr/lib/sbm/sbm-toram.sh");
        assert!(src.contains("AUTO_MIB=3584"));
        assert!(src.contains("HEADROOM_MIB=1536"));
        assert!(src.contains("resident"));
        assert!(src.contains("backing-protected"));
    }
}
