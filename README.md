# Minimal Immutable Linux Management & Recovery OS (`sbm-core`)

Independently bootable, immutable, modular, RAM-capable minimal Linux management & recovery environment built with `mkosi v27.1`.

> **Project Status:** SBM 0.1 image configuration, daemon, and Iced shell. Compressed-size targets in ADR-003 are targets. Measured sizes are written down only after a build.

## Scope & Responsibilities

- Unified `mkosi` build recipes producing signed Unified Kernel Images (UKIs) + `dm-verity`-verified EROFS (`zstd`) root filesystems for generic x86-64 and ARM64 PCs on Fedora 45.
- Apple Silicon uses the same pipeline with the delta in [docs/apple-silicon-profile.md](docs/apple-silicon-profile.md). That profile is not built: `kernel-16k` was absent from the Fedora 45 repositories queried on 2026-11-26.
- Adaptive RAM mode when `MemTotal` is at least 3584 MiB and the image plus 1536 MiB of headroom fits. Smaller machines keep the root on the backing disk and protect that disk.
- Wayland desktop (`labwc` + XWayland for GParted + a tiny-skia fallback + `wayvnc` left disabled + the Iced management shell).
- `sbm-cored` for read-only disk discovery, `DiskIdentity` matching, and `StorageOperationPlan` checks. SBM 0.1 does not execute partition changes.
- `sbm_runtime::inspect_catalog_plan` compares an `OsInstallPlan` from `sbm-catalog` with the `OsCatalogEntry` it names. An unbound plan with an empty operation list can pass inspection and stays non-executable. A hash mismatch or a storage operation on the unbound disk is rejected. See [docs/CATALOG.md](docs/CATALOG.md).

## Canonical Documentation & Contracts

- Central coordination & roadmap: [`smart-boot-menu/sbm`](https://github.com/smart-boot-menu/sbm)
- Normative specifications & ADRs: [`smart-boot-menu/docs`](https://github.com/smart-boot-menu/docs)
- Primary-source technical research: [`smart-boot-menu/research`](https://github.com/smart-boot-menu/research)
- Shared Rust types & JSON Schemas: [`smart-boot-menu/sbm-common`](https://github.com/smart-boot-menu/sbm-common)
- Agent guide: [AGENTS.md](AGENTS.md)

## License

Original code and documentation in this repository are dual-licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE), subject to third-party licensing requirements for bundled external components.
