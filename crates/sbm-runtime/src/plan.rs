//! Storage plan checks for the 0.1 inspection milestone.
//!
//! A plan can be accepted as well-formed and still not be executed. SBM 0.1
//! does not perform destructive disk changes.

use sbm_types::{SbmErrorCode, StorageOp, StorageOperationPlan};
use serde::{Deserialize, Serialize};

pub const EXECUTION_ENABLED_IN_0_1: bool = false;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanDecision {
    pub accepted: bool,
    pub execution_enabled: bool,
    pub code: Option<SbmErrorCode>,
    pub message: String,
}

pub fn validate_plan(
    plan: &StorageOperationPlan,
    backing_protected: bool,
    backing_stable_id: Option<&str>,
) -> PlanDecision {
    if plan.schema_version != "1.0" || plan.plan_id.trim().is_empty() {
        return reject(
            SbmErrorCode::InvalidSchema,
            "plan schema_version must be 1.0 and plan_id must be set",
        );
    }
    if plan.expected_partition_table_sha256.len() != 64
        || !plan
            .expected_partition_table_sha256
            .chars()
            .all(|ch| ch.is_ascii_hexdigit())
    {
        return reject(
            SbmErrorCode::InvalidSchema,
            "expected_partition_table_sha256 must be 64 hex characters",
        );
    }
    if !plan.operations.is_empty() && (!plan.destructive || plan.reversible) {
        return reject(
            SbmErrorCode::InvalidSchema,
            "partition changes are not reversible; destructive must be true and reversible must be false",
        );
    }
    if plan.operations.iter().any(is_mutating) && plan.reversible {
        return reject(
            SbmErrorCode::InvalidSchema,
            "format, delete, resize, clone, and image writes are not reversible",
        );
    }
    if plan.blocks_by_default_safety() {
        return reject(
            SbmErrorCode::ProtectedPartitionBlocked,
            "the plan touches an Apple system partition, an encrypted volume, or the SBM recovery slot",
        );
    }
    if backing_protected && backing_stable_id == Some(plan.target_disk.stable_id.as_str()) {
        return reject(
            SbmErrorCode::ProtectedPartitionBlocked,
            "this disk is the backing device for the running system and RAM-resident mode is not active",
        );
    }
    PlanDecision {
        accepted: true,
        execution_enabled: EXECUTION_ENABLED_IN_0_1,
        code: None,
        message: if plan.operations.is_empty() {
            "plan has no operations".to_string()
        } else {
            "plan passed inspection checks; SBM 0.1 does not execute disk modifications".to_string()
        },
    }
}

fn reject(code: SbmErrorCode, message: &str) -> PlanDecision {
    PlanDecision {
        accepted: false,
        execution_enabled: false,
        code: Some(code),
        message: message.to_string(),
    }
}

fn is_mutating(op: &StorageOp) -> bool {
    matches!(
        op,
        StorageOp::ResizePartition { .. }
            | StorageOp::FormatFilesystem { .. }
            | StorageOp::DeletePartition { .. }
            | StorageOp::WriteImage { .. }
            | StorageOp::ClonePartition { .. }
            | StorageOp::CreatePartition { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sbm_types::{DiskBus, DiskIdentity, PartitionProtectionClass, SCHEMA_VERSION_V1};

    fn disk(id: &str) -> DiskIdentity {
        DiskIdentity {
            stable_id: id.to_string(),
            gpt_disk_guid: Some("11111111-2222-3333-4444-555555555555".to_string()),
            wwn_or_eui64: None,
            model: "QEMU HARDDISK".to_string(),
            serial_hash: None,
            size_bytes: 1024,
            logical_sector_size: 512,
            bus: DiskBus::Virtio,
            removable: false,
            read_only: false,
        }
    }

    fn plan(
        destructive: bool,
        reversible: bool,
        classes: Vec<PartitionProtectionClass>,
    ) -> StorageOperationPlan {
        StorageOperationPlan {
            schema_version: SCHEMA_VERSION_V1.to_string(),
            plan_id: "plan-1".to_string(),
            target_disk: disk("gpt:11111111-2222-3333-4444-555555555555"),
            expected_partition_table_sha256: "ab".repeat(32),
            operations: vec![StorageOp::DeletePartition {
                partition_guid: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
            }],
            destructive,
            reversible,
            touched_protected_classes: classes,
            dry_run_summary: vec!["delete partition".to_string()],
        }
    }

    #[test]
    fn honest_plan_is_accepted_but_not_executed() {
        let decision = validate_plan(&plan(true, false, vec![]), false, None);
        assert!(decision.accepted);
        assert!(!decision.execution_enabled);
    }

    #[test]
    fn reversible_claim_is_rejected() {
        let decision = validate_plan(&plan(true, true, vec![]), false, None);
        assert!(!decision.accepted);
        assert_eq!(decision.code, Some(SbmErrorCode::InvalidSchema));
    }

    #[test]
    fn apple_and_backing_disk_are_blocked() {
        let apple = plan(
            true,
            false,
            vec![PartitionProtectionClass::ProtectedAppleSystem],
        );
        assert_eq!(
            validate_plan(&apple, false, None).code,
            Some(SbmErrorCode::ProtectedPartitionBlocked)
        );
        let backing = plan(true, false, vec![]);
        let decision = validate_plan(
            &backing,
            true,
            Some("gpt:11111111-2222-3333-4444-555555555555"),
        );
        assert!(!decision.accepted);
        assert!(decision.message.contains("backing device"));
    }
}
