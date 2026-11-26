//! Policy and parsing shared by `sbm-cored` and the Iced shell.
//!
//! Discovery and plan checks live here so they can be tested on the development
//! host without root, a display, or a built image.

#![forbid(unsafe_code)]

pub mod catalog;
pub mod discover;
pub mod handoff;
pub mod hostinfo;
pub mod ipc;
pub mod plan;
pub mod toram;

pub use catalog::inspect_catalog_plan;
pub use discover::{DiscoveredDisk, DiscoveredPartition, InstalledOs, parse_lsblk};
pub use handoff::{HandoffLoad, launch_mode_label, load_handoff};
pub use ipc::{
    ClientRequest, ClientResponse, LaunchTool, PowerAction, SOCKET_PATH, ToolReport, transact,
};
pub use plan::{EXECUTION_ENABLED_IN_0_1, PlanDecision, validate_plan};
pub use toram::{TORAM_AUTO_MEMTOTAL_KIB, TORAM_HEADROOM_BYTES, ToramDecision, decide_toram};

pub const SBM_VERSION: &str = "0.1.0";
pub const SLOT_A_TARGET_ID: &str = "sbm-core:slot-a";

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
