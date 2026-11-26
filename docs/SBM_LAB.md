# Boot contract for sbm-lab

`sbm-lab` does not yet ship a runner. Until it does, Agent 03 can call:

```bash
scripts/boot-smoke.sh \
  --arch arm64 \
  --image build/images/sbm-core-arm64.raw \
  --memory 4096 \
  --scratch \
  --evidence build/evidence/arm64-4g
```

`--arch x86-64` uses TCG. `--arch arm64` uses HVF. The script refuses an image path under `/dev` and refuses to start while another `qemu-system-*` process is running. On a 16 GiB host it also refuses a guest whose estimated footprint would leave less than 6 GiB; `SBM_ALLOW_OVERSUBSCRIBE=1` overrides that. 4096 MiB is enough for the resident RAM mode. Firmware comes from Homebrew QEMU (`edk2-aarch64-code.fd`, `edk2-x86_64-code.fd`, `edk2-i386-vars.fd`). It does not build EDK2.

`sbm-lab` clamps its own guests with `SBM_LAB_MAX_MEMORY_MIB` (default 8192) and `SBM_LAB_MAX_CONCURRENT` (default 2). On this 16 GiB Mac set `SBM_LAB_MAX_MEMORY_MIB=4096` and `SBM_LAB_MAX_CONCURRENT=1` before a lab run, and do not start a lab guest while the builder is up.

A smoke run expects the image ESP to contain `\EFI\sbm\state\smoke`. `scripts/inject-demo.sh` adds that marker and the golden handoff file to a copy of the image. The script then waits for `SBM_DESKTOP_READY` / `SBM_UI_READY` and, when present, `SBM_SMOKE_DONE` on the serial log.
