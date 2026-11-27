#!/bin/sh
# Development-only smoke helper. Excluded from release images and injected only
# when SBM_ENABLE_SMOKE=1 with a non-release SBM_BUILD_PROFILE.
# Runs only when /efi/EFI/sbm/state/smoke exists. Snapshot first, then GParted.
set -u
mkdir -p /efi/EFI/sbm/state /run/sbm
echo "<6>SBM_SMOKE_BEGIN" >/dev/kmsg || true
/usr/bin/sbm-ctl snapshot >/efi/EFI/sbm/state/snapshot.json 2>/run/sbm/smoke-snapshot.err || true
/usr/bin/sbm-ctl tools >/efi/EFI/sbm/state/tool-probe.json 2>/run/sbm/smoke-tools.err || true
read -r _cpu a b c idle _rest </proc/stat
sleep 5
read -r _cpu a2 b2 c2 idle2 _rest </proc/stat
echo "<6>SBM_CPU_SAMPLE ${a} ${b} ${c} ${idle} ${a2} ${b2} ${c2} ${idle2}" >/dev/kmsg || true
/usr/bin/sbm-ctl gparted >/run/sbm/smoke-gparted.out 2>/run/sbm/smoke-gparted.err || true
sleep 12
killall gpartedbin 2>/dev/null || true
sync
echo "<6>SBM_SMOKE_DONE" >/dev/kmsg || true
