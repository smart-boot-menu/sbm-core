#!/bin/bash
# Add the golden EFI handoff and the smoke marker to a copy of a built image.
# The pristine image is left unchanged. Run this on Linux, where the ESP can be mounted.
set -euo pipefail
if [ "$#" -ne 2 ]; then
    echo "usage: inject-demo.sh <pristine.raw> <demo.raw>" >&2
    exit 2
fi
src=$1
dest=$2
case "$src$dest" in
    /dev/*|*/dev/*) echo "refusing a path under /dev" >&2; exit 2 ;;
esac
cp --sparse=always "$src" "$dest"
handoff=$(cd "$(dirname "$0")/../.." && pwd)/sbm-common/test-vectors/efi-handoff.v1.json
mnt=$(mktemp -d)
loop=$(sudo losetup --find --show --partscan "$dest")
cleanup() {
    sudo umount "$mnt" 2>/dev/null || true
    sudo losetup --detach "$loop" 2>/dev/null || true
    rmdir "$mnt" 2>/dev/null || true
}
trap cleanup EXIT
sudo mount "${loop}p1" "$mnt"
sudo mkdir -p "$mnt/EFI/sbm/handoff" "$mnt/EFI/sbm/state"
sudo cp "$handoff" "$mnt/EFI/sbm/handoff/current.json"
echo smoke | sudo tee "$mnt/EFI/sbm/state/smoke" >/dev/null
sync
echo "demo image $dest"
