# SBM Core 0.1 implementation plan

This is the plan that the image build follows. It does not change the architecture in `sbm/TECHNOLOGY_STACK.md`.

## Image

One mkosi 27.1 configuration, Fedora 45, `Format=disk`, `Bootloader=uki`. Platform packages are selected with `[Match] Architecture=`. The Apple profile is present and is not passed to the build.

The root partition is EROFS, zstd, `Verity=data`. A sibling `Verity=hash` partition supplies the root hash that mkosi puts on the UKI command line. There is no persistent `/var` partition. `systemd.volatile=overlay` keeps writable state in RAM. SELinux relabeling is off and the command line sets `selinux=0`.

The UKI is the removable-media boot file (`EFI/BOOT/BOOT*.EFI`). A post-output step also copies it to `\EFI\sbm\recovery\slot-a\sbm-core-<arch>.efi` and writes `\EFI\sbm\recovery\manifest.json` schema 1.0.

Development Secure Boot keys are generated in the builder and are gitignored. `SignExpectedPcr=no`.

## RAM mode

`sbm.toram=auto` copies the root into RAM when `MemTotal` is at least 3584 MiB and the image plus 1536 MiB still fits. Otherwise the backing disk stays the root and non-ESP partitions on that disk are set read-only. The daemon rejects a mutating plan aimed at that disk. No mutating plan is executed in 0.1.

## Session

`sbm-cored` listens on `/run/sbm/cored.sock` for a fixed request enum. `sbm-ui` is an Iced 0.14 shell. labwc starts it. GParted is `/usr/libexec/gpartedbin` under XWayland, not a shell wrapper. wayvnc is disabled until an operator enables it, then it binds to 127.0.0.1 with a generated password.

## Build and boot

Rust release binaries are built in the Fedora VM and copied in with `--extra-tree`. `scripts/boot-smoke.sh` boots the raw image on the Mac: HVF for arm64, TCG for x86-64. Partition experiments use a second disposable qcow2.
