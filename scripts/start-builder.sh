#!/bin/bash
# Start or stop the Fedora builder. The disk, package cache, and Cargo
# target stay in build/builder/disk.qcow2 across restarts.
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
    ps -axo pid=,command= | LC_ALL=C awk '/qemu-system-aarch64/ && /sbm-builder/ { print $1; exit }'
}

stop_builder() {
    local pid
    pid=$(qemu_pid || true)
    if [ -z "$pid" ]; then
        rm -f "$pidfile"
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
    start) ;;
    *)
        echo "usage: start-builder.sh [start|stop|status]" >&2
        exit 2
        ;;
esac

pid=$(qemu_pid || true)
if [ -n "$pid" ]; then
    echo "builder already running pid=$pid"
    exit 0
fi

assert_no_other_qemu
mem=${SBM_BUILDER_MEMORY_MIB:-$(default_guest_memory_mib)}
cpus=${SBM_BUILDER_CPUS:-$(default_guest_cpus)}
assert_guest_fits "$mem"

if [ ! -f "${dir}/disk.qcow2" ] || [ ! -f "${dir}/seed.iso" ]; then
    echo "builder disk is missing in ${dir}" >&2
    exit 1
fi

mkdir -p "$dir"
cd "$dir"
/opt/homebrew/bin/qemu-system-aarch64 \
    -name sbm-builder \
    -machine virt,accel=hvf,highmem=on,gic-version=3 \
    -cpu host \
    -smp "$cpus" \
    -m "$mem" \
    -bios /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
    -drive if=none,id=hd,file=disk.qcow2,format=qcow2 \
    -device virtio-blk-pci,drive=hd \
    -drive if=none,id=cidata,file=seed.iso,format=raw,media=cdrom,readonly=on \
    -device virtio-scsi-pci,id=scsi0 \
    -device scsi-cd,bus=scsi0.0,drive=cidata \
    -netdev user,id=net,hostfwd=tcp:127.0.0.1:2222-:22 \
    -device virtio-net-pci,netdev=net \
    -display none \
    -serial file:serial.log \
    -monitor none &
echo $! >"$pidfile"
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
