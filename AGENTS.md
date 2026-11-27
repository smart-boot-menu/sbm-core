# AGENTS.md — `sbm-core`

## Canonical Organization Policy (Read First)

Before designing or implementing changes, read the canonical organization-wide engineering rules in [`smart-boot-menu/.github/AGENTS.md`](https://github.com/smart-boot-menu/.github/blob/main/AGENTS.md) (local workspace: `../.github/AGENTS.md`), the operational checklists in [`.github/agent-reference.md`](https://github.com/smart-boot-menu/.github/blob/main/agent-reference.md) (`../.github/agent-reference.md`), and the **Open-Source Reuse & Engineering Principles** in [`ADR-010`](https://github.com/smart-boot-menu/docs/blob/main/architecture-decisions/ADR-010-open-source-reuse-and-engineering-principles.md) (`../docs/architecture-decisions/ADR-010-open-source-reuse-and-engineering-principles.md`).

> **Standalone / Single-Repo Agents:** If you were launched inside this repository alone and `../.github/AGENTS.md` is not present locally, fetch and follow the GitHub URLs above before proceeding. Do not recreate inferior versions of existing software; apply proportional build-versus-reuse evaluation for new components, major dependencies, architectural changes, or replacements.

## Assigned Repository & Role

- **Repository:** `smart-boot-menu/sbm-core`
- **Primary Dependencies & Specs:** `sbm-common`, `sbm-catalog`, `docs/specifications/03..06`, `ADR-001`, `ADR-002`, `ADR-003`, `ADR-006`

## Responsibilities (What You Own)

- Unified `mkosi` build recipes producing signed Unified Kernel Images (UKIs) + `dm-verity`-verified EROFS (`zstd`) root filesystems:
  - **x86-64 & Generic ARM64 PCs:** Fedora Minimal (~420–650 MiB compressed).
  - **Apple Silicon:** Super-light Fedora Asahi Minimal (`kernel-16k` `linux-asahi`, ~500–850 MiB compressed — far under 4 GiB).
- Adaptive RAM mode (`sbm.toram=auto` on ≥ 4 GiB RAM; direct EROFS + `zram` overlay on 2 GiB older laptops).
- Wayland desktop (`labwc` + XWayland for GTK3 GParted `1.8.1` + `llvmpipe` fallback + `wayvnc` remote desktop + native Rust `Iced` management UI).
- Rust safety orchestrator (`sbm-cored`) for read-only disk discovery, `DiskIdentity` matching, `StorageOperationPlan` validation, verified OS installation (`bootc`, rootfs, `wimlib-imagex`, `kexec`), `partclone` backup/cloning, and hardware diagnostics.

## Strict Boundaries (Do NOT Duplicate or Violate)

Do NOT rewrite mature filesystem tools (`e2fsprogs`, `ntfs-3g`, `partclone`, `wimlib`, `gparted`, `cryptsetup`) in Rust; invoke via `execve` without shell interpolation. Do NOT install/uninstall SBM itself (`install`).

## Engineering & Performance Targets

Compressed image ≤ 650 MiB (x86/ARM64 PC) / ≤ 900 MiB (Apple Silicon); cold boot to desktop ≤ 10 s on HVF/KVM; usable on 2 GiB RAM laptops.

*(Targets are goals, not claims; follow `../docs/testing/benchmarks.md` and archive real measurements in `../research/validation/evidence/`.)*

## Security, Testing & Interface Coordination

- Enforce all invariants in `../docs/threat-model/README.md` (offline-first, no secrets committed, bounded timeouts, safe storage plans).
- Run formatting, clippy, unit tests, and integration hooks before committing.
- Never silently diverge from `sbm-common` types or `docs/specifications/`; follow the Interface Change Protocol in `../.github/agent-reference.md`.
