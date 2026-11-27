# Agent 13 Handoff — SBM Graphical Recovery Integration (`smart-boot-menu/sbm-core`)

**Date:** 2026-11-26  
**Owner Role:** Agent 13 — SBM Graphical Recovery Integration  
**Primary Repository:** `smart-boot-menu/sbm-core` (commits `f93d4e9`, `10b8f9c`, `5031613`, plus this handoff)  
**Coordinated Repositories:** `smart-boot-menu/sbm-lab`, `smart-boot-menu/install`, `smart-boot-menu/research`  
**Unified `sbm-common` Pin:** `b3c77141504be71bfad6602d8df628f84f704c7b`

> **Scope & Honesty Notice:** All VM verifications below are **`N = 1` functional integration runs** under QEMU/OVMF (`hvf` hardware virtualization for `aarch64`, `tcg` cross-architecture emulation for `x86_64`) on a 16 GiB Apple Silicon host. **They are NOT performance benchmarks and NOT physical-hardware tests.** Physical x86-64 and ARM64 laptop testing remains unverified.

---

## 1. Root-Cause Diagnosis of the Graphical Recovery Failure

Inspecting live serial/journal logs, `sbm-session.service`, `sbm-cored.service`, `sbm-ui.log`, and `Xwayland` revealed that the failure was **not** solely QEMU's `ramfb`/`VGA` display device. Eight concrete defects existed in the integrated Linux recovery stack:

1. **Stale candidate image predating `sysusers.d/sbm.conf` group-before-user fix (`status=217/USER`):** Previously assembled dual-arch Core images were built from `0a49e45` before `c7e9439` added `g sbm 1000`.
2. **Missing `systemd-pam` package (`status=224/PAM`):** Fedora's `labwc` (`libseat`) requires a `systemd-logind` session on `seat0` created via `pam_systemd.so`, which was absent from `mkosi.conf` alongside missing `After/Wants=systemd-logind.service` and `TTYReset`/`XDG_SEAT=seat0`/`XDG_VTNR=1` in `sbm-session.service`.
3. **`RuntimeDirectory=sbm` wiping `/run/sbm` permissions and state:** `sbm-cored.service` declared `RuntimeDirectory=sbm`, causing systemd to reset `/run/sbm` to `root:root` and wipe `/run/sbm/mode`, resulting in `Permission denied` when `sbm-ui` (UID 1000) connected to `/run/sbm/cored.sock`.
4. **Missing `/var/lib/sbm/.config` ownership in `tmpfiles.d/sbm.conf`:** `labwc` config files under `/var/lib/sbm/.config` were not explicitly ensured owned by `sbm:sbm`.
5. **Lazy `Xwayland -terminate 10` dropping `xhost` authorization:** `labwc` launches Xwayland on demand with `-terminate 10`, so boot-time `xhost +si:localuser:root` expired before an operator clicked **Open GParted**. Fixed in `sbm-cored` by refreshing `xhost +si:localuser:root` as UID/GID 1000 immediately before spawning `gpartedbin` (`execve` only; GPL boundary preserved).
6. **Runtime discovery/protection bugs:**
   - `resolve_disk()` did not canonicalize `/dev/mapper/root` before inspecting `/sys/block/<dm>/slaves`, causing backing-disk protection to skip the USB disk.
   - `find_comm()` aborted `/proc` iteration on the first non-numeric entry (e.g. `/proc/acpi`), preventing `labwc` PID/environment lookup.
   - `image/initrd-extra/usr/lib/sbm/sbm-toram.sh` invoked `awk` (absent in initrd), reporting `mem_kib=0`. Replaced with pure POSIX `read` on `/proc/meminfo` + dedicated tmpfs mount.
7. **Boot ESP read-only race (`os error 30` writing `boot-success.json`):** `sbm-protect.service` ran before udev exposed USB ESP metadata (`esp=none`), setting `/dev/sda1` read-only before `boot.mount` mounted `/boot`. Fixed by waiting up to 3 s for boot ESP discovery, enforcing `set_ro(&esp, false)` while locking the 4 non-ESP partitions RO, and remounting `/boot` `rw` in `boot_efi_mount()` if needed.
8. **`x86_64` `virtio-vga` `wgpu` shader panic on Mesa `llvmpipe`:** With `virtio-vga` (no `virgl`), `/dev/dri/renderD128` exists so Iced defaulted to `wgpu` on software `llvmpipe`, panicking in `iced_wgpu.quad.solid.shader` (`Shader requires capability Capabilities(SHADER_FLOAT16_IN_FLOAT32)`). Fixed in `mkosi.extra/var/lib/sbm/.config/labwc/autostart` to automatically switch to `ICED_BACKEND=tiny-skia` on non-zero `sbm-ui` exit and report `software (wgpu fallback: tiny-skia)`.

Additional Spec 03 & UX hardening landed in the same commits:
- Strict boot ESP selection in [`crates/sbm-runtime/src/handoff.rs`](crates/sbm-runtime/src/handoff.rs) (`select_boot_esp`, `decode_efivar_utf16`, `verify_handoff`): supports both Spec 03 volatile EFI vars and legacy cmdline tokens, binds strictly to `esp_partuuid` + VFAT + GPT ESP on the verity backing disk, and rejects decoy ESPs.
- Premature `SBM_DESKTOP_READY` removed from `start-session`; readiness is emitted only after Iced `sbm-ui` renders live daemon-backed system info and calls `confirm_desktop_ready` (`SBM_UI_READY` + fsynced `boot-success.json` -> `SBM_BOOT_SUCCESS_WRITTEN`).
- `PowerAction::BootManager` no longer writes `requested-command.json`, eliminating double-reboot loops.
- `sbm-ui` displays detected disks summary on Home and maximizes via `rc.xml`.
- Updated [`.github/workflows/ci.yml`](.github/workflows/ci.yml) `sbm-common` pin to `b3c77141504be71bfad6602d8df628f84f704c7b` and `cargo test --locked --workspace`.

---

## 2. Reproducible Artifacts & Digests

### Clean `sbm-core` Build (`50316131251bf10fda467fddccc70931978bdc21`, Fedora 45 / mkosi, `SBM_ARCHES="arm64 x86-64"`)

| Artifact | Size (bytes) | SHA-256 |
|---|---:|---|
| `build/images/sbm-core-arm64.raw` | 1,363,530,240 | `dd85499142edf6ea3b0a6e238d720c7c67360963165d78b85c6672adf443606d` |
| `build/images/sbm-core-arm64.efi` | 74,513,752 | `5ba2c2a6dee393516b95b9ac36e5009ed8f797571d303f17cdc5cc7744dd210e` |
| `build/images/sbm-core-arm64.raw.zst` | 923,214,751 | `5f5c1028dc96c225250fbb715f2c6353cfc46f72bdad0934a4836eb5655ab294` |
| `build/images/sbm-core-x86-64.raw` | 1,387,459,072 | `fbfd9e5b9ad39d2fad7d853bd3634fc43eddb34ee4e48e6a06ffa2adff789a28` |
| `build/images/sbm-core-x86-64.efi` | 80,909,144 | `1d5f1011f43decd0f120525865e36d66f5c0ac63179f845e73b6510fa803d9d8` |
| `build/images/sbm-core-x86-64.raw.zst` | 972,400,774 | `61730dd814581284f1448650ba5860f29d1eaf82c15e2df2f7318d661e5baba0` |

### Dual-Architecture Candidate (`install/media/0.0.1.lock.toml`)

Verified bit-for-bit reproducible via `./scripts/check-reproducible.sh media/0.0.1.lock.toml` (all 6 emitted files identical across two fresh builds) and passed full `sbm-install media verify`:

| Artifact | Size (bytes) | SHA-256 |
|---|---:|---|
| `sbm-0.0.1-uefi-dualarch.raw` | 2,216,689,664 | `7be07b851f1e61be2544027c1caaf28febcc385bf01d8978b12738bef3be64a9` |
| `sbm-0.0.1-uefi-dualarch.raw.zst` | 1,756,287,279 | `e18a41b08ac71c01883a73b8d4b94c1ad81be965f972568f93e3dbd8434b23a8` |
| `release-manifest.json` | 772 | `fe8309fe69370b1a8d772b6ce7c289acc4c9d3456924dd55f96ef536d3626e70` |
| `build-receipt.json` | — | `8a17718c47dde4f252cdf4f7df298afad3969d6043df3515ad4c7e6de4407a77` |

- `aarch64` dm-verity roothash: `925e22ebd0dc9ce7fb72e24c1d3787aa648cab7606124570a0ca26d13190b412`
- `x86_64` dm-verity roothash: `d4d79238973794157dba3ddb380d020262d80b228a1b672196c852043f41c5f7`

---

## 3. End-to-End Integration & Negative Verification Results

Automated harness implemented in `smart-boot-menu/sbm-lab` (`sbm-lab test --tier integration --image PATH`): boots a disposable QCOW2 overlay of the unchanged `.raw` image as removable USB (`bootindex=0`) alongside a decoy ESP (`SBMDECOY`, enumerated first as `/dev/vda`) and two blank virtual disks (`1 GiB` `SBMTEST1G` `/dev/vdb`, `2 GiB` `SBMTEST2G` `/dev/vdc`), `virtio-gpu-pci,xres=1280,yres=800` (`aarch64`) / `virtio-vga,xres=1280,yres=800` (`x86_64`), zero NIC, no `-no-reboot`.

### Positive End-to-End Runs (Exact Same `.raw` Image `7be07b85`)

| Architecture | Accel / Class | Run ID | Duration | Gates Passed | Active Renderer | Confirmed Attempt |
|---|---|---|---:|---|---|---|
| `aarch64` | `hvf` (`accelerated_virtualization`) | `int-aarch64-18ebcad4c2d656f8-4847-1` | 61.3 s | **9 / 9** | `auto (DRM render node present)` | `uefi-1727508` (`status=confirmed`, `consecutive_failures=0`, `boot_success_consumed=true`) |
| `x86_64` | `tcg` (`emulation`) | `int-x86_64-18ebcae5732ae478-490e-1` | 159.1 s | **9 / 9** | `software (wgpu fallback: tiny-skia)` | `uefi-16688591` (`status=confirmed`, `consecutive_failures=0`, `boot_success_consumed=true`) |
| `aarch64` (`ramfb`) | `hvf` (`accelerated_virtualization`, 2 GiB RAM) | `software-ramfb` | ~15 s | UI + Handoff + `boot-success.json` | `software (no DRM render node: pixman + tiny-skia)` | `uefi-1666419` (`SBM_BOOT_SUCCESS_WRITTEN`) |

All 12 QMP screenshots are archived in `research/validation/evidence/screenshots/2026-11-26-graphical-recovery/` and visually verified:
- UEFI graphical menu showing both `Fedora Linux (Decoy Fixture)` and `SBM Core Slot A` (`01-uefi-menu.png`)
- Live daemon-backed Iced `sbm-ui` displaying kernel, memory, backing disk `/dev/sda`, all 4 detected disks, verified handoff, renderer mode, and `desktop ready (confirmed ...)` (`02-core-ui.png`)
- Real GParted launched via XWayland displaying `/dev/sda` GPT partitions (`03-gparted.png`) and open device dropdown listing `/dev/sda (2.06 GiB)`, `/dev/vda (49.02 MiB)`, `/dev/vdb (1.00 GiB)`, `/dev/vdc (2.00 GiB)` (`04-gparted-devices.png`)
- Clean guest reboot (`RESET` QMP event) returning to the SBM UEFI menu with `last_attempt.json` confirmed and `boot-success.json` consumed (`05-post-reboot-menu.png`)
- Pure software fallback on `ramfb` (`aarch64-ramfb-software-ui.png`) and fail-closed tampered UKI refusal (`aarch64-negative-tampered-uki.png`).

### Negative Gate Verifications (Disposable Copies, `aarch64` HVF)

1. **Tampered UKI (1 byte flipped in `sbm-core-aarch64.efi`):** `sbm-uefi` tagged `SBM Core Slot A (unverified)`, refused launch on `c`, displayed *"Cannot start this operating system — No healthy recovery system was found"*, and remained at a usable picker (zero chainload, zero Linux boot).
2. **Corrupted dm-verity (4 KiB flipped in `root-arm64-verity`):** Kernel logged `device-mapper: verity: 8:4: metadata block 1 is corrupted` and `reached maximum errors`; never mounted EROFS, never emitted `SBM_UI_READY`, never wrote `boot-success.json`.
3. **Decoy ESP priority:** Enumerated before boot USB as `/dev/vda` in both full integration runs; `decoy_esp_unchanged: true`, Core bound strictly to `/dev/sda1`.
4. **Invalid handoff:** Unit-tested in `crates/sbm-runtime/src/handoff.rs` (mismatched hash, attempt ID, arch, decoy ESP) and observed live on direct boot (`SBM_BOOT_SUCCESS_DECLINED reason=no EFI handoff file`).

### Workspace Checks & Host Resource Hygiene

- `sbm-core`: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace` (**22 / 22 passed**).
- `sbm-lab`: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace` (**52 / 52 passed**).
- `install`: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace` (**61 / 61 passed**).
- **Host resources (16 GiB Apple Silicon):** Every QEMU guest (builder VM, `aarch64` HVF, `x86_64` TCG, `ramfb`, negative tests) ran sequentially through `sbm-lab resources` within the 5120 MiB guest budget; host pressure remained `normal` throughout (`swap 651–699 / 2048 MiB`), and final `sbm-lab resources doctor` confirmed `running 0/2`, `reservations: none`, `qemu: none`.

---

## 4. Canonical Release Gate Status & Remaining Non-Core Blockers

Full gate-by-gate scorecard is recorded in `research/validation/evidence/2026-11-26-graphical-recovery-integration.md`. All **Core graphical recovery gates (Gates 4, 5, 6, 7, 8, and Core/Lab/Media reproducibility)** now **PASS** on both `aarch64` and `x86_64`. Three non-Core gaps remain open in `sbm/RELEASE_GATES.md` before declaring `0.0.1` fully qualified:

1. **Gate 0 (`install` `release-manifest.json`):** `sbm-media` currently emits `"artifacts": []` (populating `"media": [...]` only) in `release-manifest.json`; `RELEASE_GATES.md` §2 expects `artifacts` populated with both menus and both UKIs.
2. **Gate 1 (`sbm-uefi` startup latency):** `sbm-uefi` (`653012f`) reached `menu_ready` in `1,083 ms` on `aarch64` HVF (`16.0 s` on `x86_64` TCG) due to discovery-time UKI hashing, exceeding the `< 250 ms` HVF target in `RELEASE_GATES.md` Gate 1.
3. **Gate 3 (`sbm-uefi` Spec 03 volatile EFI vars):** `sbm-uefi` (`653012f`) still passes `sbm.handoff_sha256` and `sbm.esp_partuuid` via external load options rather than publishing all 5 Spec 03 volatile EFI variables with empty load options. (`sbm-core` already reads all 5 Spec 03 EFI variables first and falls back to cmdline tokens.)
4. **Observation on `sbm-toram.sh` at 4 GiB RAM:** With 4 GiB guest RAM (`mem_kib=3976632`, `need=2429743104`), `sbm-toram.sh` fell back safely to `backing-protected` (rather than copying into RAM) because `mount -t erofs -o ro,loop` or tmpfs copy inside the initrd did not complete; backing-disk protection worked cleanly (`SBM_PROTECT backing=/dev/sda esp=/dev/sda1 ro_partitions=4`), and investigating initrd `loop` module inclusion for `resident` mode is a clean follow-up.
5. **Physical hardware & native x86_64 KVM CI:** `x86_64` was verified under TCG emulation on Apple Silicon; native `x86_64` CI and physical-machine USB boot remain to be run.
