#!/bin/bash
# Default guest size for this repo.
# On 2026-11-26 an 8192 MiB aarch64 guest was idle at 533 MiB inside the
# guest and still charged a 15 GB phys_footprint on a 16 GB Mac.
# Admission for every SBM repo is `sbm-lab resources`, not the checks below.
# Those checks are per shell and are not the cross-checkout lock.

_sbm_resources=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../sbm-lab/scripts" && pwd)/sbm-resources.sh
if [ -f "$_sbm_resources" ]; then
    # shellcheck disable=SC1091
    source "$_sbm_resources"
fi

host_memory_mib() {
    local bytes
    if [ "$(uname -s)" = "Darwin" ]; then
        bytes=$(sysctl -n hw.memsize)
    else
        bytes=$(awk '/^MemTotal:/ { print $2 * 1024 }' /proc/meminfo)
    fi
    echo $((bytes / 1024 / 1024))
}

host_cpu_count() {
    if [ "$(uname -s)" = "Darwin" ]; then
        sysctl -n hw.physicalcpu
    else
        nproc
    fi
}

# Leave 10 GiB for macOS on a 16 GiB machine. Never default above 4096.
default_guest_memory_mib() {
    local host reserve room
    host=$(host_memory_mib)
    if [ "$host" -ge 16384 ]; then
        reserve=10240
    else
        reserve=$((host / 2))
    fi
    room=$((host - reserve))
    if [ "$room" -gt 4096 ]; then
        room=4096
    fi
    if [ "$room" -lt 2048 ]; then
        room=2048
    fi
    echo "$room"
}

default_guest_cpus() {
    local n
    n=$(host_cpu_count)
    if [ "$n" -ge 4 ]; then
        echo 2
    else
        echo 1
    fi
}

qemu_system_lines() {
    ps -axo pid=,command= 2>/dev/null | LC_ALL=C /usr/bin/grep -E 'qemu-system-' | LC_ALL=C /usr/bin/grep -v grep || true
}

assert_no_other_qemu() {
    local lines
    lines=$(qemu_system_lines)
    if [ -n "$lines" ] && [ "${SBM_ALLOW_CONCURRENT_QEMU:-0}" != "1" ]; then
        echo "another QEMU system process is already running:" >&2
        echo "$lines" >&2
        echo "stop it, or set SBM_ALLOW_CONCURRENT_QEMU=1" >&2
        return 1
    fi
}

# phys_footprint tracked roughly twice -m. Keep at least 6 GiB after that estimate.
assert_guest_fits() {
    local mem=$1
    local host estimate room
    host=$(host_memory_mib)
    estimate=$((mem * 2))
    room=$((host - estimate))
    if [ "$room" -lt 6144 ] && [ "${SBM_ALLOW_OVERSUBSCRIBE:-0}" != "1" ]; then
        echo "refusing a ${mem} MiB guest on a ${host} MiB host (estimated footprint ${estimate} MiB)" >&2
        echo "set SBM_ALLOW_OVERSUBSCRIBE=1 to override" >&2
        return 1
    fi
}
