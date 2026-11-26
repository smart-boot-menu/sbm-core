#!/bin/bash
# Provision the Fedora 45 aarch64 builder. Runs inside that VM, never on the Mac.
set -euxo pipefail
export HOME=/home/builder
exec > /home/builder/provision.log 2>&1

echo "provision start $(date -u +%Y-%m-%dT%H:%M:%SZ)"
sudo dnf -y install --setopt=install_weak_deps=False \
    python3 python3-pip python3-devel \
    git curl ca-certificates \
    systemd-ukify systemd-boot-unsigned \
    erofs-utils sbsigntools openssl mtools \
    gcc make gcc-c++ \
    gcc-x86_64-linux-gnu binutils-x86_64-linux-gnu \
    distribution-gpg-keys bubblewrap \
    zstd e2fsprogs dosfstools parted util-linux rsync \
    qemu-user-static-x86 \
    cpio tar xz bzip2 \
    python3-cryptography \
    clang pkgconf-pkg-config \
    jq kmod policycoreutils socat

sudo setenforce 0 || true
if [ -f /etc/selinux/config ]; then
    sudo sed -i 's/^SELINUX=.*/SELINUX=permissive/' /etc/selinux/config
fi

if [ ! -x /opt/mkosi/bin/mkosi ]; then
    sudo python3 -m venv /opt/mkosi
    sudo /opt/mkosi/bin/pip install --upgrade pip
    sudo /opt/mkosi/bin/pip install 'https://github.com/systemd/mkosi/archive/refs/tags/v27.1.tar.gz'
fi
sudo ln -sfn /opt/mkosi/bin/mkosi /usr/local/bin/mkosi
/usr/local/bin/mkosi --version

if [ ! -x "$HOME/.cargo/bin/rustup" ]; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
        sh -s -- -y --default-toolchain 1.100.0 --profile minimal --component rustfmt,clippy
fi
# shellcheck disable=SC1091
source "$HOME/.cargo/env"
rustup target add x86_64-unknown-linux-gnu
rustc --version
cargo --version

echo "package versions:"
sudo dnf -q repoquery --qf '%{name} %{epoch}:%{version}-%{release} %{arch}\n' \
    labwc gparted wayvnc kernel-core systemd erofs-utils \
    || true
echo "provision done $(date -u +%Y-%m-%dT%H:%M:%SZ)"
