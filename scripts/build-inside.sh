#!/bin/bash
# Run inside the Fedora builder, from a checkout of this repo.
set -euo pipefail
export HOME="${HOME:-/home/builder}"
export PATH="${HOME}/.cargo/bin:/usr/local/bin:${PATH}"
cd "$(dirname "$0")/.."
root=$(pwd)
# shellcheck disable=SC1091
source "${HOME}/.cargo/env"
mem_kib=$(awk '/^MemTotal:/ { print $2 }' /proc/meminfo)
if [ "${mem_kib:-0}" -lt 5242880 ]; then
    default_jobs=1
else
    default_jobs=2
fi
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$default_jobs}"

rustup target add aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu
cargo build --release --locked --target aarch64-unknown-linux-gnu
cargo build --release --locked --target x86_64-unknown-linux-gnu

stage() {
    local triple=$1
    local dest="build/staging/${triple}"
    mkdir -p "${dest}/usr/bin"
    cp "target/${triple}/release/sbm-cored" "${dest}/usr/bin/sbm-cored"
    cp "target/${triple}/release/sbm-ctl" "${dest}/usr/bin/sbm-ctl"
    cp "target/${triple}/release/sbm-ui" "${dest}/usr/bin/sbm-ui"
    cp "target/${triple}/release/sbm-toram" "${dest}/usr/bin/sbm-toram"
    chmod 755 "${dest}/usr/bin/"*
}

stage aarch64-unknown-linux-gnu
stage x86_64-unknown-linux-gnu

if [ ! -f mkosi.key ] || [ ! -f mkosi.crt ]; then
    /usr/local/bin/mkosi genkey
fi
chmod 600 mkosi.key
chmod 755 mkosi.postinst.chroot mkosi.postoutput \
    mkosi.extra/usr/lib/sbm/start-session \
    mkosi.extra/usr/lib/sbm/sbm-smoke.sh \
    mkosi.extra/var/lib/sbm/.config/labwc/autostart \
    image/initrd-extra/usr/lib/sbm/sbm-toram.sh

mkdir -p build/images build/mkosi-cache build/mkosi-pkgcache build/mkosi-build
version=$(tr -d '[:space:]' < mkosi.version)

finalize() {
    sudo env \
        ARCHITECTURE="$1" \
        OUTPUTDIR="${root}/build/images" \
        IMAGE_VERSION="${version}" \
        python3 "${root}/scripts/finalize-image.py"
}

# SBM_ARCHES="arm64" or "x86-64" builds one image. The default builds both.
arches=${SBM_ARCHES:-"arm64 x86-64"}
for arch in $arches; do
    case "$arch" in
        arm64) triple=aarch64-unknown-linux-gnu ;;
        x86-64) triple=x86_64-unknown-linux-gnu ;;
        *) echo "unknown architecture: $arch" >&2; exit 2 ;;
    esac
    sudo /usr/local/bin/mkosi --force build --architecture="$arch" \
        --extra-tree="${root}/build/staging/${triple}"
    finalize "$arch"
done
sudo chmod -R a+rX build/images
echo "images:"
ls -lh build/images
