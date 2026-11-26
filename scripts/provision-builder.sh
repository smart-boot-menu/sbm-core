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
    sysroot-x86_64-fc45-glibc \
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

# The cross gcc looks in /usr/x86_64-linux-gnu/sys-root, but Fedora ships the
# glibc sysroot under /usr/x86_64-redhat-linux/sys-root/fc45 and its libc.so
# uses absolute paths. A wrapper passes --sysroot. Fedora publishes no
# libatomic or libgcc sysroot, so those two x86_64 libraries are unpacked
# from the matching rpms.
sysroot=/usr/x86_64-redhat-linux/sys-root/fc45
if [ ! -e "${sysroot}/usr/lib64/libatomic.so" ]; then
    work=$(mktemp -d)
    sudo dnf -y download --forcearch=x86_64 --destdir "$work" libatomic libgcc
    mkdir -p "${work}/tree"
    for rpm in "$work"/*.rpm; do
        rpm2cpio "$rpm" | (cd "${work}/tree" && cpio -idmu)
    done
    sudo mkdir -p "${sysroot}/usr/lib64" "${sysroot}/lib64"
    sudo cp -a "${work}/tree/usr/lib64/libatomic.so."* "${sysroot}/usr/lib64/"
    sudo cp -a "${work}/tree/lib64/libgcc_s"* "${sysroot}/lib64/"
    sudo cp -a "${work}/tree/lib64/libgcc_s"* "${sysroot}/usr/lib64/"
    sudo ln -sfn libatomic.so.1 "${sysroot}/usr/lib64/libatomic.so"
    sudo ln -sfn libgcc_s.so.1 "${sysroot}/usr/lib64/libgcc_s.so"
    sudo ln -sfn libgcc_s.so.1 "${sysroot}/lib64/libgcc_s.so"
    rm -rf "$work"
fi
gccdir=$(echo /usr/lib/gcc/x86_64-linux-gnu/*)
if [ ! -f "${gccdir}/libatomic_asneeded.so" ]; then
    sudo tee "${gccdir}/libatomic_asneeded.so" >/dev/null <<'EOF'
/* GNU ld script */
OUTPUT_FORMAT(elf64-x86-64)
INPUT ( AS_NEEDED ( -latomic ) )
EOF
fi
sudo tee /usr/local/bin/x86_64-linux-gnu-gcc >/dev/null <<'EOF'
#!/bin/bash
exec /usr/bin/x86_64-linux-gnu-gcc --sysroot=/usr/x86_64-redhat-linux/sys-root/fc45 "$@"
EOF
sudo chmod 755 /usr/local/bin/x86_64-linux-gnu-gcc

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
