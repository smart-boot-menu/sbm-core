#!/bin/sh
# Copy the verified root into RAM when memory allows.
# Constants must match sbm-runtime: AUTO_MIB=3584 and HEADROOM_MIB=1536.
# Unknown sbm.toram values fail safe and leave the backing device in place.

set -u

MARKER=/run/sbm/mode
if [ -f "$MARKER" ]; then
    exit 0
fi
mkdir -p /run/sbm

MODE=auto
for arg in $(cat /proc/cmdline 2>/dev/null); do
    case "$arg" in
        sbm.toram=*) MODE=${arg#sbm.toram=} ;;
    esac
done

MEM_KIB=0
while read -r key val _rest; do
    if [ "$key" = "MemTotal:" ]; then
        MEM_KIB=$val
        break
    fi
done < /proc/meminfo 2>/dev/null || true

SRC=$(findmnt -n -o SOURCE /sysroot 2>/dev/null || true)
SRC=$(printf '%s' "$SRC" | tr -d '[:space:]')
if [ -z "$SRC" ]; then
    printf '%s\n' backing-protected > "$MARKER"
    printf '%s\n' missing > /run/sbm/root-source
    exit 0
fi
SIZE=$(blockdev --getsize64 "$SRC" 2>/dev/null || echo 0)

# sbm-runtime TORAM_AUTO_MEMTOTAL_KIB = 3584 MiB
# sbm-runtime TORAM_HEADROOM_BYTES = 1536 MiB
AUTO_MIB=3584
HEADROOM_MIB=1536
AUTO_KIB=$((AUTO_MIB * 1024))
HEADROOM=$((HEADROOM_MIB * 1024 * 1024))
MEM_BYTES=$((MEM_KIB * 1024))
NEED=$((SIZE + HEADROOM))

copy_ok=0
if [ "$MODE" != "never" ] && [ "$SIZE" -gt 0 ] && [ "$MEM_BYTES" -gt "$NEED" ]; then
    if [ "$MODE" = "always" ] || [ "$MODE" = "auto" ] || [ -z "$MODE" ]; then
        if [ "$MODE" = "always" ] || [ "$MEM_KIB" -ge "$AUTO_KIB" ]; then
            copy_ok=1
        fi
    fi
fi

if [ "$copy_ok" -eq 1 ]; then
    modprobe loop 2>/dev/null || true
    modprobe erofs 2>/dev/null || true
    mkdir -p /run/sbm/ramroot
    TMPFS_SIZE=$((SIZE + 16777216))
    if mount -t tmpfs -o "size=${TMPFS_SIZE},mode=0755" tmpfs /run/sbm/ramroot 2>/dev/null; then
        IMG=/run/sbm/ramroot/root.erofs
    else
        IMG=/run/sbm/root.erofs
    fi
    if dd if="$SRC" of="$IMG" bs=4194304 status=none; then
        mkdir -p /run/old-sysroot
        if mount --move /sysroot /run/old-sysroot && mount -t erofs -o ro,loop "$IMG" /sysroot; then
            umount /run/old-sysroot 2>/dev/null || true
            printf '%s\n' resident > "$MARKER"
            printf '%s\n' "$SRC" > /run/sbm/root-source
            printf '%s\n' "$SIZE" > /run/sbm/root-bytes
            echo "<6>SBM_TORAM_MODE=resident src=${SRC} bytes=${SIZE} mem_kib=${MEM_KIB}" > /dev/kmsg 2>/dev/null || true
            exit 0
        fi
        if ! findmnt /sysroot >/dev/null 2>&1; then
            mount --move /run/old-sysroot /sysroot 2>/dev/null || mount -t erofs -o ro "$SRC" /sysroot 2>/dev/null || true
        fi
    fi
    rm -f "$IMG" 2>/dev/null || true
    umount /run/sbm/ramroot 2>/dev/null || true
fi

printf '%s\n' backing-protected > "$MARKER"
printf '%s\n' "$SRC" > /run/sbm/root-source
printf '%s\n' "$SIZE" > /run/sbm/root-bytes
echo "<6>SBM_TORAM_MODE=backing-protected src=${SRC:-missing} bytes=${SIZE} mem_kib=${MEM_KIB} need=${NEED}" > /dev/kmsg 2>/dev/null || true
exit 0
