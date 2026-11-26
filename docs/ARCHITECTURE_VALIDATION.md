# Architecture validation

The stack is unchanged: mkosi 27.1, Fedora 45, Fedora Asahi Remix only as an unbuilt profile, signed UKIs, EROFS with zstd, a dm-verity hash partition, an immutable root, a RAM overlay, labwc, Wayland, XWayland, Iced 0.14, wgpu with a tiny-skia fallback, GParted, and wayvnc.

Primary-source notes and the two deliberate refinements (the 3584 MiB `MemTotal` threshold, and not building `kernel-16k` because that package is not in the enabled Fedora 45 repositories) are in `research/architecture/sbm-core-baseline-validation.md`.

NVIDIA modesetting is the in-kernel `nouveau` driver. `nvidia-gpu-firmware` is not in the base package list; virtual GPUs and the Intel/AMD firmware packages are. A missing proprietary NVIDIA driver is not a second graphics stack.

The local Secure Boot key is not enrolled in the QEMU firmware used by `scripts/boot-smoke.sh`. Those runs boot with Secure Boot disabled and still load the signed UKI.
