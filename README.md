# Minimal Immutable Linux Management & Recovery OS (`sbm-core`)

Independently bootable, immutable, modular, RAM-capable minimal Linux management & recovery environment built with `mkosi v27.1`.

> **Project Status:** Phase 0 initialized — ready for dedicated engineering agent implementation targeting **SBM 0.1**.

## Scope & Responsibilities

- Unified `mkosi` build recipes producing signed Unified Kernel Images (UKIs) + `dm-verity`-verified EROFS (`zstd`) root filesystems:
  - **x86-64 & Generic ARM64 PCs:** Fedora Minimal (~420–650 MiB compressed).
  - **Apple Silicon:** the shared mkosi pipeline with the delta in [docs/apple-silicon-profile.md](docs/apple-silicon-profile.md). `kernel-16k` and the ~500–850 MiB figure are the historical target names, not a measured image.
- Adaptive RAM mode (`sbm.toram=auto` on ≥ 4 GiB RAM; direct EROFS + `zram` overlay on 2 GiB older laptops).
- Wayland desktop (`labwc` + XWayland for GTK3 GParted `1.8.1` + `llvmpipe` fallback + `wayvnc` remote desktop + native Rust `Iced` management UI).
- Rust safety orchestrator (`sbm-cored`) for read-only disk discovery, `DiskIdentity` matching, `StorageOperationPlan` validation, verified OS installation (`bootc`, rootfs, `wimlib-imagex`, `kexec`), `partclone` backup/cloning, and hardware diagnostics.

## Canonical Documentation & Contracts

- Central coordination & roadmap: [`smart-boot-menu/sbm`](https://github.com/smart-boot-menu/sbm)
- Normative specifications & ADRs: [`smart-boot-menu/docs`](https://github.com/smart-boot-menu/docs)
- Primary-source technical research: [`smart-boot-menu/research`](https://github.com/smart-boot-menu/research)
- Shared Rust types & JSON Schemas: [`smart-boot-menu/sbm-common`](https://github.com/smart-boot-menu/sbm-common)
- Agent guide: [AGENTS.md](AGENTS.md)

## License

Original code and documentation in this repository are dual-licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE), subject to third-party licensing requirements for bundled external components.
