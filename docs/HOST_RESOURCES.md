# Host memory for the builder

Measured on the 16 GiB, 10-core Apple M5 development Mac on 2026-11-26.

## Before

One process, owned by this repo: `qemu-system-aarch64 -name sbm-builder -m 8192 -smp 4 -accel hvf`. Its working directory was `build/builder` and its disk was `build/builder/disk.qcow2`. No other QEMU system process was running. The guest was idle: 533 MiB used, 7.4 GiB page cache, no guest swap, and no `mkosi`, `cargo`, or `zstd` process. The image build had already finished.

| Host metric | Value |
| --- | --- |
| `hw.memsize` | 17179869184 (16 GiB) |
| QEMU `phys_footprint` | 15 GB (peak 15 GB) |
| QEMU RSS | about 0.9–1.0 GB |
| Pages free | 9290 (about 145 MiB at 16 KiB pages) |
| Compressor pages occupied | 475772 (about 7.3 GiB) |
| Swap used | 1983.75 MiB of 3072 MiB |
| `memory_pressure` free percentage | 39% |

The guest allocation, not the idle workload, was the cost. HVF charges the full `-m` size to the process footprint.

## After `shutdown -h now`

The QEMU process exited in a few seconds. The qcow2 disk, Cargo `target/`, and dnf cache were left in place.

| Host metric | Value |
| --- | --- |
| QEMU processes | none |
| Pages free | 458214 (about 7.0 GiB) |
| Compressor pages occupied | 69054 (about 1.1 GiB) |
| Swap used | 1190.38 MiB of 2048 MiB |
| `memory_pressure` free percentage | 77% |

## Defaults

`scripts/start-builder.sh` now chooses the guest size from the host. On this machine that is 4096 MiB and 2 vCPUs, HVF, and it refuses to start when another `qemu-system-*` is already running. `SBM_BUILDER_MEMORY_MIB`, `SBM_BUILDER_CPUS`, `SBM_ALLOW_CONCURRENT_QEMU=1`, and `SBM_ALLOW_OVERSUBSCRIBE=1` are the explicit overrides. A guest is refused when `2 * memory` would leave less than 6 GiB, which is what rejects 8192 MiB here.

`scripts/build-inside.sh` uses one Cargo job when `MemTotal` is under 5 GiB, and `zstd -19 -T1` for the image. `scripts/boot-smoke.sh` uses the same refusal rules and kills its QEMU child if the script exits early.

`sbm-lab` has its own clamps, `SBM_LAB_MAX_MEMORY_MIB` (default 8192) and `SBM_LAB_MAX_CONCURRENT` (default 2). Those are not changed here. Set them to 4096 and 1 on this Mac, and do not run a lab guest at the same time as the builder.
