# Boot contract for sbm-lab

`sbm-lab` does not yet ship a runner. Until it does, Agent 03 can call:

```bash
scripts/boot-smoke.sh \
  --arch arm64 \
  --image build/images/sbm-core-arm64_0.1.0.raw \
  --memory 8192 \
  --scratch \
  --evidence build/evidence/arm64-8g
```

`--arch x86-64` uses TCG. `--arch arm64` uses HVF. The script refuses an image path under `/dev`. Firmware comes from Homebrew QEMU (`edk2-aarch64-code.fd`, `edk2-x86_64-code.fd`, `edk2-i386-vars.fd`). It does not build EDK2.

A smoke run expects the image ESP to contain `\EFI\sbm\state\smoke`. `scripts/inject-demo.sh` adds that marker and the golden handoff file to a copy of the image. The script then waits for `SBM_DESKTOP_READY` / `SBM_UI_READY` and, when present, `SBM_SMOKE_DONE` on the serial log.
