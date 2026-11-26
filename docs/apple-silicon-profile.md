# Apple Silicon image profile

This is the difference list for the shared mkosi pipeline. It is not a second distribution and it is not a package lock. Confirm every package NEVRA, including the historical name `kernel-16k`, when the image is actually built. Do not build that image from this note, and do not put Apple firmware blobs in git.

The normative boot rules are in `docs/hardware/apple-silicon-boot-architecture.md`. Hardware limits are in `research/hardware/apple-silicon-m1-to-m5.md`.

| Topic | Apple profile | Generic ARM64 PC profile |
|---|---|---|
| Kernel | Asahi kernel, 16 KiB pages | Distro `aarch64` kernel, usually 4 KiB pages |
| Firmware | `asahi-fwextract` on the target Mac, into the System ESP `vendorfw` archive | `linux-firmware` packages |
| Graphics | Mesa Asahi only where the GPU cell is `linux-asahi` (M1 and M2). Otherwise the boot framebuffer or a software renderer | The machine's UEFI GOP plus the distro GPU driver |
| Storage | Apple NVMe. Do not write the ISC container, Recovery, or macOS APFS | Standard NVMe or SATA |
| Networking | No UEFI SNP (`CONFIG_NO_NET` on Asahi U-Boot). Wi-Fi needs the extracted firmware and a published driver | UEFI SNP may exist |
| Input | U-Boot keyboard. The pinned Asahi defconfig has no USB mouse or trackpad | GOP plus the firmware's pointer |
| Boot parameters | Flattened device tree from m1n1 and U-Boot | ACPI or the board device tree |
| Device tree | One OS owns `/m1n1/boot.bin` on the internal System ESP | The firmware's own tables |
| Packaging | The same mkosi pipeline, with this delta. Image size targets in ADR-003 are targets, not measurements | The PC mkosi profile |

Machines whose Asahi installer cell is `no` (M3 Ultra, all M4, all M5 including `T8142`) are not image targets until those tables change.
