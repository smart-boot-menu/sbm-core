#!/usr/bin/env python3
"""Copy the UKI onto the recovery path and record image digests.

Run this as root on the builder after mkosi has written the raw image.
mkosi's own post-output script cannot mount the image.
"""

import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

ARCH_NAME = {"x86-64": "x86_64", "arm64": "aarch64"}
BOOT_EFI = {"x86-64": "BOOTX64.EFI", "arm64": "BOOTAA64.EFI"}
DISSECT = "/usr/lib/systemd/systemd-dissect"


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def find_esp(root: pathlib.Path) -> pathlib.Path | None:
    candidates = [root, root / "efi", root / "boot", root / "boot/efi"]
    for candidate in candidates:
        if (candidate / "EFI" / "BOOT").is_dir() or (candidate / "efi" / "boot").is_dir():
            return candidate
    return None


def vfat_partition(loop: str) -> str:
    listed = subprocess.run(
        ["lsblk", "-ln", "-o", "PATH,FSTYPE", loop],
        check=True,
        text=True,
        capture_output=True,
    ).stdout
    found = []
    for line in listed.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[1] == "vfat":
            found.append(parts[0])
    if len(found) != 1:
        raise RuntimeError(f"expected one vfat partition on {loop}, found {found or listed!r}")
    return found[0]


def mount_loop(image: pathlib.Path, mountpoint: pathlib.Path) -> str:
    loop = subprocess.run(
        ["losetup", "--find", "--show", "--partscan", str(image)],
        check=True,
        text=True,
        capture_output=True,
    ).stdout.strip()
    subprocess.run(["udevadm", "settle"], check=False)
    part = vfat_partition(loop)
    subprocess.run(["mount", part, str(mountpoint)], check=True)
    return loop


def mount(image: pathlib.Path, mountpoint: pathlib.Path) -> str:
    if os.path.exists(DISSECT):
        dissected = subprocess.run(
            [DISSECT, "--mount", str(image), str(mountpoint)],
            check=False,
        )
        if dissected.returncode == 0:
            return "dissect"
        print("systemd-dissect --mount failed, using losetup", file=sys.stderr)
    return mount_loop(image, mountpoint)


def unmount(mountpoint: pathlib.Path, how: str) -> None:
    if how == "dissect":
        subprocess.run([DISSECT, "--umount", str(mountpoint)], check=False)
        return
    subprocess.run(["umount", str(mountpoint)], check=False)
    if how.startswith("/dev/loop"):
        subprocess.run(["losetup", "--detach", how], check=False)


def main() -> int:
    arch = os.environ["ARCHITECTURE"]
    pretty = ARCH_NAME[arch]
    output_dir = pathlib.Path(os.environ["OUTPUTDIR"])
    compress = os.environ.get("SBM_COMPRESS", "1") != "0" and "--no-compress" not in sys.argv[1:]
    force_compress = os.environ.get("SBM_FORCE_COMPRESS", "0") == "1"

    raws = sorted(
        path
        for path in output_dir.glob("*.raw")
        if path.is_file()
        and not path.is_symlink()
        and (f"-{arch}_" in path.name or path.name.endswith(f"-{arch}.raw"))
    )
    if len(raws) != 1:
        print(f"expected one {arch} raw image in {output_dir}, found {raws}", file=sys.stderr)
        return 1
    image = raws[0]
    raw_sha_path = output_dir / f"{image.name}.sha256"
    prev_raw_digest = None
    if raw_sha_path.is_file():
        parts = raw_sha_path.read_text().split()
        if parts:
            prev_raw_digest = parts[0]

    mountpoint = pathlib.Path(tempfile.mkdtemp(prefix="sbm-esp-"))
    how = mount(image, mountpoint)
    try:
        esp = find_esp(mountpoint)
        if esp is None:
            print(f"ESP not found under {mountpoint}", file=sys.stderr)
            for path in mountpoint.rglob("*"):
                if path.suffix.lower() == ".efi":
                    print(path, file=sys.stderr)
            return 1
        boot = esp / "EFI" / "BOOT" / BOOT_EFI[arch]
        if not boot.is_file():
            alt = esp / "efi" / "boot" / BOOT_EFI[arch].lower()
            boot = alt if alt.is_file() else boot
        if not boot.is_file():
            found = list(esp.rglob("*.efi")) + list(esp.rglob("*.EFI"))
            print(f"missing {boot}; efi files: {found}", file=sys.stderr)
            return 1
        if not os.access(boot.parent, os.W_OK):
            print("ESP mount is read-only, remounting with losetup", file=sys.stderr)
            unmount(mountpoint, how)
            how = mount_loop(image, mountpoint)
            esp = find_esp(mountpoint)
            if esp is None:
                print("ESP not found after losetup", file=sys.stderr)
                return 1
            boot = esp / "EFI" / "BOOT" / BOOT_EFI[arch]
        slot = esp / "EFI" / "sbm" / "recovery" / "slot-a"
        slot.mkdir(parents=True, exist_ok=True)
        uki_name = f"sbm-core-{pretty}.efi"
        uki = slot / uki_name
        shutil.copy2(boot, uki)
        digest = sha256_file(uki)
        manifest = {
            "schema_version": "1.0",
            "active_slot": "slot-a",
            "slots": [
                {
                    "slot_name": "slot-a",
                    "uki_efi_path": f"\\EFI\\sbm\\recovery\\slot-a\\{uki_name}",
                    "sha256": digest,
                    "version": os.environ.get("IMAGE_VERSION", "0.1.0"),
                    "healthy": True,
                }
            ],
        }
        manifest_path = esp / "EFI" / "sbm" / "recovery" / "manifest.json"
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
        (esp / "EFI" / "sbm" / "state").mkdir(parents=True, exist_ok=True)
    finally:
        unmount(mountpoint, how)
        mountpoint.rmdir()
    subprocess.run(["sync"], check=False)
    image_digest = sha256_file(image)
    raw_sha_path.write_text(f"{image_digest}  {image.name}\n")
    print(f"uki sha256 {digest}")
    print(f"raw {image.name} {image.stat().st_size} {image_digest}")

    zst = pathlib.Path(str(image) + ".zst")
    zst_sha_path = output_dir / f"{zst.name}.sha256"
    if not compress:
        # Remove any stale compressed image so callers never pair an old .zst
        # with a newly built .raw image.
        for stale in (zst, zst_sha_path):
            if stale.exists():
                stale.unlink()
        print("zst skipped (SBM_COMPRESS=0)")
        return 0

    if not force_compress and prev_raw_digest == image_digest and zst.is_file() and zst_sha_path.is_file():
        print(f"zst reused {zst.name} {zst.stat().st_size} (raw sha256 unchanged)")
        return 0

    # One thread. Level 19 with every core can exhaust an 8 GiB builder.
    subprocess.run(
        ["zstd", "-19", "-T1", "-f", str(image), "-o", str(zst)],
        check=True,
    )
    zst_sha_path.write_text(f"{sha256_file(zst)}  {zst.name}\n")
    print(f"zst {zst.name} {zst.stat().st_size}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
