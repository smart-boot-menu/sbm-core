#!/bin/bash
# Run inside the Fedora builder, from a checkout of this repo.
#
# Environment controls:
#   SBM_BUILD_PROFILE  dev (default), integration, or release
#   SBM_ARCHES         Space-separated subset of "arm64" and "x86-64".
#                      Defaults to native builder arch for dev/integration,
#                      and "arm64 x86-64" for release.
#   SBM_INCREMENTAL    1 to enable mkosi Incremental=yes (default 1 for dev/integration,
#                      0 for release). Production mkosi.conf remains Incremental=no.
#   SBM_COMPRESS       1 to produce .raw.zst via zstd -19 (default 0 for dev/integration,
#                      1 for release).
#   CARGO_BUILD_JOBS   Defaults to 1 (<5 GiB RAM) or 2 (>=5 GiB RAM).
set -euo pipefail
export HOME="${HOME:-/home/builder}"
export PATH="${HOME}/.cargo/bin:/usr/local/bin:${PATH}"
cd "$(dirname "$0")/.."
root=$(pwd)
# shellcheck disable=SC1091
source "${HOME}/.cargo/env"

build_t0=$(date +%s)
profile=${SBM_BUILD_PROFILE:-dev}
case "$profile" in
    dev|integration)
        default_incremental=1
        default_compress=0
        case "$(uname -m)" in
            aarch64|arm64) default_arches="arm64" ;;
            x86_64|amd64) default_arches="x86-64" ;;
            *) default_arches="arm64" ;;
        esac
        ;;
    release)
        default_incremental=0
        default_compress=1
        default_arches="arm64 x86-64"
        ;;
    *)
        echo "unknown SBM_BUILD_PROFILE: $profile (expected dev, integration, or release)" >&2
        exit 2
        ;;
esac

arches=${SBM_ARCHES:-$default_arches}
incremental=${SBM_INCREMENTAL:-$default_incremental}
compress=${SBM_COMPRESS:-$default_compress}

mem_kib=$(awk '/^MemTotal:/ { print $2 }' /proc/meminfo)
if [ "${mem_kib:-0}" -lt 5242880 ]; then
    default_jobs=1
else
    default_jobs=2
fi
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$default_jobs}"

echo "sbm-core build: profile=${profile} arches=[${arches}] incremental=${incremental} compress=${compress} jobs=${CARGO_BUILD_JOBS}"

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

# Compile and stage only the Rust target(s) required by SBM_ARCHES.
cargo_t0=$(date +%s)
for arch in $arches; do
    case "$arch" in
        arm64) triple=aarch64-unknown-linux-gnu ;;
        x86-64) triple=x86_64-unknown-linux-gnu ;;
        *) echo "unknown architecture: $arch" >&2; exit 2 ;;
    esac
    rustup target add "$triple"
    cargo build --release --locked --target "$triple"
    stage "$triple"
done
cargo_secs=$(( $(date +%s) - cargo_t0 ))

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

mkosi_inc_flag="--incremental=no"
if [ "$incremental" = "1" ]; then
    mkosi_inc_flag="--incremental=yes"
fi

finalize() {
    sudo env \
        ARCHITECTURE="$1" \
        OUTPUTDIR="${root}/build/images" \
        IMAGE_VERSION="${version}" \
        SBM_COMPRESS="${compress}" \
        SBM_FORCE_COMPRESS="${SBM_FORCE_COMPRESS:-0}" \
        python3 "${root}/scripts/finalize-image.py"
}

mkosi_secs=0
finalize_secs=0
for arch in $arches; do
    case "$arch" in
        arm64) triple=aarch64-unknown-linux-gnu ;;
        x86-64) triple=x86_64-unknown-linux-gnu ;;
        *) echo "unknown architecture: $arch" >&2; exit 2 ;;
    esac
    t0=$(date +%s)
    sudo /usr/local/bin/mkosi --force build \
        --architecture="$arch" \
        "$mkosi_inc_flag" \
        --extra-tree="${root}/build/staging/${triple}"
    mkosi_secs=$(( mkosi_secs + $(date +%s) - t0 ))

    t0=$(date +%s)
    finalize "$arch"
    finalize_secs=$(( finalize_secs + $(date +%s) - t0 ))
done

sudo chmod -R a+rX build/images
total_secs=$(( $(date +%s) - build_t0 ))
echo "timings: cargo=${cargo_secs}s mkosi=${mkosi_secs}s finalize=${finalize_secs}s total=${total_secs}s"
echo "images:"
ls -lh build/images
