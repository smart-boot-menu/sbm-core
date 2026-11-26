//! Inspection of a catalog install plan.
//!
//! `sbm-catalog` emits `OsCatalogEntry` plus an `OsInstallPlan` whose disk is
//! `unbound` and whose operation list is empty. This module checks that the
//! plan names the same release and artifact hash as the entry. It does not
//! download an image and it does not write a disk.

use sbm_types::{OsCatalogEntry, OsInstallPlan, SbmErrorCode};

use crate::plan::{PlanDecision, validate_plan};

pub fn inspect_catalog_plan(entry: &OsCatalogEntry, plan: &OsInstallPlan) -> PlanDecision {
    if entry.schema_version != "1.0" || plan.schema_version != "1.0" {
        return reject(
            SbmErrorCode::InvalidSchema,
            "catalog entry and install plan schema_version must be 1.0",
        );
    }
    if plan.catalog_entry_id != entry.id {
        return reject(
            SbmErrorCode::TargetNotFound,
            "install plan catalog_entry_id does not match the catalog entry",
        );
    }
    let Some(release) = entry
        .releases
        .iter()
        .find(|release| release.version == plan.catalog_release_version)
    else {
        return reject(
            SbmErrorCode::TargetNotFound,
            "install plan names a release version that is not in the catalog entry",
        );
    };
    if !release.architectures.contains(&plan.target_arch) {
        return reject(
            SbmErrorCode::UnsupportedPlatform,
            "install plan architecture is not in the catalog release",
        );
    }
    if plan.artifact_verification.is_empty() {
        return reject(
            SbmErrorCode::SignatureInvalid,
            "install plan has no artifact hash to compare with the catalog",
        );
    }
    for artifact in &plan.artifact_verification {
        let matched = release.artifacts.iter().any(|candidate| {
            candidate.filename == artifact.filename
                && candidate.size_bytes == artifact.size_bytes
                && candidate.sha256.eq_ignore_ascii_case(&artifact.sha256)
        });
        if !matched {
            return reject(
                SbmErrorCode::SignatureInvalid,
                "install plan artifact hash does not match the catalog entry",
            );
        }
    }
    if plan.storage_plan.target_disk.stable_id == "unbound"
        && !plan.storage_plan.operations.is_empty()
    {
        return reject(
            SbmErrorCode::DiskIdentityMismatch,
            "an unbound catalog plan cannot name storage operations",
        );
    }
    let decision = validate_plan(&plan.storage_plan, false, None);
    if decision.accepted && plan.storage_plan.target_disk.stable_id == "unbound" {
        return PlanDecision {
            accepted: true,
            execution_enabled: false,
            code: None,
            message: "unbound catalog plan passed inspection and is not executable".to_string(),
        };
    }
    decision
}

fn reject(code: SbmErrorCode, message: &str) -> PlanDecision {
    PlanDecision {
        accepted: false,
        execution_enabled: false,
        code: Some(code),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sbm_types::{
        BootRegistrationPlan, CpuArch, DiskBus, DiskIdentity, InstallStrategy, OsCatalogEntry,
        OsFamily, OsInstallPlan, OsReleaseEntry, ReleaseChannel, SCHEMA_VERSION_V1, StorageOp,
        StorageOperationPlan, VerifiedArtifactRef,
    };

    fn entry() -> OsCatalogEntry {
        OsCatalogEntry {
            schema_version: SCHEMA_VERSION_V1.to_string(),
            id: "debian".to_string(),
            name: "Debian".to_string(),
            os_family: OsFamily::Linux,
            vendor: "Debian".to_string(),
            homepage_url: "https://www.debian.org/".to_string(),
            releases: vec![OsReleaseEntry {
                version: "13.8.0".to_string(),
                channel: ReleaseChannel::Stable,
                eol_date: None,
                architectures: vec![CpuArch::Aarch64],
                supported_platforms: vec![],
                min_ram_mib: 512,
                min_disk_gib: 10,
                install_strategy: InstallStrategy::DirectInstallerBoot,
                artifacts: vec![sbm_types::CatalogArtifact {
                    filename: "debian-13.8.0-arm64-netinst.iso".to_string(),
                    size_bytes: 737456128,
                    sha256: "f4e05f67b5c74e3ff7b24c2a8f889960f9eb31fe0c1ac8513d27d5656958b6b3"
                        .to_string(),
                    sha512: None,
                    upstream_signature_url: None,
                    mirrors: vec![
                        "https://cdimage.debian.org/debian-cd/13.8.0/arm64/iso-cd/".to_string(),
                    ],
                    user_supplied_only: false,
                }],
                recipe_version: "1.0".to_string(),
            }],
        }
    }

    fn install_plan(sha: &str, operations: Vec<StorageOp>) -> OsInstallPlan {
        OsInstallPlan {
            schema_version: SCHEMA_VERSION_V1.to_string(),
            plan_id: "sbm-catalog:debian:13.8.0:aarch64".to_string(),
            catalog_entry_id: "debian".to_string(),
            catalog_release_version: "13.8.0".to_string(),
            target_arch: CpuArch::Aarch64,
            strategy: InstallStrategy::DirectInstallerBoot,
            artifact_verification: vec![VerifiedArtifactRef {
                filename: "debian-13.8.0-arm64-netinst.iso".to_string(),
                size_bytes: 737456128,
                sha256: sha.to_string(),
                source_url_or_path: "https://example.invalid/debian.iso".to_string(),
            }],
            storage_plan: StorageOperationPlan {
                schema_version: SCHEMA_VERSION_V1.to_string(),
                plan_id: "sbm-catalog-storage:debian:unbound".to_string(),
                target_disk: DiskIdentity {
                    stable_id: "unbound".to_string(),
                    gpt_disk_guid: None,
                    wwn_or_eui64: None,
                    model: "unbound".to_string(),
                    serial_hash: None,
                    size_bytes: 0,
                    logical_sector_size: 512,
                    bus: DiskBus::Unknown,
                    removable: false,
                    read_only: true,
                },
                expected_partition_table_sha256: "0".repeat(64),
                destructive: !operations.is_empty(),
                reversible: operations.is_empty(),
                operations,
                touched_protected_classes: vec![],
                dry_run_summary: vec!["inspection only".to_string()],
            },
            boot_registration: BootRegistrationPlan {
                register_bls_or_efi: false,
                set_as_default: false,
                display_name: Some("Debian installer".to_string()),
            },
            offline_media_only: false,
        }
    }

    #[test]
    fn unbound_catalog_plan_is_inspection_only() {
        let sha = "f4e05f67b5c74e3ff7b24c2a8f889960f9eb31fe0c1ac8513d27d5656958b6b3";
        let decision = inspect_catalog_plan(&entry(), &install_plan(sha, vec![]));
        assert!(decision.accepted);
        assert!(!decision.execution_enabled);
        assert!(decision.message.contains("not executable"));
    }

    #[test]
    fn hash_mismatch_and_unbound_writes_are_rejected() {
        let wrong = "ab".repeat(32);
        let mismatch = inspect_catalog_plan(&entry(), &install_plan(&wrong, vec![]));
        assert_eq!(mismatch.code, Some(SbmErrorCode::SignatureInvalid));
        let write = inspect_catalog_plan(
            &entry(),
            &install_plan(
                "f4e05f67b5c74e3ff7b24c2a8f889960f9eb31fe0c1ac8513d27d5656958b6b3",
                vec![StorageOp::DeletePartition {
                    partition_guid: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
                }],
            ),
        );
        assert_eq!(write.code, Some(SbmErrorCode::DiskIdentityMismatch));
        assert!(!write.execution_enabled);
    }
}
