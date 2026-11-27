#!/bin/bash
# Start, stop, inspect, or trim the Fedora builder. The disk, package cache,
# and Cargo target stay in build/builder/disk.qcow2 across restarts.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
# shellcheck disable=SC1091
source "${root}/scripts/host-resources.sh"
dir="${root}/build/builder"
pidfile="${dir}/qemu.pid"
action=${1:-start}

qemu_pid() {
    if [ -f "$pidfile" ]; then
        local pid
        pid=$(cat "$pidfile")
        if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
            echo "$pid"
            return 0
        fi
    fi
    # Match the QEMU executable, not this awk line, which also contains the name.
    ps -axww -o pid=,comm=,args= | LC_ALL=C awk '$2 == "qemu-system-aarch64" && / -name sbm-builder/ { print $1; exit }'
}

release_reservation() {
    if [ -f "${dir}/reservation.id" ]; then
        sbm_release "$(cat "${dir}/reservation.id")" || true
        rm -f "${dir}/reservation.id"
    fi
}

stop_builder() {
    local pid
    pid=$(qemu_pid || true)
    if [ -z "$pid" ]; then
        rm -f "$pidfile"
        release_reservation
        echo "builder is not running"
        return 0
    fi
    if [ -f "${dir}/id_ed25519" ]; then
        ssh -i "${dir}/id_ed25519" -p 2222 \
            -o StrictHostKeyChecking=no \
            -o UserKnownHostsFile="${dir}/known_hosts" \
            -o ConnectTimeout=8 \
            builder@127.0.0.1 'sudo shutdown -h now' || true
    fi
    local i
    for i in $(seq 1 30); do
        if ! kill -0 "$pid" 2>/dev/null; then
            rm -f "$pidfile"
            release_reservation
            echo "builder stopped"
            return 0
        fi
        sleep 1
    done
    echo "shutdown did not exit; sending TERM to $pid" >&2
    kill "$pid" 2>/dev/null || true
    sleep 2
    if kill -0 "$pid" 2>/dev/null; then
        kill -9 "$pid" 2>/dev/null || true
    fi
    rm -f "$pidfile"
    release_reservation
    echo "builder stopped"
}

case "$action" in
    stop)
        stop_builder
        exit 0
        ;;
    status)
        pid=$(qemu_pid || true)
        if [ -n "$pid" ]; then
            echo "running pid=$pid"
            ps -o pid,rss,etime,command -p "$pid"
        else
            echo "stopped"
        fi
        exit 0
        ;;
    trim)
        pid=$(qemu_pid || true)
        if [ -z "$pid" ]; then
            echo "builder is not running; start it before trimming" >&2
            exit 1
        fi
        ssh -i "${dir}/id_ed25519" -p 2222 \
            -o StrictHostKeyChecking=no \
            -o UserKnownHostsFile="${dir}/known_hosts" \
            -o ConnectTimeout=8 \
            builder@127.0.0.1 'sudo fstrim -av'
        exit 0
        ;;
    start) ;;
    *)
        echo "usage: start-builder.sh [start|stop|status|trim]" >&2
        exit 2
        ;;
esac

pid=$(qemu_pid || true)
if [ -n "$pid" ]; then
    echo "builder already running pid=$pid"
    exit 0
fi

mem=${SBM_BUILDER_MEMORY_MIB:-$(default_guest_memory_mib)}
cpus=${SBM_BUILDER_CPUS:-$(default_guest_cpus)}

if [ ! -f "${dir}/disk.qcow2" ] || [ ! -f "${dir}/seed.iso" ]; then
    echo "builder disk is missing in ${dir}" >&2
    exit 1
fi

mkdir -p "$dir"
hold=$(SBM_DETACHED=1 sbm_reserve build "$mem" "$cpus" "$root")
printf '%s\n' "$hold" >"${dir}/reservation.id"
cd "$dir"
# A new session so the launching shell can exit without killing QEMU.
# discard=unmap,detect-zeroes=unmap allows guest fstrim to punch holes back
# into the host APFS QCOW2 file without offline qemu-img convert.
python3 -c 'import os, sys; os.setsid(); os.execv(sys.argv[1], sys.argv[1:])' \
    /opt/homebrew/bin/qemu-system-aarch64 \
    -name sbm-builder \
    -machine virt,accel=hvf,highmem=on,gic-version=3 \
    -cpu host \
    -smp "$cpus" \
    -m "$mem" \
    -bios /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
    -drive if=none,id=hd,file=disk.qcow2,format=qcow2,discard=unmap,detect-zeroes=unmap \
    -device virtio-blk-pci,drive=hd \
    -drive if=none,id=cidata,file=seed.iso,format=raw,media=cdrom,readonly=on \
    -device virtio-scsi-pci,id=scsi0 \
    -device scsi-cd,bus=scsi0.0,drive=cidata \
    -device virtio-balloon-pci,free-page-reporting=on,deflate-on-oom=on \
    -netdev user,id=net,hostfwd=tcp:127.0.0.1:2222-:22 \
    -device virtio-net-pci,netdev=net \
    -display none \
    -serial file:serial.log \
    -monitor none \
    </dev/null >>qemu.log 2>&1 &
echo $! >"$pidfile"
if ! sbm_attach "$hold" "$(cat "$pidfile")"; then
    kill "$(cat "$pidfile")" 2>/dev/null || true
    sbm_release "$hold" || true
    rm -f "${dir}/reservation.id" "$pidfile"
    exit 1
fi
echo "builder started pid=$(cat "$pidfile") memory_mib=${mem} cpus=${cpus} accel=hvf"
known="${dir}/known_hosts"
for _ in $(seq 1 45); do
    if ssh -i "${dir}/id_ed25519" -p 2222 \
        -o StrictHostKeyChecking=no \
        -o UserKnownHostsFile="$known" \
        -o ConnectTimeout=5 \
        builder@127.0.0.1 'true' 2>/dev/null; then
        echo "builder ssh is up"
        exit 0
    fi
    sleep 2
done
echo "builder process is up but ssh did not answer" >&2
exit 1
