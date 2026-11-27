#!/bin/bash
# Boot a built SBM Core disk image with Homebrew QEMU.
# aarch64 uses HVF. x86-64 uses TCG; say so when reporting that run.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
# shellcheck disable=SC1091
source "${root}/scripts/host-resources.sh"

usage() {
    echo "usage: boot-smoke.sh --arch arm64|x86-64 --image PATH --memory MIB [--scratch] [--timeout SEC] --evidence DIR" >&2
    exit 2
}

arch=""
image=""
memory=""
scratch=0
timeout=240
evidence=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --arch) arch=$2; shift 2 ;;
        --image) image=$2; shift 2 ;;
        --memory) memory=$2; shift 2 ;;
        --scratch) scratch=1; shift ;;
        --timeout) timeout=$2; shift 2 ;;
        --evidence) evidence=$2; shift 2 ;;
        *) usage ;;
    esac
done
[ -n "$arch" ] && [ -n "$image" ] && [ -n "$memory" ] && [ -n "$evidence" ] || usage
case "$image" in
    /dev/*) echo "refusing an image path under /dev" >&2; exit 2 ;;
esac
[ -f "$image" ] || { echo "image not found: $image" >&2; exit 2; }
hold=""
hold=$(sbm_reserve graphical "$memory" 2 "$root")
mkdir -p "$evidence"
image=$(cd "$(dirname "$image")" && pwd)/$(basename "$image")

qemu_aarch64=/opt/homebrew/bin/qemu-system-aarch64
qemu_x86=/opt/homebrew/bin/qemu-system-x86_64
fw=/opt/homebrew/share/qemu
serial="$evidence/serial.log"
qmp="$evidence/qmp.sock"
overlay="$evidence/disk-overlay.qcow2"
rm -f "$serial" "$qmp" "$overlay" "$evidence/screen.ppm" "$evidence/screen-smoke.ppm"
: >"$serial"

# Attach a disposable QCOW2 overlay so smoke runs never mutate the raw image or its digest.
/opt/homebrew/bin/qemu-img create -q -f qcow2 -F raw -b "$image" "$overlay"

scratch_arg=()
if [ "$scratch" -eq 1 ]; then
    scratch_disk="$evidence/scratch.qcow2"
    /opt/homebrew/bin/qemu-img create -q -f qcow2 "$scratch_disk" 1G
    scratch_arg=(
        -drive "if=none,id=scratch,file=${scratch_disk},format=qcow2"
        -device virtio-blk-pci,drive=scratch
    )
fi

common=(
    -smp 2
    -m "$memory"
    -device virtio-gpu-pci,edid=on,xres=1280,yres=800
    -device qemu-xhci -device usb-kbd -device usb-tablet
    -netdev user,id=net -device virtio-net-pci,netdev=net
    -serial "file:${serial}"
    -display none
    -qmp "unix:${qmp},server,nowait"
    -smbios type=1,manufacturer=SBM,product=SBM-Core-Lab,version=0.1.0
    -drive "if=none,id=hd,file=${overlay},format=qcow2"
    -device virtio-blk-pci,drive=hd
)

cleanup_qemu() {
    if [ -n "${pid:-}" ] && kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
        sleep 1
        kill -9 "$pid" 2>/dev/null || true
    fi
    rm -f "$overlay"
    if [ -n "${hold:-}" ]; then
        sbm_release "$hold" || true
        hold=""
    fi
}
trap cleanup_qemu EXIT
trap 'cleanup_qemu; exit 130' INT
trap 'cleanup_qemu; exit 143' TERM

if [ "$arch" = "arm64" ]; then
    "$qemu_aarch64" -name sbm-core-arm64 \
        -machine virt,accel=hvf,highmem=on,gic-version=3 -cpu host \
        "${common[@]}" "${scratch_arg[@]}" \
        -bios "$fw/edk2-aarch64-code.fd" &
    pid=$!
    accel=hvf
elif [ "$arch" = "x86-64" ]; then
    vars="$evidence/ovmf-vars.fd"
    cp "$fw/edk2-i386-vars.fd" "$vars"
    "$qemu_x86" -name sbm-core-x86-64 -accel tcg -machine q35 -cpu qemu64 \
        "${common[@]}" "${scratch_arg[@]}" \
        -drive "if=pflash,format=raw,unit=0,readonly=on,file=${fw}/edk2-x86_64-code.fd" \
        -drive "if=pflash,format=raw,unit=1,file=${vars}" &
    pid=$!
    accel=tcg
else
    echo "unknown arch $arch" >&2
    exit 2
fi
sbm_attach "$hold" "$pid"

python3 - "$qmp" "$evidence" "$serial" "$timeout" "$pid" <<'PY'
import json, os, socket, sys, time
qmp, evidence, serial, timeout, pid = sys.argv[1:]
timeout = int(timeout)
pid = int(pid)
started = time.time()
deadline = started + timeout
desktop_at = None
shots = 0

def alive(target):
    try:
        os.kill(target, 0)
    except OSError:
        return False
    return True

def read_serial():
    try:
        with open(serial, "r", errors="replace") as handle:
            return handle.read()
    except FileNotFoundError:
        return ""

def qmp_cmd(cmd):
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(8)
    sock.connect(qmp)
    sock.recv(65536)
    sock.send(b'{"execute":"qmp_capabilities"}\n')
    sock.recv(65536)
    sock.send(json.dumps(cmd).encode() + b"\n")
    data = b""
    while b"\n" not in data:
        chunk = sock.recv(65536)
        if not chunk:
            break
        data += chunk
    sock.close()
    return data.decode(errors="replace")

def shoot(name):
    path = os.path.join(evidence, name)
    qmp_cmd({"execute": "screendump", "arguments": {"filename": path}})
    print(f"screenshot {path}")

while time.time() < deadline and alive(pid):
    text = read_serial()
    desktop = "SBM_DESKTOP_READY" in text or "SBM_UI_READY" in text
    if desktop and desktop_at is None:
        desktop_at = time.time()
    if desktop and shots == 0 and time.time() - desktop_at > 3:
        try:
            shoot("screen.ppm")
            shots = 1
        except OSError as exc:
            print(f"screendump failed: {exc}")
    if "SBM_SMOKE_DONE" in text and shots < 2 and time.time() - (desktop_at or started) > 2:
        try:
            shoot("screen-smoke.ppm")
            shots = 2
        except OSError as exc:
            print(f"smoke screendump failed: {exc}")
        break
    if desktop and "SBM_SMOKE_BEGIN" not in text and time.time() - desktop_at > 25:
        break
    time.sleep(2)

names = (
    "SBM_DESKTOP_READY",
    "SBM_UI_READY",
    "SBM_TORAM_MODE",
    "SBM_BOOT_METRICS",
    "SBM_IDLE_MEM",
    "SBM_SMOKE_BEGIN",
    "SBM_SMOKE_DONE",
    "SBM_CPU_SAMPLE",
    "SBM_LABWC_EXIT",
)
text = read_serial()
found = [name for name in names if name in text]
with open(os.path.join(evidence, "markers.txt"), "w") as handle:
    handle.write("\n".join(found) + ("\n" if found else ""))
try:
    qmp_cmd({"execute": "system_powerdown"})
except OSError:
    pass
for _ in range(40):
    if not alive(pid):
        break
    time.sleep(1)
else:
    try:
        os.kill(pid, 15)
    except OSError:
        pass
    time.sleep(2)
    try:
        os.kill(pid, 9)
    except OSError:
        pass
print("markers:", ", ".join(found) or "none")
PY

wait "$pid" 2>/dev/null || true
for ppm in "$evidence"/screen.ppm "$evidence"/screen-smoke.ppm; do
    if [ -f "$ppm" ]; then
        sips -s format png "$ppm" --out "${ppm%.ppm}.png" >/dev/null
    fi
done
{
    echo "arch=$arch"
    echo "accel=$accel"
    echo "memory_mib=$memory"
    echo "image=$image"
    if [ "$arch" = "arm64" ]; then
        echo "qemu=$("$qemu_aarch64" --version | head -n 1)"
    else
        echo "qemu=$("$qemu_x86" --version | head -n 1)"
    fi
    echo "finished=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
} >"$evidence/meta.txt"
echo "evidence in $evidence"
